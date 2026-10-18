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
//!
//! With a live source (`HostOptions::live`, the screen; ADR `HOST-pipeline`) the desktop is the
//! source's size, a pump thread ([`crate::live`]) streams pictures and pointer updates while the
//! session runs, and Display Control layouts are answered with the source's size (HOST-10 adds
//! virtual displays sized to the client).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
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
use crate::display::{DisplayManager, DisplayTarget, LayoutRequest};
use crate::frames::{FrameError, FrameSource};
use crate::gate::SessionGate;
use crate::identity::HostIdentity;
use crate::input::{InputSink, SharedSink, SinkHandler};
use crate::layout::clamp_layout;
use crate::live::{GfxTarget, Pump};
use crate::pairing::{AuthPhase, ConnectionAuth, Pairing};
use crate::pipeline::{LiveFrame, LiveRequest, LiveSource};

/// Makes the frame source for a connection.
pub type FrameSourceFactory = Box<dyn Fn() -> Box<dyn FrameSource> + Send>;

/// Makes the live picture for a connection (the screen) for what [`LiveRequest`] asks: which
/// display (HOST-10's virtual one, or the main one) and which codec (AVC444 or AVC420); an error
/// falls back to the test pattern.
pub type LiveFactory = Box<dyn Fn(LiveRequest) -> Result<Box<dyn LiveSource>, FrameError> + Send>;

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
    /// What the host shows when there is no live picture.
    pub frames: FrameSourceFactory,
    /// The live picture, if any (HOST-pipeline).
    pub live: Option<LiveFactory>,
    /// Where input goes.
    pub input: Box<dyn InputSink>,
    /// The Mac pasteboard to share over CLIPRDR (HOST-11); `None` offers no clipboard.
    pub clipboard: Option<PasteboardFactory>,
    /// The sound to stream over rdpsnd (HOST-11); `None` offers no sound.
    pub audio: Option<AudioSourceFactory>,
    /// Turns client layouts into the display to capture: a virtual display sized to the client,
    /// or the physical one (HOST-10).
    pub displays: Box<dyn DisplayManager>,
    /// Device pairing (HOST-8). With it, a connection sees nothing and its input is ignored until
    /// its device paired or proved its key over `Drift::Mac::v1`, and it is dropped if it does
    /// not within [`Pairing::auth_timeout`]. Without it (the HOST-2 skeleton, tests), every
    /// connection that passes NLA is served.
    pub pairing: Option<Pairing>,
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
    /// The session socket carries the interactive-video service class (HOST-7).
    pub video_service_class: bool,
    /// How far behind the current client is (HOST-7; EXT-4 turns it into quality hints).
    pub backlog: crate::link::Backlog,
    /// Heartbeats sent on an idle link (HOST-7).
    pub heartbeats: u64,
    /// Frames sent as AVC444 (both views), for clients that confirmed it (AVC444-client).
    pub avc444_frames: u64,
    /// The current connection's clipboard channel is ready (HOST-11).
    pub clipboard_ready: bool,
    /// Sound waves sent (HOST-11).
    pub audio_waves: u32,
    /// The display the host captures since the latest client layout (HOST-10); `None` before one.
    pub display: Option<DisplayTarget>,
    /// Connections whose device paired or authenticated (HOST-8).
    pub authorized: u32,
    /// Connections refused over `Drift::Mac::v1` (wrong code, unknown device, bad signature).
    pub auth_refused: u32,
    /// Connections dropped for not authenticating in time.
    pub auth_timeouts: u32,
}

