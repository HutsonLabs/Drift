//! `SpikeHost`: the vendored `ironrdp-server` with `egfx`, as a loopback RDP host (task HOST-1,
//! ADR `HOST-1-server-spike`).
//!
//! It is the first Drift-side use of the server half of IronRDP: NLA (CredSSP over TLS) with one
//! account, a fixed-size display that sends no bitmap updates, the EGFX bridge, and the Display
//! Control channel `ironrdp-server` always offers. Once the client's graphics channel is ready the
//! host creates one surface the size of the desktop, maps it to the output and sends each queued
//! access unit as one AVC420 frame. Client monitor layouts are recorded.
//!
//! It exists to answer what `ironrdp-server` can do for Drift Host before HOST-2 builds the real
//! listener; tests drive Drift's own client (`drift-rdp`) against it.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use drift_core::Size;
use ironrdp_displaycontrol::pdu::DisplayControlMonitorLayout;
use ironrdp_dvc::{DvcMessage, encode_dvc_messages};
use ironrdp_egfx::pdu::{Avc420Region, CapabilitiesAdvertisePdu, CapabilitySet};
use ironrdp_egfx::server::{GraphicsPipelineHandler, GraphicsPipelineServer};
use ironrdp_server::{
    Credentials, DesktopSize, DisplayUpdate, EgfxServerMessage, GfxDvcBridge, GfxServerFactory,
    GfxServerHandle, RdpServer, RdpServerDisplay, RdpServerDisplayUpdates, ServerEvent, ServerEventSender,
    ServerResult,
};
use ironrdp_svc::ChannelFlags;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::fake_server::TestCert;

/// What the host saw and did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpikeLog {
    /// The client confirmed a capability set with AVC420 enabled.
    pub avc420_negotiated: bool,
    /// The client confirmed a capability set with AVC444 enabled.
    pub avc444_negotiated: bool,
    /// AVC420 frames queued for the client.
    pub frames_sent: u32,
    /// Frame acknowledgements received (frame ids).
    pub frames_acked: Vec<u32>,
    /// Primary-monitor sizes from Display Control layouts, in order.
    pub layouts: Vec<(u32, u32)>,
    /// The last connection error, if any.
    pub error: Option<String>,
}

type SharedLog = Arc<Mutex<SpikeLog>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running loopback host. The server thread stops when this is dropped.
#[derive(Debug)]
pub struct SpikeHost {
    port: u16,
    log: SharedLog,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for SpikeHost {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

impl SpikeHost {
    /// Listens on `127.0.0.1:<random>` with `cert`, accepting `user`/`password` over NLA, with a
    /// `size` desktop; `frames` are AVC420 access units (Annex-B) sent once the graphics channel
    /// is ready. Serves one connection at a time, for as long as the value lives.
    pub async fn start(
        cert: TestCert,
        user: &str,
        password: &str,
        size: Size<u32>,
        frames: Vec<Vec<u8>>,
    ) -> io::Result<Self> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let acceptor = cert.acceptor().map_err(io::Error::other)?;
        let log = SharedLog::default();
        let width = u16::try_from(size.width).map_err(io::Error::other)?;
        let height = u16::try_from(size.height).map_err(io::Error::other)?;
        let factory = SpikeGfx::new(Arc::clone(&log), frames, (width, height));
        let (user, password) = (user.to_owned(), password.to_owned());
        let display_log = Arc::clone(&log);
        let task_log = Arc::clone(&log);
        let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
        // `RdpServer::run_connection` is not `Send`: the host gets its own thread and runtime.
        std::thread::Builder::new().name("spike-host".into()).spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                return;
            };
            // `RdpServer` holds non-`Send` factories, so it is built on this thread too.
            let mut server = RdpServer::builder()
                .with_addr(addr)
                .with_hybrid(acceptor, cert.public_key().to_vec())
                .with_no_input()
                .with_display_handler(SpikeDisplay { size: DesktopSize { width, height }, log: display_log })
                .with_gfx_factory(Some(Box::new(factory)))
                .build();
            server.set_credentials(Some(Credentials { username: user, password, domain: None }));
            runtime.block_on(async move {
                let Ok(listener) = TcpListener::from_std(listener) else { return };
                loop {
                    let stream = tokio::select! {
                        accepted = listener.accept() => match accepted {
                            Ok((stream, _)) => stream,
                            Err(_) => return,
                        },
                        _ = &mut stopped => return,
                    };
                    tokio::select! {
                        result = server.run_connection(stream) => {
                            if let Err(e) = result {
                                lock(&task_log).error = Some(format!("{e:?}"));
                            }
                        }
                        _ = &mut stopped => return,
                    }
                }
            });
        })?;
        Ok(Self { port: addr.port(), log, stop: Some(stop) })
    }

    /// The listening port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// A snapshot of what happened so far.
    pub fn log(&self) -> SpikeLog {
        lock(&self.log).clone()
    }
}

/// A fixed-size display that never sends bitmap updates (pictures go over EGFX).
struct SpikeDisplay {
    size: DesktopSize,
    log: SharedLog,
}

