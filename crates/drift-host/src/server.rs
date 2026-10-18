//! The listener and the connection loop (ADR `HOST-2-host-skeleton`).
//!
//! `ironrdp-server`'s `RdpServer` is not `Send` (HOST-1), so the host runs it on a thread of its
//! own with a current-thread runtime. The loop owns the listener: while a session runs, further
//! connections are accepted and closed at once (one session at a time, [`SessionGate`]), so a
//! second client fails fast instead of hanging in the backlog.
//!
//! Pictures go over EGFX as AVC420. When the client's graphics channel is ready the host creates
//! one surface the size of the desktop and sends a keyframe from its [`FrameSource`]; a Display
//! Control layout resizes the desktop (`resize_with_monitors`, then a new surface and keyframe).
//! The `Drift::Mac::v1` channel (EXT-1) is offered on every connection.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use drift_core::Size;
use drift_ext::channel::{DriftMacServer, Link};
use drift_ext::wire::Capabilities;
use ironrdp_displaycontrol::pdu::DisplayControlMonitorLayout;
use ironrdp_dvc::{DrdynvcServer, DvcMessage, encode_dvc_messages};
use ironrdp_egfx::pdu::{Avc420Region, CapabilitiesAdvertisePdu, CapabilitySet};
use ironrdp_egfx::server::{GraphicsPipelineHandler, GraphicsPipelineServer};
use ironrdp_pdu::gcc::{Monitor, MonitorFlags};
use ironrdp_server::{
    Credentials, DesktopSize, DisplayUpdate, DvcServerFactory, EgfxServerMessage, GfxDvcBridge,
    GfxServerFactory, GfxServerHandle, RdpServer, RdpServerDisplay, RdpServerDisplayUpdates, ServerEvent,
    ServerEventSender, ServerResult,
};
use ironrdp_svc::ChannelFlags;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::audio::{AudioSourceFactory, HostSound};
use crate::clipboard::{HostClipboard, PasteboardFactory};
use crate::frames::FrameSource;
use crate::gate::SessionGate;
use crate::identity::HostIdentity;
use crate::input::{InputSink, SinkHandler};
use crate::layout::clamp_layout;

/// Makes the frame source for a connection.
pub type FrameSourceFactory = Box<dyn Fn() -> Box<dyn FrameSource> + Send>;

/// Everything [`Host::start`] needs.
pub struct HostOptions {
    /// Where to listen (`127.0.0.1:0` picks a free port).
    pub listen: SocketAddr,
    /// The TLS identity.
    pub identity: HostIdentity,
    /// The NLA account's user name.
    pub username: String,
    /// The NLA account's password.
    pub password: zeroize::Zeroizing<String>,
    /// The desktop size before a client asks for its own.
    pub desktop: Size<u32>,
    /// What the host shows.
    pub frames: FrameSourceFactory,
    /// Where input goes.
    pub input: Box<dyn InputSink>,
    /// The Mac pasteboard to share over CLIPRDR (HOST-11); `None` offers no clipboard.
    pub clipboard: Option<PasteboardFactory>,
    /// The sound to stream over rdpsnd (HOST-11); `None` offers no sound.
    pub audio: Option<AudioSourceFactory>,
}

/// What the host has done so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostStats {
    /// Sessions admitted.
    pub sessions_served: u32,
    /// Sessions that have ended.
    pub sessions_ended: u32,
    /// Connections refused because a session was running.
    pub refused: u32,
    /// The current client confirmed AVC420.
    pub avc420_negotiated: bool,
    /// Keyframes sent.
    pub frames_sent: u32,
    /// Frame acknowledgements received.
    pub frames_acked: u32,
    /// Desktop sizes applied from client layouts, in order.
    pub layouts: Vec<Size<u32>>,
    /// The `Drift::Mac::v1` channel's state on the latest connection (`Absent` until opened).
    pub drift_mac: String,
    /// The latest connection or encoder error.
    pub last_error: Option<String>,
    /// The current connection's clipboard channel is ready (HOST-11).
    pub clipboard_ready: bool,
    /// Sound waves sent (HOST-11).
    pub audio_waves: u32,
}