type SharedStats = Arc<Mutex<HostStats>>;
type SharedDisplays = Arc<Mutex<Box<dyn DisplayManager>>>;

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
        let HostOptions {
            listen,
            identity,
            username,
            password,
            desktop,
            frames,
            live,
            input,
            clipboard,
            audio,
            displays,
            pairing,
        } = options;
        let displays: SharedDisplays = Arc::new(Mutex::new(displays));
        let listener = std::net::TcpListener::bind(listen)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let acceptor = identity.acceptor().map_err(std::io::Error::other)?;
        let stats = SharedStats::default();
        // HOST-11: the clipboard thread starts with the host; the sound factory per connection.
        // HOST-8: the current connection's authentication, shared by everything it may reach.
        let current = Arc::new(Mutex::new(ConnectionAuth::open()));
        let authorized: crate::gate::Authorized = {
            let current = Arc::clone(&current);
            Arc::new(move || lock(&current).is_authorized())
        };
        let clipboard =
            clipboard.map(|f| HostClipboard::start_gated(f, Arc::clone(&authorized))).transpose()?;
        let clipboard_ready = clipboard.as_ref().map(HostClipboard::ready_flag);
        let sound = audio.map(|f| HostSound::new(f).gated(authorized));
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
                auth: Arc::clone(&current),
                sender: Mutex::new(None),
                handle: Mutex::new(None),
                source: Mutex::new(frames()),
                size: Arc::clone(&size),
                stats: Arc::clone(&thread_stats),
                live_factory: Mutex::new(live),
                request: Mutex::new(LiveRequest::default()),
                live: Mutex::new(None),
                pump: Mutex::new(None),
                surface: Mutex::new(None),
                pointer: Mutex::new(None),
                keyframe_wanted: AtomicBool::new(false),
                probe: Mutex::new(None),
                last_pointer: Mutex::new(None),
            });
            let input: SharedSink = Arc::new(Mutex::new(input));
            // `RdpServer` holds non-`Send` parts, so it is built on this thread.
            let mut server = RdpServer::builder()
                .with_addr(addr)
                .with_hybrid(acceptor, identity.public_key().to_vec())
                .with_input_handler(GatedInput {
                    sink: SinkHandler(Arc::clone(&input)),
                    auth: Arc::clone(&current),
                })
                .with_display_handler(HostDisplay {
                    size: Arc::clone(&size),
                    desktop,
                    gfx: Arc::clone(&gfx),
                    displays: Arc::clone(&displays),
                })
                .with_gfx_factory(Some(Box::new(HostGfx(Arc::clone(&gfx)))))
                .with_cliprdr_factory(
                    clipboard.map(|c| Box::new(c) as Box<dyn ironrdp_server::CliprdrServerFactory>),
                )
                .with_sound_factory(sound.map(|s| Box::new(s) as Box<dyn ironrdp_server::SoundServerFactory>))
                .build();
            server.add_dvc_factory(Box::new(DriftMacFactory {
                link: thread_mac,
                pairing: pairing.clone(),
                host_fingerprint: *identity.fingerprint().as_bytes(),
                current: Arc::clone(&current),
            }));
            server.set_credentials(Some(Credentials {
                username,
                password: password.to_string(),
                domain: None,
            }));
            let on_start = |stream: &tokio::net::TcpStream| {
                use std::os::fd::AsRawFd as _;
                let fd = stream.as_raw_fd();
                let video = crate::link::set_service_class(fd, crate::link::NET_SERVICE_TYPE_VI)
                    .inspect_err(|e| tracing::warn!(error = %e, "could not set the video service class"))
                    .is_ok();
                lock(&thread_stats).video_service_class = video;
                *lock(&gfx.probe) = crate::link::SocketProbe::new(fd).ok();
            };
            let on_end = || {
                // Stop the pump (and the capture with it), and let go of held keys and buttons.
                drop(lock(&gfx.pump).take());
                drop(lock(&gfx.live).take());
                lock(&input).session_ended();
                // The next client starts on the main display, codec chosen afresh.
                *lock(&gfx.request) = LiveRequest::default();
                *lock(&gfx.probe) = None;
                *lock(&gfx.last_pointer) = None;
            };
            let connections = Connections { pairing, current, gfx: Arc::clone(&gfx), displays };
            runtime.block_on(serve(
                &mut server,
                listener,
                &thread_stats,
                &mut stopped,
                &on_start,
                &on_end,
                &connections,
            ));
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

/// What the connection loop needs beyond the server: pairing and the current connection's
/// authentication, which the display and the input check.
struct Connections {
    pairing: Option<Pairing>,
    current: Arc<Mutex<ConnectionAuth>>,
    gfx: Arc<GfxShared>,
    /// The display manager; each session's virtual display is released when it ends (HOST-10).
    displays: SharedDisplays,
}

