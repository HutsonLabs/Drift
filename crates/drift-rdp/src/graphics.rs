//! Graphics pipeline wiring (M1-1 actor seam of `drift_gfx`, see `docs/adr/M1-2-gfx-client.md`).
//!
//! Every leg gets a fresh [`GfxClient`] (a new connection is a new graphics pipeline), all of
//! them drawing into the tab's one [`FrameSink`] through [`SharedSink`]. H.264 goes to
//! VideoToolbox ([`VtDecoder`]), the CPU codecs share one process-wide [`TilePool`].
//! `FrameAcknowledge`s produced by `presented` callbacks wake the actor through a
//! [`Notify`]; [`flush_acks`] sends them on the graphics channel.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use drift_codec::TilePool;
use drift_core::{Bgra, Clock, DisconnectReason, Nv12Frame, Point, Rect, Size};
use drift_gfx::{FrameSink, GfxClient, PresentedCallback};
use drift_video::decode::VtDecoder;
use ironrdp_cliprdr::CliprdrClient;
use ironrdp_connector::ClientConnector;
use ironrdp_displaycontrol::client::DisplayControlClient;
use ironrdp_dvc::DrdynvcClient;
use ironrdp_session::ActiveStage;
use ironrdp_svc::{ChannelFlags, SvcProcessorMessages};
use tokio::sync::{Notify, watch};

use crate::gfx_ack::GfxAckOnly;

/// Observable state of the tab's sink, shared with the session actor.
struct SinkState {
    sink: Mutex<Box<dyn FrameSink>>,
    /// Output size of the last `ResetGraphics` (the current remote desktop size).
    output: watch::Sender<Option<Size<u32>>>,
    /// Frames handed to the sink since the last [`Graphics::take_presented_frames`].
    frames: AtomicU64,
    /// The session clock, shared with the render thread (M9-1 latency measurement).
    clock: Arc<dyn Clock>,
    /// When the graphics payload currently being processed came off the wire.
    wire_at: Mutex<Option<Instant>>,
    /// Decode-and-present latencies since the last [`Graphics::take_frame_latencies`].
    latencies: Mutex<Vec<Duration>>,
}

/// How many latency samples are buffered between two statistics ticks (about 60 arrive per
/// second); beyond that the oldest are dropped rather than growing the buffer.
const MAX_PENDING_LATENCIES: usize = 512;

/// The tab's frame sink, shared by the GFX clients of successive legs.
#[derive(Clone)]
pub(crate) struct SharedSink(Arc<SinkState>);

impl SharedSink {
    fn lock(&self) -> MutexGuard<'_, Box<dyn FrameSink>> {
        self.0.sink.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl FrameSink for SharedSink {
    fn reset(&mut self, output: Size<u32>) {
        self.lock().reset(output);
        self.0.output.send_replace(Some(output));
    }
    fn create_surface(&mut self, id: u16, size: Size<u32>) {
        self.lock().create_surface(id, size);
    }
    fn delete_surface(&mut self, id: u16) {
        self.lock().delete_surface(id);
    }
    fn map_surface_to_output(&mut self, id: u16, origin: Point<u32>) {
        self.lock().map_surface_to_output(id, origin);
    }
    fn blit_bgra(&mut self, id: u16, rect: Rect, stride: usize, data: &[u8]) {
        self.lock().blit_bgra(id, rect, stride, data);
    }
    fn blit_nv12(&mut self, id: u16, frame: &Nv12Frame, regions: &[Rect]) {
        self.lock().blit_nv12(id, frame, regions);
    }
    fn solid_fill(&mut self, id: u16, color: Bgra, rects: &[Rect]) {
        self.lock().solid_fill(id, color, rects);
    }
    fn surface_to_surface(&mut self, src: u16, dst: u16, rect: Rect, dests: &[Point<u32>]) {
        self.lock().surface_to_surface(src, dst, rect, dests);
    }
    fn surface_to_cache(&mut self, id: u16, rect: Rect, slot: u16) {
        self.lock().surface_to_cache(id, rect, slot);
    }
    fn cache_to_surface(&mut self, slot: u16, id: u16, dests: &[Point<u32>]) {
        self.lock().cache_to_surface(slot, id, dests);
    }
    fn evict_cache(&mut self, slot: u16) {
        self.lock().evict_cache(slot);
    }
    fn end_frame(&mut self, frame_id: u32, presented: PresentedCallback) {
        self.0.frames.fetch_add(1, Ordering::Relaxed);
        // The frame's decode started when its payload came off the wire (M9-1: the budget is
        // decode *and* present). `presented` runs on the render thread, so the measurement
        // ends there.
        let state = Arc::clone(&self.0);
        let started = *state.wire_at.lock().unwrap_or_else(PoisonError::into_inner);
        let timed: PresentedCallback = Box::new(move || {
            if let Some(started) = started {
                let elapsed = state.clock.now().saturating_duration_since(started);
                let mut pending = state.latencies.lock().unwrap_or_else(PoisonError::into_inner);
                if pending.len() >= MAX_PENDING_LATENCIES {
                    pending.remove(0);
                }
                pending.push(elapsed);
            }
            presented();
        });
        self.lock().end_frame(frame_id, timed);
    }
    fn set_visible(&mut self, visible: bool) {
        self.lock().set_visible(visible);
    }
}

/// The process-wide CPU codec pool (`None` if the OS refused to spawn its threads).
fn tile_pool() -> Option<TilePool> {
    static POOL: OnceLock<Option<TilePool>> = OnceLock::new();
    POOL.get_or_init(|| {
        TilePool::with_default_threads()
            .map_err(|e| tracing::error!(error = %e, "cannot start the codec tile pool"))
            .ok()
    })
    .clone()
}

/// Per-session graphics state: the shared sink and the ack wake-up.
pub(crate) struct Graphics {
    sink: SharedSink,
    acks_ready: Arc<Notify>,
}

impl Graphics {
    /// Wraps the tab's sink; `clock` timestamps the latency measurements.
    pub(crate) fn new(sink: Box<dyn FrameSink>, clock: Arc<dyn Clock>) -> Self {
        let (output, _) = watch::channel(None);
        let state = SinkState {
            sink: Mutex::new(sink),
            output,
            frames: AtomicU64::new(0),
            clock,
            wire_at: Mutex::new(None),
            latencies: Mutex::new(Vec::new()),
        };
        Self { sink: SharedSink(Arc::new(state)), acks_ready: Arc::new(Notify::new()) }
    }