type SharedStats = Arc<Mutex<HostStats>>;

fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running host. Dropping it (or [`Host::stop`]) stops the listener and the session.
pub struct Host {
    addr: SocketAddr,
    stats: SharedStats,
    drift_mac: Arc<Mutex<Option<Link>>>,
    clipboard_ready: Option<Arc<Mutex<bool>>>,
    audio_waves: Option<Arc<std::sync::atomic::AtomicU32>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host").field("addr", &self.addr).finish_non_exhaustive()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

impl Host {
    /// Binds the listener and starts serving on a thread of its own.
    pub fn start(options: HostOptions) -> std::io::Result<Self> {
        let HostOptions { listen, identity, username, password, desktop, frames, input, clipboard, audio } =
            options;
        let listener = std::net::TcpListener::bind(listen)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let acceptor = identity.acceptor().map_err(std::io::Error::other)?;
        let stats = SharedStats::default();
        // HOST-11: the clipboard thread starts with the host; the sound factory per connection.
        let clipboard = clipboard.map(HostClipboard::start).transpose()?;
        let clipboard_ready = clipboard.as_ref().map(HostClipboard::ready_flag);
        let sound = audio.map(HostSound::new);
        let audio_waves = sound.as_ref().map(HostSound::waves);
        let drift_mac: Arc<Mutex<Option<Link>>> = Arc::default();
        let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
        let (thread_stats, thread_mac) = (Arc::clone(&stats), Arc::clone(&drift_mac));
        let thread = std::thread::Builder::new().name("drift-host".into()).spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                return;
            };
            let size = Arc::new(Mutex::new(desktop));
            let gfx = Arc::new(GfxShared {
                sender: Mutex::new(None),
                handle: Mutex::new(None),
                source: Mutex::new(frames()),
                size: Arc::clone(&size),
                stats: Arc::clone(&thread_stats),
            });
            // `RdpServer` holds non-`Send` parts, so it is built on this thread.
            let mut server = RdpServer::builder()
                .with_addr(addr)
                .with_hybrid(acceptor, identity.public_key().to_vec())
                .with_input_handler(SinkHandler(input))
                .with_display_handler(HostDisplay { size: Arc::clone(&size), desktop, gfx: Arc::clone(&gfx) })
                .with_gfx_factory(Some(Box::new(HostGfx(Arc::clone(&gfx)))))
                .with_cliprdr_factory(
                    clipboard.map(|c| Box::new(c) as Box<dyn ironrdp_server::CliprdrServerFactory>),
                )
                .with_sound_factory(sound.map(|s| Box::new(s) as Box<dyn ironrdp_server::SoundServerFactory>))
                .build();
            server.add_dvc_factory(Box::new(DriftMacFactory(thread_mac)));
            server.set_credentials(Some(Credentials {
                username,
                password: password.to_string(),
                domain: None,
            }));
            runtime.block_on(serve(&mut server, listener, &thread_stats, &mut stopped));
        })?;
        Ok(Self {
            addr,
            stats,
            drift_mac,
            clipboard_ready,
            audio_waves,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    /// The bound address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The bound port.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// A snapshot of what happened so far.
    pub fn stats(&self) -> HostStats {
        let mut stats = lock(&self.stats).clone();
        stats.drift_mac = lock(&self.drift_mac)
            .as_ref()
            .map_or_else(|| "Absent".to_owned(), |l| format!("{:?}", l.state()));
        stats.clipboard_ready = self.clipboard_ready.as_ref().is_some_and(|r| *lock(r));
        stats.audio_waves =
            self.audio_waves.as_ref().map_or(0, |w| w.load(std::sync::atomic::Ordering::Relaxed));
        stats
    }

    /// Stops serving and waits for the server thread.
    pub fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Blocks until the server thread ends (it runs until stopped).
    pub fn wait(mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The connection loop: one admitted session at a time, extra connections closed at once.
async fn serve(
    server: &mut RdpServer,
    listener: std::net::TcpListener,
    stats: &SharedStats,
    stopped: &mut tokio::sync::oneshot::Receiver<()>,
) {
    let Ok(listener) = TcpListener::from_std(listener) else { return };
    let gate = SessionGate::new();
    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    continue;
                }
            },
            _ = &mut *stopped => return,
        };
        let Some(ticket) = gate.try_begin() else { continue };
        tracing::info!(%peer, "session starting");
        lock(stats).sessions_served += 1;
        let session = server.run_connection(stream);
        tokio::pin!(session);
        loop {
            tokio::select! {
                result = &mut session => {
                    if let Err(e) = result {
                        tracing::info!(error = ?e, "session ended with an error");
                        lock(stats).last_error = Some(format!("{e:?}"));
                    }
                    break;
                }
                accepted = listener.accept() => {
                    if let Ok((extra, extra_peer)) = accepted {
                        debug_assert!(gate.try_begin().is_none());
                        tracing::info!(peer = %extra_peer, "refused: a session is already running");
                        lock(stats).refused += 1;
                        drop(extra);
                    }
                }
                _ = &mut *stopped => return,
            }
        }
        drop(ticket);
        lock(stats).sessions_ended += 1;
        tracing::info!(%peer, "session ended");
    }
}