/// How long a refused connection stays up so the refusal reaches the client.
const REFUSAL_GRACE: std::time::Duration = std::time::Duration::from_millis(400);

/// Sleeps until `deadline`, or forever without one.
async fn until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// The connection loop: one admitted session at a time, extra connections closed at once.
async fn serve(
    server: &mut RdpServer,
    listener: std::net::TcpListener,
    stats: &SharedStats,
    stopped: &mut tokio::sync::oneshot::Receiver<()>,
    on_start: &dyn Fn(&tokio::net::TcpStream),
    on_end: &dyn Fn(),
    connections: &Connections,
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
        on_start(&stream);
        let auth = match &connections.pairing {
            Some(_) => ConnectionAuth::pending(),
            None => ConnectionAuth::open(),
        };
        *lock(&connections.current) = auth.clone();
        // Pairing on: the device must pair or authenticate in time, or it is dropped.
        let mut auth_deadline =
            connections.pairing.as_ref().map(|p| tokio::time::Instant::now() + p.auth_timeout());
        let mut kick_at: Option<tokio::time::Instant> = None;
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
                () = auth.changed(), if auth_deadline.is_some() => match auth.phase() {
                    AuthPhase::Authorized(device) => {
                        tracing::info!(device = %device.name, %peer, "connection authorized");
                        lock(stats).authorized += 1;
                        auth_deadline = None;
                        let gfx = Arc::clone(&connections.gfx);
                        tokio::spawn(async move { gfx.show() });
                    }
                    AuthPhase::Refused => {
                        lock(stats).auth_refused += 1;
                        auth_deadline = None;
                        kick_at = Some(tokio::time::Instant::now() + REFUSAL_GRACE);
                    }
                    AuthPhase::Pending => {}
                },
                () = until(auth_deadline) => {
                    tracing::info!(%peer, "no pairing or authentication in time; dropping the connection");
                    lock(stats).auth_timeouts += 1;
                    break;
                }
                () = until(kick_at) => {
                    tracing::info!(%peer, "device refused; dropping the connection");
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
        on_end();
        drop(ticket);
        // The virtual display goes with the session; the next client gets its own size.
        lock(&connections.displays).release();
        lock(stats).display = None;
        // Nothing reaches the next connection's input or display until it is authorized.
        *lock(&connections.current) = match &connections.pairing {
            Some(_) => ConnectionAuth::pending(),
            None => ConnectionAuth::open(),
        };
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
    displays: SharedDisplays,
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
        // A new connection starts from the live picture's size, or the configured desktop.
        let size = self.gfx.open_live().unwrap_or(self.desktop);
        *lock(&self.size) = size;
        to_desktop_size(size)
    }

    async fn updates(&mut self) -> ServerResult<Box<dyn RdpServerDisplayUpdates>> {
        if lock(&self.gfx.live).is_none() {
            return Ok(Box::new(NoUpdates));
        }
        let (tx, rx) = mpsc::unbounded_channel();
        *lock(&self.gfx.pointer) = Some(tx);
        Ok(Box::new(PointerUpdates(rx)))
    }

    fn request_layout(&mut self, layout: DisplayControlMonitorLayout) {
        let primary = layout.monitors().iter().find(|m| m.is_primary()).or_else(|| layout.monitors().first());
        let Some(monitor) = primary else { return };
        let (width, height) = monitor.dimensions();
        let request = LayoutRequest {
            pixels: Size::new(width, height),
            desktop_scale: monitor.desktop_scale_factor(),
            physical_mm: monitor.physical_dimensions(),
        };
        let target = lock(&self.displays).apply(&request);
        tracing::info!(?target, "display for the client layout");
        // A virtual display renders exactly its mode; the physical one is letterboxed by the
        // client, so the desktop keeps the client's (clamped) size.
        let (virtual_id, virtual_size) = match &target {
            DisplayTarget::Virtual { display_id, plan } => (Some(*display_id), Some(plan.pixels)),
            DisplayTarget::Physical => (None, None),
        };
        lock(&self.gfx.stats).display = Some(target);
        if self.gfx.is_live() {
            if let Some(size) = virtual_size {
                // HOST-10: capture the client's virtual display at its mode's size.
                tracing::info!(
                    width = size.width,
                    height = size.height,
                    display_id = ?virtual_id,
                    "client layout; capturing the virtual display"
                );
                lock(&self.gfx.stats).layouts.push(size);
                let gfx = Arc::clone(&self.gfx);
                tokio::spawn(async move { gfx.switch_live(virtual_id) });
                return;
            }
            // The physical screen keeps its size; answer with it.
            let size = *lock(&self.size);
            tracing::info!(width, height, "client layout; keeping the screen's size");
            lock(&self.gfx.stats).layouts.push(size);
            let gfx = Arc::clone(&self.gfx);
            tokio::spawn(async move { gfx.reset_live_surface() });
            return;
        }
        let size = virtual_size.unwrap_or_else(|| clamp_layout(Size::new(width, height)));
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

/// Pointer changes from the pump, as `ironrdp-server` display updates.
struct PointerUpdates(mpsc::UnboundedReceiver<DisplayUpdate>);

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for PointerUpdates {
    async fn next_update(&mut self) -> ServerResult<Option<DisplayUpdate>> {
        match self.0.recv().await {
            Some(update) => Ok(Some(update)),
            None => std::future::pending().await,
        }
    }
}

/// State shared by the EGFX factory, its handlers and the display.
struct GfxShared {
    /// The current connection's authentication: nothing is drawn until it is authorized.
    auth: Arc<Mutex<ConnectionAuth>>,
    sender: Mutex<Option<mpsc::UnboundedSender<ServerEvent>>>,
    handle: Mutex<Option<GfxServerHandle>>,
    source: Mutex<Box<dyn FrameSource>>,
    size: Arc<Mutex<Size<u32>>>,
    stats: SharedStats,
    /// Makes the live picture for each connection (HOST-pipeline).
    live_factory: Mutex<Option<LiveFactory>>,
    /// What the live picture captures and how it is encoded now.
    request: Mutex<LiveRequest>,
    /// This connection's live picture until the pump takes it.
    live: Mutex<Option<Box<dyn LiveSource>>>,
    /// The running pump.
    pump: Mutex<Option<Pump>>,
    /// The live picture's surface.
    surface: Mutex<Option<u16>>,
    /// Where pointer updates go.
    pointer: Mutex<Option<mpsc::UnboundedSender<DisplayUpdate>>>,
    /// The next picture must be a keyframe (a new surface).
    keyframe_wanted: AtomicBool,
    /// The session socket's send-buffer probe (HOST-7).
    probe: Mutex<Option<crate::link::SocketProbe>>,
    /// The last pointer position or hide sent, repeated as the idle heartbeat (HOST-7).
    last_pointer: Mutex<Option<DisplayUpdate>>,
}

impl GfxShared {
    /// Makes this connection's live picture; its size, or `None` without one.
    fn open_live(&self) -> Option<Size<u32>> {
        drop(lock(&self.pump).take());
        let request = *lock(&self.request);
        let factory = lock(&self.live_factory);
        let factory = factory.as_ref()?;
        match factory(request) {
            Ok(source) => {
                let size = source.size();
                *lock(&self.live) = Some(source);
                Some(size)
            }
            Err(e) => {
                tracing::warn!(error = %e, "no live picture; showing the test pattern");
                lock(&self.stats).last_error = Some(e.to_string());
                *lock(&self.live) = None;
                None
            }
        }
    }

    /// Whether this connection shows the live picture.
    fn is_live(&self) -> bool {
        lock(&self.live).is_some() || lock(&self.pump).is_some()
    }

    /// Creates the live picture's surface (after a reset when `resize`), asks for a keyframe and
    /// starts the pump if it is not running.
    fn start_live(self: &Arc<Self>) {
        self.start_live_with(false);
    }

    /// Shows the connection its picture once the graphics channel is ready and the device is
    /// authorized (HOST-8): the live screen when there is one, else the test pattern.
    fn show(self: &Arc<Self>) {
        if !lock(&self.auth).is_authorized() {
            return;
        }
        if lock(&self.live).is_some() {
            self.start_live();
        } else {
            self.present(false);
        }
    }

    fn start_live_with(self: &Arc<Self>, resize: bool) {
        if !lock(&self.auth).is_authorized() || !self.new_live_surface(resize) {
            return;
        }
        // The client's capabilities are known now: AVC444 when it confirmed it and the picture
        // is small enough (`codec::choose`), else AVC420. A source of the other kind is reopened.
        let want_avc444 = self.wants_avc444();
        let reopen = lock(&self.live).as_ref().is_some_and(|s| s.avc444() != want_avc444);
        if reopen {
            lock(&self.request).avc444 = want_avc444;
            tracing::info!(avc444 = want_avc444, "reopening the live picture for the client's codec");
            if self.open_live().is_none() {
                return;
            }
        }
        let Some(source) = lock(&self.live).take() else { return };
        let target: Arc<dyn GfxTarget> = Arc::clone(self) as Arc<dyn GfxTarget>;
        match crate::live::start(source, target) {
            Ok(pump) => *lock(&self.pump) = Some(pump),
            Err(e) => tracing::warn!(error = %e, "could not start the picture pump"),
        }
    }

    /// A client layout on a live desktop: a fresh output and surface at the same size.
    fn reset_live_surface(&self) {
        self.new_live_surface(true);
    }

    /// Whether the client confirmed AVC444 and the picture is small enough for it.
    fn wants_avc444(&self) -> bool {
        let Some(server) = lock(&self.handle).clone() else { return false };
        let (avc420, avc444) = {
            let server = lock(&server);
            (server.supports_avc420(), server.supports_avc444())
        };
        crate::codec::choose(avc420, avc444, *lock(&self.size)) == crate::codec::Codec::Avc444
    }

    /// Moves the live picture to display `display` (HOST-10's virtual display for the client's
    /// layout): stops the pump, reopens capture there, and starts again on a new output and
    /// surface at that display's size.
    fn switch_live(self: &Arc<Self>, display: Option<u32>) {
        drop(lock(&self.pump).take());
        lock(&self.request).display = display;
        let Some(size) = self.open_live() else { return };
        *lock(&self.size) = size;
        self.start_live_with(true);
    }

    fn new_live_surface(&self, resize: bool) -> bool {
        let size = *lock(&self.size);
        let Some(server) = lock(&self.handle).clone() else { return false };
        let (w, h) = (to_desktop_size(size).width, to_desktop_size(size).height);
        let messages = {
            let mut server = lock(&server);
            if !server.is_ready() {
                return false;
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
            let Some(surface) = server.create_surface(w, h) else { return false };
            server.map_surface_to_output(surface, 0, 0);
            *lock(&self.surface) = Some(surface);
            let Some(channel) = server.channel_id() else { return false };
            encode_dvc_messages(channel, server.drain_output(), ChannelFlags::empty())
        };
        self.keyframe_wanted.store(true, Ordering::Relaxed);
        self.forward(messages);
        true
    }

    fn forward(&self, messages: ironrdp_core::EncodeResult<Vec<ironrdp_svc::SvcMessage>>) {
        match (messages, lock(&self.sender).as_ref()) {
            (Ok(messages), Some(tx)) => {
                let _ = tx.send(ServerEvent::Egfx(EgfxServerMessage::SendMessages { messages }));
            }
            (Err(e), _) => tracing::warn!(error = %e, "could not encode graphics messages"),
            (Ok(_), None) => {}
        }
    }

    /// Sends a keyframe of the current desktop on a new surface; with `resize`, resets the
    /// graphics output to the new size first.
    fn present(&self, resize: bool) {
        if !lock(&self.auth).is_authorized() {
            return;
        }
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
            gfx.show();
        });
    }

    fn on_frame_ack(&mut self, _frame_id: u32, _queue_depth: u32, _total_frames_decoded: u32) {
        lock(&self.0.stats).frames_acked += 1;
    }
}

/// Offers `Drift::Mac::v1` (EXT-1) on every connection and keeps the latest link; with pairing,
/// the channel also carries the device's pairing or authentication (HOST-8).
struct DriftMacFactory {
    link: Arc<Mutex<Option<Link>>>,
    pairing: Option<Pairing>,
    host_fingerprint: [u8; 32],
    current: Arc<Mutex<ConnectionAuth>>,
}

impl DvcServerFactory for DriftMacFactory {
    fn attach(&self, dvc: DrdynvcServer) -> DrdynvcServer {
        let link = Link::new();
        *lock(&self.link) = Some(link.clone());
        let channel = match &self.pairing {
            Some(pairing) => {
                let auth = lock(&self.current).clone();
                DriftMacServer::new(link, Capabilities::PAIRING)
                    .with_responder(pairing.responder(self.host_fingerprint, auth))
            }
            None => DriftMacServer::new(link, Capabilities::NONE),
        };
        dvc.with_dynamic_channel(channel)
    }
}

/// Input reaches the sink only while the connection is authorized.
struct GatedInput {
    sink: SinkHandler,
    auth: Arc<Mutex<ConnectionAuth>>,
}

impl ironrdp_server::RdpServerInputHandler for GatedInput {
    fn keyboard(&mut self, event: ironrdp_server::KeyboardEvent) {
        if lock(&self.auth).is_authorized() {
            self.sink.keyboard(event);
        }
    }

    fn mouse(&mut self, event: ironrdp_server::MouseEvent) {
        if lock(&self.auth).is_authorized() {
            self.sink.mouse(event);
        }
    }
}

impl GfxTarget for GfxShared {
    fn backpressure(&self) -> bool {
        let Some(server) = lock(&self.handle).clone() else { return true };
        let server = lock(&server);
        !server.is_ready() || server.should_backpressure()
    }

    fn send(&self, frame: &LiveFrame, timestamp_ms: u32) -> Option<bool> {
        let server = lock(&self.handle).clone()?;
        let surface = (*lock(&self.surface))?;
        let messages = {
            let mut server = lock(&server);
            if !server.is_ready() {
                return Some(false);
            }
            let sent = match &frame.chroma {
                Some(chroma) => server.send_avc444_frame(
                    surface,
                    &frame.au,
                    &frame.regions,
                    Some(chroma),
                    Some(&frame.regions),
                    timestamp_ms,
                ),
                None => server.send_avc420_frame(surface, &frame.au, &frame.regions, timestamp_ms),
            };
            if sent.is_none() {
                return Some(false);
            }
            let mut stats = lock(&self.stats);
            stats.frames_sent += 1;
            if frame.chroma.is_some() {
                stats.avc444_frames += 1;
            }
            drop(stats);
            let channel = server.channel_id()?;
            encode_dvc_messages(channel, server.drain_output(), ChannelFlags::empty())
        };
        self.forward(messages);
        Some(true)
    }

    fn pointer(&self, update: DisplayUpdate) {
        if matches!(update, DisplayUpdate::PointerPosition(_) | DisplayUpdate::HidePointer) {
            *lock(&self.last_pointer) = Some(update.clone());
        }
        if let Some(tx) = lock(&self.pointer).as_ref() {
            let _ = tx.send(update);
        }
    }

    fn sample(&self) -> crate::link::BacklogSample {
        let frames_in_flight = lock(&self.handle).clone().map_or(0, |s| lock(&s).frames_in_flight());
        let unsent_bytes = lock(&self.probe).as_ref().and_then(|p| p.unsent_bytes().ok()).unwrap_or(0);
        crate::link::BacklogSample { frames_in_flight, unsent_bytes }
    }

    fn report_backlog(&self, level: crate::link::Backlog) {
        tracing::info!(?level, "backlog");
        lock(&self.stats).backlog = level;
    }

    /// Repeats the last pointer position (or hide): a few bytes, and no visible change on any
    /// client — Drift never warps its pointer, and others move it to where it already is.
    fn heartbeat(&self) -> bool {
        let Some(update) = lock(&self.last_pointer).clone() else { return false };
        let Some(tx) = lock(&self.pointer).clone() else { return false };
        if tx.send(update).is_err() {
            return false;
        }
        lock(&self.stats).heartbeats += 1;
        true
    }

    fn take_keyframe_request(&self) -> bool {
        self.keyframe_wanted.swap(false, Ordering::Relaxed)
    }
}