struct NoUpdates;

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for NoUpdates {
    async fn next_update(&mut self) -> ServerResult<Option<DisplayUpdate>> {
        std::future::pending().await
    }
}

#[async_trait::async_trait]
impl RdpServerDisplay for SpikeDisplay {
    async fn size(&mut self) -> DesktopSize {
        self.size
    }

    async fn updates(&mut self) -> ServerResult<Box<dyn RdpServerDisplayUpdates>> {
        Ok(Box::new(NoUpdates))
    }

    fn request_layout(&mut self, layout: DisplayControlMonitorLayout) {
        let primary = layout.monitors().iter().find(|m| m.is_primary()).or_else(|| layout.monitors().first());
        if let Some(monitor) = primary {
            lock(&self.log).layouts.push(monitor.dimensions());
        }
    }
}

/// Builds the EGFX bridge and, once the client is ready, sends the queued frames.
struct SpikeGfx {
    log: SharedLog,
    frames: Arc<Mutex<VecDeque<Vec<u8>>>>,
    size: (u16, u16),
    sender: Arc<Mutex<Option<mpsc::UnboundedSender<ServerEvent>>>>,
    handle: Arc<Mutex<Option<GfxServerHandle>>>,
}

impl SpikeGfx {
    fn new(log: SharedLog, frames: Vec<Vec<u8>>, size: (u16, u16)) -> Self {
        Self {
            log,
            frames: Arc::new(Mutex::new(frames.into())),
            size,
            sender: Arc::default(),
            handle: Arc::default(),
        }
    }

    fn handler(&self) -> SpikeGfxHandler {
        SpikeGfxHandler {
            log: Arc::clone(&self.log),
            frames: Arc::clone(&self.frames),
            size: self.size,
            sender: Arc::clone(&self.sender),
            handle: Arc::clone(&self.handle),
        }
    }
}

impl ServerEventSender for SpikeGfx {
    fn set_sender(&mut self, sender: mpsc::UnboundedSender<ServerEvent>) {
        *lock(&self.sender) = Some(sender);
    }
}

impl GfxServerFactory for SpikeGfx {
    fn build_gfx_handler(&self) -> Box<dyn GraphicsPipelineHandler> {
        Box::new(self.handler())
    }

    fn build_server_with_handle(&self) -> Option<(GfxDvcBridge, GfxServerHandle)> {
        let server: GfxServerHandle =
            Arc::new(Mutex::new(GraphicsPipelineServer::new(Box::new(self.handler()))));
        *lock(&self.handle) = Some(Arc::clone(&server));
        Some((GfxDvcBridge::new(Arc::clone(&server)), server))
    }
}

struct SpikeGfxHandler {
    log: SharedLog,
    frames: Arc<Mutex<VecDeque<Vec<u8>>>>,
    size: (u16, u16),
    sender: Arc<Mutex<Option<mpsc::UnboundedSender<ServerEvent>>>>,
    handle: Arc<Mutex<Option<GfxServerHandle>>>,
}

impl GraphicsPipelineHandler for SpikeGfxHandler {
    fn capabilities_advertise(&mut self, _pdu: &CapabilitiesAdvertisePdu) {}

    fn on_ready(&mut self, _negotiated: &CapabilitySet) {
        // `on_ready` runs with the server locked (inside `process`); send from a task.
        let (log, frames, size) = (Arc::clone(&self.log), Arc::clone(&self.frames), self.size);
        let (sender, handle) = (Arc::clone(&self.sender), Arc::clone(&self.handle));
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            let Some(server) = lock(&handle).clone() else { return };
            let messages = {
                let mut server = lock(&server);
                {
                    let mut log = lock(&log);
                    log.avc420_negotiated = server.supports_avc420();
                    log.avc444_negotiated = server.supports_avc444();
                }
                let Some(surface) = server.create_surface(size.0, size.1) else { return };
                server.map_surface_to_output(surface, 0, 0);
                let region = Avc420Region {
                    left: 0,
                    top: 0,
                    right: size.0,
                    bottom: size.1,
                    quantization_parameter: 22,
                    quality: 100,
                };
                while let Some(au) = lock(&frames).pop_front() {
                    if server.send_avc420_frame(surface, &au, std::slice::from_ref(&region), 0).is_some() {
                        lock(&log).frames_sent += 1;
                    }
                }
                let Some(channel) = server.channel_id() else { return };
                let out: Vec<DvcMessage> = server.drain_output();
                encode_dvc_messages(channel, out, ChannelFlags::empty())
            };
            if let (Ok(messages), Some(tx)) = (messages, lock(&sender).as_ref()) {
                let _ = tx.send(ServerEvent::Egfx(EgfxServerMessage::SendMessages { messages }));
            }
        });
    }

    fn on_frame_ack(&mut self, frame_id: u32, _queue_depth: u32, _total_frames_decoded: u32) {
        lock(&self.log).frames_acked.push(frame_id);
    }
}