/// The desktop as `ironrdp-server` sees it: a size, no bitmap updates (pictures go over EGFX),
/// and Display Control layouts that resize it.
struct HostDisplay {
    size: Arc<Mutex<Size<u32>>>,
    desktop: Size<u32>,
    gfx: Arc<GfxShared>,
}

struct NoUpdates;

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for NoUpdates {
    async fn next_update(&mut self) -> ServerResult<Option<DisplayUpdate>> {
        std::future::pending().await
    }
}

#[async_trait::async_trait]
impl RdpServerDisplay for HostDisplay {
    async fn size(&mut self) -> DesktopSize {
        // A new connection starts from the configured desktop.
        *lock(&self.size) = self.desktop;
        to_desktop_size(self.desktop)
    }

    async fn updates(&mut self) -> ServerResult<Box<dyn RdpServerDisplayUpdates>> {
        Ok(Box::new(NoUpdates))
    }

    fn request_layout(&mut self, layout: DisplayControlMonitorLayout) {
        let primary = layout.monitors().iter().find(|m| m.is_primary()).or_else(|| layout.monitors().first());
        let Some(monitor) = primary else { return };
        let (width, height) = monitor.dimensions();
        let size = clamp_layout(Size::new(width, height));
        tracing::info!(width = size.width, height = size.height, "client layout");
        *lock(&self.size) = size;
        lock(&self.gfx.stats).layouts.push(size);
        let gfx = Arc::clone(&self.gfx);
        tokio::spawn(async move { gfx.present(true) });
    }
}

fn to_desktop_size(size: Size<u32>) -> DesktopSize {
    DesktopSize {
        width: u16::try_from(size.width).unwrap_or(u16::MAX),
        height: u16::try_from(size.height).unwrap_or(u16::MAX),
    }
}

/// State shared by the EGFX factory, its handlers and the display.
struct GfxShared {
    sender: Mutex<Option<mpsc::UnboundedSender<ServerEvent>>>,
    handle: Mutex<Option<GfxServerHandle>>,
    source: Mutex<Box<dyn FrameSource>>,
    size: Arc<Mutex<Size<u32>>>,
    stats: SharedStats,
}