    /// A graphics payload came off the wire at `now`; frames finished while processing it are
    /// measured from here (M9-1 decode + present).
    pub(crate) fn mark_wire(&self, now: Instant) {
        *self.sink.0.wire_at.lock().unwrap_or_else(PoisonError::into_inner) = Some(now);
    }

    /// Decode-and-present latencies measured since the last call.
    pub(crate) fn take_frame_latencies(&self) -> Vec<Duration> {
        std::mem::take(&mut *self.sink.0.latencies.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Watches the output size of the last `ResetGraphics` (the remote desktop size).
    pub(crate) fn output_size(&self) -> watch::Receiver<Option<Size<u32>>> {
        self.sink.0.output.subscribe()
    }

    /// Frames handed to the sink since the last call (for the fps statistic).
    pub(crate) fn take_presented_frames(&self) -> u64 {
        self.sink.0.frames.swap(0, Ordering::Relaxed)
    }

    /// Shows or hides the session: the graphics client switches its acknowledgement policy
    /// (`SUSPEND_FRAME_ACKNOWLEDGEMENT` while hidden) and pauses the render thread.
    pub(crate) fn set_visible(&self, stage: &mut ActiveStage, visible: bool) {
        match stage.get_dvc_mut::<GfxClient>() {
            Some(mut gfx) => gfx.processor_mut().set_visible(visible),
            None => self.sink.clone().set_visible(visible),
        }
    }

    /// Frames handed to the graphics pipeline but not yet acknowledged.
    pub(crate) fn unacked_frames(stage: &ActiveStage) -> u32 {
        stage.get_dvc::<GfxClient>().map_or(0, |gfx| gfx.processor().acks().in_flight())
    }

    /// Notified whenever a presented frame queued a `FrameAcknowledge`.
    pub(crate) fn acks_ready(&self) -> Arc<Notify> {
        Arc::clone(&self.acks_ready)
    }

    /// The static channels of one leg: DRDYNVC with a fresh GFX client. Without the graphics
    /// pipeline g-r-d terminates the session, so if the codec pool is unavailable the leg still
    /// gets the ack-only listener.
    pub(crate) fn channels_for_leg(
        &self,
        display_control: DisplayControlClient,
        clipboard: Option<CliprdrClient>,
        drift_mac: Option<&drift_ext::channel::Link>,
        sound: Option<ironrdp_rdpsnd::client::Rdpsnd>,
    ) -> impl FnOnce(ClientConnector) -> ClientConnector + Send + use<> {
        let drdynvc = DrdynvcClient::new().with_dynamic_channel(display_control);
        // EXT-1: offered only when the app asked for it; a non-Drift server never opens it.
        let drdynvc = match drift_mac {
            Some(link) => {
                link.reset();
                drdynvc.with_dynamic_channel(drift_ext::channel::DriftMacClient::new(
                    link.clone(),
                    drift_ext::wire::Capabilities::NONE,
                ))
            }
            None => drdynvc,
        };
        let drdynvc = match tile_pool() {
            Some(pool) => {
                let gfx = GfxClient::new(Box::new(self.sink.clone()), Box::new(VtDecoder::new()), pool);
                let notify = Arc::clone(&self.acks_ready);
                gfx.acks().set_notifier(move || notify.notify_one());
                drdynvc.with_dynamic_channel(gfx)
            }
            None => drdynvc.with_dynamic_channel(GfxAckOnly::default()),
        };
        move |connector| {
            let connector = connector.with_static_channel(drdynvc);
            let connector = match clipboard {
                Some(cliprdr) => connector.with_static_channel(cliprdr),
                None => connector,
            };
            match sound {
                Some(rdpsnd) => connector.with_static_channel(rdpsnd),
                None => connector,
            }
        }
    }
}

/// Encodes the queued `FrameAcknowledge`s as a frame for the wire (`None` when nothing is due).
pub(crate) fn flush_acks(stage: &ActiveStage) -> Result<Option<Vec<u8>>, DisconnectReason> {
    let Some(gfx) = stage.get_dvc::<GfxClient>() else { return Ok(None) };
    let messages = gfx.processor().acks().drain_messages();
    if messages.is_empty() {
        return Ok(None);
    }
    let svc = ironrdp_dvc::encode_dvc_messages(gfx.channel_id(), messages, ChannelFlags::empty())
        .map_err(|e| DisconnectReason::ProtocolError(format!("encode frame acknowledge: {e}")))?;
    stage
        .process_svc_processor_messages(SvcProcessorMessages::<DrdynvcClient>::new(svc))
        .map(Some)
        .map_err(|e| DisconnectReason::ProtocolError(format!("send frame acknowledge: {e}")))
}

/// The graphics pipeline's own error, if the last processing failure came from it.
pub(crate) fn take_gfx_error(stage: &mut ActiveStage) -> Option<DisconnectReason> {
    stage.get_dvc_mut::<GfxClient>()?.processor_mut().take_error().map(|e| e.disconnect_reason())
}