impl GfxShared {
    /// Sends a keyframe of the current desktop on a new surface; with `resize`, resets the
    /// graphics output to the new size first.
    fn present(&self, resize: bool) {
        let size = *lock(&self.size);
        let au = match lock(&self.source).keyframe(size) {
            Ok(au) => au,
            Err(e) => {
                tracing::warn!(error = %e, "no picture");
                lock(&self.stats).last_error = Some(e.to_string());
                return;
            }
        };
        let Some(server) = lock(&self.handle).clone() else { return };
        let (w, h) = (to_desktop_size(size).width, to_desktop_size(size).height);
        let messages = {
            let mut server = lock(&server);
            if !server.is_ready() {
                return;
            }
            lock(&self.stats).avc420_negotiated = server.supports_avc420();
            if resize {
                let primary = Monitor {
                    left: 0,
                    top: 0,
                    right: i32::from(w) - 1,
                    bottom: i32::from(h) - 1,
                    flags: MonitorFlags::PRIMARY,
                };
                server.resize_with_monitors(w, h, vec![primary]);
            }
            let Some(surface) = server.create_surface(w, h) else { return };
            server.map_surface_to_output(surface, 0, 0);
            let region = Avc420Region {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
                quantization_parameter: 22,
                quality: 100,
            };
            if server.send_avc420_frame(surface, &au, std::slice::from_ref(&region), 0).is_some() {
                lock(&self.stats).frames_sent += 1;
            }
            let Some(channel) = server.channel_id() else { return };
            let out: Vec<DvcMessage> = server.drain_output();
            encode_dvc_messages(channel, out, ChannelFlags::empty())
        };
        match (messages, lock(&self.sender).as_ref()) {
            (Ok(messages), Some(tx)) => {
                let _ = tx.send(ServerEvent::Egfx(EgfxServerMessage::SendMessages { messages }));
            }
            (Err(e), _) => tracing::warn!(error = %e, "could not encode graphics messages"),
            (Ok(_), None) => {}
        }
    }
}

/// Builds the EGFX bridge per connection.
struct HostGfx(Arc<GfxShared>);

impl ServerEventSender for HostGfx {
    fn set_sender(&mut self, sender: mpsc::UnboundedSender<ServerEvent>) {
        *lock(&self.0.sender) = Some(sender);
    }
}

impl GfxServerFactory for HostGfx {
    fn build_gfx_handler(&self) -> Box<dyn GraphicsPipelineHandler> {
        Box::new(HostGfxHandler(Arc::clone(&self.0)))
    }

    fn build_server_with_handle(&self) -> Option<(GfxDvcBridge, GfxServerHandle)> {
        let server: GfxServerHandle =
            Arc::new(Mutex::new(GraphicsPipelineServer::new(Box::new(HostGfxHandler(Arc::clone(&self.0))))));
        *lock(&self.0.handle) = Some(Arc::clone(&server));
        Some((GfxDvcBridge::new(Arc::clone(&server)), server))
    }
}

struct HostGfxHandler(Arc<GfxShared>);

impl GraphicsPipelineHandler for HostGfxHandler {
    fn capabilities_advertise(&mut self, _pdu: &CapabilitiesAdvertisePdu) {}

    fn on_ready(&mut self, _negotiated: &CapabilitySet) {
        // `on_ready` runs with the pipeline locked; present from a task.
        let gfx = Arc::clone(&self.0);
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            gfx.present(false);
        });
    }

    fn on_frame_ack(&mut self, _frame_id: u32, _queue_depth: u32, _total_frames_decoded: u32) {
        lock(&self.0.stats).frames_acked += 1;
    }
}

/// Offers `Drift::Mac::v1` (EXT-1) on every connection and keeps the latest link.
struct DriftMacFactory(Arc<Mutex<Option<Link>>>);

impl DvcServerFactory for DriftMacFactory {
    fn attach(&self, dvc: DrdynvcServer) -> DrdynvcServer {
        let link = Link::new();
        *lock(&self.0) = Some(link.clone());
        dvc.with_dynamic_channel(DriftMacServer::new(link, Capabilities::NONE))
    }
}
