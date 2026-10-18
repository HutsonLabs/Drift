//! `FakeServer`: a scripted RDP server on `127.0.0.1` for loopback tests (plan §0, M1-1, M3-1).
//!
//! It is built on IronRDP's server-side acceptor (the engine behind `ironrdp-server`) plus
//! scripted PDUs, and reproduces the g-r-d behaviour verified in plan §1.2–§1.3:
//!
//! - **NLA legs** select `PROTOCOL_HYBRID` and run CredSSP/NTLM against a configured
//!   user name and password (Headless / Desktop Sharing / Remote Login leg 1).
//! - **RDSTLS legs** select `PROTOCOL_RDSTLS` (0x4), send the capabilities
//!   `01 00 01 00 01 00 03 00`, record the client's authentication request and answer with a
//!   configurable result code (Remote Login legs ≥ 2, `0x52E` for reused credentials).
//! - Each leg presents its own TLS certificate ([`TestCert`]), so tests can exercise pinning,
//!   TOFU prompts and the redirect target-certificate check.
//! - After activation a leg runs a list of [`ServerAction`]s, e.g. injecting an Enhanced
//!   Security **Server Redirection PDU** (share control `pduType` 0xA).
//! - A **stall** leg accepts TCP and never answers (connect timeouts).
//!
//! One `FakeServer` listens on one port and serves its [`LegScript`]s to consecutive TCP
//! connections, exactly like g-r-d's system daemon, which receives all three Remote Login legs
//! on `:3389` (no `TargetNetAddress` is sent). Everything the client sent is recorded in a
//! [`FakeServerLog`] for assertions.

use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use drift_core::CertFingerprint;
use ironrdp_acceptor::{Acceptor, DesktopSize};
use ironrdp_async::{Framed, FramedWrite as _, NetworkClient};
use ironrdp_cliprdr::CliprdrServer;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, OwnedFormatDataResponse,
};
use ironrdp_connector::sspi::generator::NetworkRequest;
use ironrdp_connector::{ConnectorResult, Sequence as _, ServerName, general_err};
use ironrdp_core::{WriteBuf, decode, encode_vec};
use ironrdp_dvc::{DrdynvcServer, DvcMessage, encode_dvc_messages};
use ironrdp_pdu::input::fast_path::{FastPathInput, FastPathInputEvent};
use ironrdp_pdu::nego::{self, SecurityProtocol};
use ironrdp_pdu::rdp::capability_sets::{self as caps, CapabilitySet};
use ironrdp_pdu::rdp::client_info::{CompressionType, Credentials};
use ironrdp_pdu::rdp::headers::{
    CompressionFlags, ShareControlHeader, ShareControlPdu, ShareDataHeader, ShareDataPdu, StreamPriority,
};
use ironrdp_pdu::rdp::server_redirection::ServerRedirectionPdu;
use ironrdp_pdu::x224::{X224, X224Data};
use ironrdp_pdu::{gcc, mcs};
use ironrdp_svc::{ChannelFlags, StaticChannelSet, SvcMessage, server_encode_svc_messages};
use ironrdp_tokio::TokioStream;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::fake_channels::{
    self, Channels, ClipEvent, FakeClipboardBackend, FakeDisplayControl, FakeGraphics, RecordedLayout,
    ServerClipFormat, SharedState, redraw_pdus,
};

/// The RDSTLS capabilities PDU g-r-d sends (versions v1|v2), plan §1.3.
pub const RDSTLS_CAPABILITIES: [u8; 8] = [0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x03, 0x00];

/// A self-signed TLS identity for one fake server leg.
#[derive(Clone)]
pub struct TestCert {
    der: Vec<u8>,
    key_pkcs8: Vec<u8>,
    public_key: Vec<u8>,
}

impl std::fmt::Debug for TestCert {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestCert").field("fingerprint", &self.fingerprint().to_string()).finish()
    }
}

impl TestCert {
    /// Generates a fresh self-signed certificate for `name` (a DNS name or IP literal).
    ///
    /// # Panics
    /// If key generation fails (test-only helper).
    pub fn generate(name: &str) -> Self {
        #[expect(clippy::expect_used, reason = "test helper; rcgen only fails on invalid names")]
        let certified =
            rcgen::generate_simple_self_signed(vec![name.to_owned()]).expect("rcgen self-signed cert");
        Self {
            der: certified.cert.der().to_vec(),
            key_pkcs8: certified.signing_key.serialize_der(),
            public_key: certified.signing_key.public_key_raw().to_vec(),
        }
    }

    /// The certificate DER (what the client sees as the TLS leaf).
    pub fn der(&self) -> &[u8] {
        &self.der
    }

    /// SHA-256 of the DER, as pinned by Drift.
    pub fn fingerprint(&self) -> CertFingerprint {
        CertFingerprint::of_der(&self.der)
    }

    /// The private key, PKCS#8 DER (test-only identity; for hosts built in tests).
    pub fn key_pkcs8(&self) -> &[u8] {
        &self.key_pkcs8
    }

    /// The raw `subjectPublicKey` bits (what CredSSP binds to).
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    pub(crate) fn acceptor(&self) -> std::result::Result<TlsAcceptor, rustls::Error> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_pkcs8.clone()));
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(self.der.clone())], key)?;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

/// How a leg authenticates the client.
#[derive(Clone)]
pub enum LegAuth {
    /// `PROTOCOL_HYBRID`: CredSSP/NTLM against these credentials.
    Nla {
        /// Expected user name.
        username: String,
        /// Expected password.
        password: String,
    },
    /// `PROTOCOL_RDSTLS`: record the AuthRequest and answer with `result` (0 = success).
    Rdstls {
        /// RDSTLS AuthResponse result code.
        result: u32,
    },
}

impl std::fmt::Debug for LegAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nla { username, .. } => {
                f.debug_struct("Nla").field("username", username).finish_non_exhaustive()
            }
            Self::Rdstls { result } => {
                f.debug_struct("Rdstls").field("result", &format_args!("{result:#x}")).finish()
            }
        }
    }
}

/// A scripted server action after the client is activated.
#[derive(Debug, Clone)]
pub enum ServerAction {
    /// Pause (real time).
    Wait(Duration),
    /// Send an Enhanced Security Server Redirection PDU on the I/O channel.
    Redirect(Box<ServerRedirectionPdu>),
    /// Close the TCP connection.
    Close,
    /// Remote copy: announce these formats on CLIPRDR and serve their data on request.
    ClipboardCopy(Vec<ServerClipFormat>),
    /// Ask the client for its clipboard data in this format id (a remote paste).
    ClipboardPaste(u32),
    /// While `true`, the client's Format Data Requests are recorded but answered only when
    /// this is set back to `false` (newest-wins race tests).
    HoldClipboardResponses(bool),
    /// Send `ResetGraphics` with a new surface of this size on the graphics channel.
    GfxReset(u32, u32),
    /// Fill `rect` (`left, top, right, bottom`) of the newest surface with an RGB colour, in
    /// one frame (a moving picture for the iOS first-light host, IOS-5).
    GfxFill {
        /// `left, top, right, bottom` in surface pixels (right/bottom exclusive).
        rect: (u16, u16, u16, u16),
        /// `r, g, b`.
        color: [u8; 3],
    },
    /// Send these bytes verbatim on the graphics channel: a damaged `RDP_SEGMENTED_DATA`
    /// frame, which the client must reject as `ProtocolError` (M9-2).
    GfxRaw(Vec<u8>),
}

/// What one TCP connection to the fake server does.
#[derive(Debug, Clone)]
pub struct LegScript {
    /// TLS identity presented on this leg.
    pub cert: TestCert,
    /// Authentication method.
    pub auth: LegAuth,
    /// Desktop size announced in the Demand Active.
    pub desktop: (u16, u16),
    /// Actions after activation; afterwards the leg keeps reading until the client leaves.
    pub actions: Vec<ServerAction>,
    /// Accept the TCP connection but never answer (connect timeout tests).
    pub stall: bool,
    /// Virtual channels offered after activation (none by default).
    pub channels: Channels,
}

impl LegScript {
    /// An NLA leg (1280×800) with `cert` and the given credentials.
    pub fn nla(cert: TestCert, username: &str, password: &str) -> Self {
        Self {
            cert,
            auth: LegAuth::Nla { username: username.to_owned(), password: password.to_owned() },
            desktop: (1280, 800),
            actions: Vec::new(),
            stall: false,
            channels: Channels::default(),
        }
    }

    /// An RDSTLS leg (1280×800) answering with `result`.
    pub fn rdstls(cert: TestCert, result: u32) -> Self {
        Self {
            cert,
            auth: LegAuth::Rdstls { result },
            desktop: (1280, 800),
            actions: Vec::new(),
            stall: false,
            channels: Channels::default(),
        }
    }

    /// A leg that accepts TCP and then stays silent.
    pub fn stall(cert: TestCert) -> Self {
        Self { stall: true, ..Self::rdstls(cert, 0) }
    }

    /// Offers `channels` after activation.
    #[must_use]
    pub fn with_channels(mut self, channels: Channels) -> Self {
        self.channels = channels;
        self
    }

    /// Appends a post-activation action.
    #[must_use]
    pub fn then(mut self, action: ServerAction) -> Self {
        self.actions.push(action);
        self
    }
}

/// The RDSTLS AuthRequest a client sent (one-time credentials; test data only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RdstlsRequest {
    /// `RedirectionGuid`, verbatim.
    pub redirection_guid: Vec<u8>,
    /// User name (UTF-16LE on the wire, NUL stripped).
    pub username: String,
    /// Domain.
    pub domain: String,
    /// Password blob, verbatim.
    pub password: Vec<u8>,
}

/// Everything recorded for one TCP connection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegRecord {
    /// `requestedProtocols` of the X.224 Connection Request.
    pub requested_protocols: u32,
    /// Protocol the server selected.
    pub selected_protocol: u32,
    /// Routing token / cookie text of the X.224 Connection Request (without CRLF).
    pub routing_token: Option<String>,
    /// RDSTLS AuthRequest, if this was an RDSTLS leg.
    pub rdstls_request: Option<RdstlsRequest>,
    /// GCC client name.
    pub client_name: Option<String>,
    /// Desktop size requested in the GCC Client Core Data.
    pub client_desktop: Option<(u16, u16)>,
    /// `desktopScaleFactor` of the GCC Client Core Data.
    pub client_desktop_scale: Option<u32>,
    /// `RNS_UD_CS_SUPPORT_DYNVC_GFX_PROTOCOL` was set in the client core data.
    pub gfx_protocol_advertised: bool,
    /// Major platform type from the client's General capability set (`Debug` name, e.g. `MACINTOSH`).
    pub platform: Option<String>,
    /// The client reached the active state.
    pub activated: bool,
    /// Actions of the script that were executed.
    pub actions_done: usize,
    /// The client sent a Shutdown Request (graceful close, M3-4).
    pub shutdown_requested: bool,
    /// Fast-path input PDUs received after activation.
    pub fast_path_inputs: usize,
    /// Every fast-path input event received, in order (M2-4 loopback).
    pub fast_path_events: Vec<FastPathInputEvent>,
    /// Suppress Output PDUs in order: `None` = `allowDisplayUpdates=0`, `Some([l, t, r, b])` =
    /// allow with that inclusive desktop rectangle (M6-3).
    pub suppress_output: Vec<Option<[u16; 4]>>,
    /// Monitor layouts received on Display Control (M4-2).
    pub monitor_layouts: Vec<RecordedLayout>,
    /// The client opened the graphics pipeline and advertised its caps.
    pub gfx_caps_advertised: bool,
    /// `ResetGraphics` sizes the server sent.
    pub gfx_resets_sent: Vec<(u32, u32)>,
    /// `FrameAcknowledge` frame ids received.
    pub gfx_frame_acks: Vec<u32>,
    /// What the server end of `Drift::Mac::v1` negotiated, if the leg offered it (EXT-1).
    pub drift_mac_negotiated: Option<drift_ext::Negotiated>,
    /// `FrameAcknowledge`s with `SUSPEND_FRAME_ACKNOWLEDGEMENT`.
    pub gfx_suspend_acks: u32,
    /// CLIPRDR reached the ready state (the client answered the initial format-list request).
    pub clipboard_ready: bool,
    /// Format lists the client sent: `(id, name)` per format.
    pub client_format_lists: Vec<Vec<(u32, Option<String>)>>,
    /// Format ids the client requested from the server (eager fetches of remote copies).
    pub client_data_requests: Vec<u32>,
    /// The client's answers to server paste requests (`None` = error response).
    pub client_data_responses: Vec<Option<Vec<u8>>>,
    /// First error on this leg (e.g. failed NLA), for diagnostics.
    pub error: Option<String>,
}

/// Recorded activity of a fake server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeServerLog {
    /// Legs in connection order.
    pub legs: Vec<LegRecord>,
}

/// A scripted RDP server listening on `127.0.0.1:<ephemeral>`.
pub struct FakeServer {
    addr: SocketAddr,
    log: Arc<Mutex<FakeServerLog>>,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for FakeServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeServer").field("addr", &self.addr).finish_non_exhaustive()
    }
}

impl FakeServer {
    /// Starts serving `legs` to consecutive connections. Connections beyond the script are
    /// accepted, recorded and closed immediately.
    pub async fn start(legs: Vec<LegScript>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let log = Arc::new(Mutex::new(FakeServerLog::default()));
        let task_log = Arc::clone(&log);
        let task = tokio::spawn(async move {
            let mut legs = legs.into_iter();
            let mut index = 0;
            while let Ok((stream, _)) = listener.accept().await {
                lock(&task_log).legs.push(LegRecord::default());
                match legs.next() {
                    Some(script) => {
                        let log = Arc::clone(&task_log);
                        tokio::spawn(async move {
                            if let Err(e) = serve_leg(stream, script, &log, index).await {
                                let mut log = lock(&log);
                                if let Some(rec) = log.legs.get_mut(index)
                                    && rec.error.is_none()
                                {
                                    rec.error = Some(e);
                                }
                            }
                        });
                    }
                    None => drop(stream),
                }
                index += 1;
            }
        });
        Ok(Self { addr, log, task })
    }

    /// The listening address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The listening port.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// A snapshot of what was recorded so far.
    pub fn log(&self) -> FakeServerLog {
        lock(&self.log).clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn lock(log: &Mutex<FakeServerLog>) -> std::sync::MutexGuard<'_, FakeServerLog> {
    log.lock().unwrap_or_else(PoisonError::into_inner)
}

fn update(log: &Mutex<FakeServerLog>, index: usize, f: impl FnOnce(&mut LegRecord)) {
    if let Some(rec) = lock(log).legs.get_mut(index) {
        f(rec);
    }
}

type Result<T> = std::result::Result<T, String>;

fn err(context: &str, e: impl std::fmt::Display) -> String {
    format!("{context}: {e}")
}

async fn serve_leg(
    mut tcp: TcpStream,
    script: LegScript,
    log: &Mutex<FakeServerLog>,
    index: usize,
) -> Result<()> {
    if script.stall {
        let mut sink = [0u8; 1024];
        while tcp.read(&mut sink).await.map_err(|e| err("stall read", e))? > 0 {}
        return Ok(());
    }

    // X.224 negotiation, done by hand so the leg can select RDSTLS and record the token.
    let request = read_tpkt(&mut tcp).await?;
    let X224(connection_request) =
        decode::<X224<nego::ConnectionRequest>>(&request).map_err(|e| err("decode connection request", e))?;
    let routing_token = connection_request.nego_data.as_ref().map(|d| match d {
        nego::NegoRequestData::RoutingToken(t) => format!("Cookie: msts={}", t.0),
        nego::NegoRequestData::Cookie(c) => format!("Cookie: mstshash={}", c.0),
    });
    let requested = connection_request.protocol;
    let selected = match &script.auth {
        LegAuth::Nla { .. } => SecurityProtocol::HYBRID,
        LegAuth::Rdstls { .. } => SecurityProtocol::RDSTLS,
    };
    update(log, index, |r| {
        r.requested_protocols = requested.bits();
        r.selected_protocol = selected.bits();
        r.routing_token = routing_token;
    });
    if !requested.contains(selected) {
        let failure = nego::ConnectionConfirm::Failure { code: nego::FailureCode::HYBRID_REQUIRED_BY_SERVER };
        let bytes = encode_vec(&X224(failure)).map_err(|e| err("encode failure", e))?;
        tcp.write_all(&bytes).await.map_err(|e| err("write failure", e))?;
        return Err(format!("client did not request {selected:?} (got {requested:?})"));
    }
    let confirm = nego::ConnectionConfirm::Response {
        flags: nego::ResponseFlags::EXTENDED_CLIENT_DATA_SUPPORTED,
        protocol: selected,
    };
    let bytes = encode_vec(&X224(confirm)).map_err(|e| err("encode confirm", e))?;
    tcp.write_all(&bytes).await.map_err(|e| err("write confirm", e))?;

    // TLS.
    let tls_acceptor = script.cert.acceptor().map_err(|e| err("tls config", e))?;
    let mut tls = tls_acceptor.accept(tcp).await.map_err(|e| err("tls accept", e))?;

    // An acceptor positioned right after the security upgrade: feed it a synthetic
    // Connection Request for the protocol it knows (it cannot select RDSTLS itself).
    let (acceptor_protocol, creds) = match &script.auth {
        LegAuth::Nla { username, password } => (
            SecurityProtocol::HYBRID,
            Some(Credentials { username: username.clone(), password: password.clone(), domain: None }),
        ),
        LegAuth::Rdstls { .. } => (SecurityProtocol::SSL, None),
    };
    let desktop = DesktopSize { width: script.desktop.0, height: script.desktop.1 };
    let mut acceptor = Acceptor::new(acceptor_protocol, desktop, server_capabilities(desktop), creds);
    let state: SharedState = Arc::default();
    attach_channels(&mut acceptor, script.channels, &state, script.desktop);
    prime_acceptor(&mut acceptor, acceptor_protocol)?;

    if let LegAuth::Rdstls { result } = &script.auth {
        tls.write_all(&RDSTLS_CAPABILITIES).await.map_err(|e| err("write rdstls caps", e))?;
        let req = read_rdstls_auth_request(&mut tls).await?;
        update(log, index, |r| r.rdstls_request = Some(req));
        let mut rsp = vec![0x01, 0x00, 0x04, 0x00, 0x01, 0x00];
        rsp.extend_from_slice(&result.to_le_bytes());
        tls.write_all(&rsp).await.map_err(|e| err("write rdstls response", e))?;
        if *result != 0 {
            let _ = tls.shutdown().await;
            return Ok(());
        }
    }

    let mut framed = Framed::<TokioStream<_>>::new(tls);
    if acceptor.should_perform_credssp() {
        ironrdp_acceptor::accept_credssp(
            &mut framed,
            &mut acceptor,
            &mut NoNetwork,
            ServerName::new("drift-client"),
            script.cert.public_key.clone(),
            None,
        )
        .await
        .map_err(|e| err("credssp", e))?;
    }

    // Capability exchange and finalization, recording the client's GCC data on the way.
    let mut buf = WriteBuf::new();
    let result = loop {
        if let Some(result) = acceptor.get_result() {
            break result;
        }
        buf.clear();
        let written = match acceptor.next_pdu_hint() {
            Some(hint) => {
                let pdu = framed.read_by_hint(hint).await.map_err(|e| err("read", e))?;
                record_connect_initial(&pdu, log, index);
                acceptor.step(&pdu, None, &mut buf).map_err(|e| err("accept step", e))?
            }
            None => acceptor.step(&[], None, &mut buf).map_err(|e| err("accept step", e))?,
        };
        if let Some(len) = written.size() {
            framed.write_all(&buf[..len]).await.map_err(|e| err("write", e))?;
        }
    };
    let platform = result.capabilities.iter().find_map(|c| match c {
        CapabilitySet::General(g) => Some(format!("{:?}", g.major_platform_type)),
        _ => None,
    });
    update(log, index, |r| {
        r.activated = true;
        r.platform = platform;
    });

    let mut active = Active {
        channels: result.static_channels,
        user_channel_id: result.user_channel_id,
        io_channel_id: result.io_channel_id,
        state,
        clip_formats: Vec::new(),
        hold_clipboard: false,
        held_requests: Vec::new(),
        log,
        index,
    };
    active.start_channels(&mut framed).await?;

    let mut actions = script.actions.into_iter().peekable();
    let mut wait_until: Option<tokio::time::Instant> = None;
    loop {
        // Run every action that is due.
        while wait_until.is_none_or(|t| tokio::time::Instant::now() >= t) {
            let Some(action) = actions.next() else { break };
            wait_until = None;
            match action {
                ServerAction::Wait(d) => wait_until = Some(tokio::time::Instant::now() + d),
                ServerAction::Redirect(pdu) => {
                    let frame = redirection_frame(&pdu, active.user_channel_id, active.io_channel_id)?;
                    framed.write_all(&frame).await.map_err(|e| err("write redirect", e))?;
                }
                ServerAction::Close => {
                    update(log, index, |r| r.actions_done += 1);
                    return Ok(());
                }
                ServerAction::ClipboardCopy(formats) => active.clipboard_copy(&mut framed, formats).await?,
                ServerAction::ClipboardPaste(id) => active.clipboard_paste(&mut framed, id).await?,
                ServerAction::HoldClipboardResponses(hold) => {
                    active.hold_clipboard = hold;
                    if !hold {
                        for id in std::mem::take(&mut active.held_requests) {
                            active.answer_data_request(&mut framed, id).await?;
                        }
                    }
                }
                ServerAction::GfxReset(w, h) => active.gfx_reset(&mut framed, w, h).await?,
                ServerAction::GfxFill { rect, color } => {
                    let message =
                        fake_channels::gfx_message(&fake_channels::fill_pdus(&active.state, rect, color))
                            .map_err(|e| err("encode gfx", e))?;
                    active.gfx_send(&mut framed, message).await?;
                }
                ServerAction::GfxRaw(bytes) => {
                    active.gfx_send(&mut framed, fake_channels::gfx_raw_message(bytes)).await?
                }
            }
            update(log, index, |r| r.actions_done += 1);
        }
        let sleep_until = wait_until.filter(|_| actions.peek().is_some());
        let frame = tokio::select! {
            res = framed.read_pdu() => res,
            () = async {
                match sleep_until {
                    Some(t) => tokio::time::sleep_until(t).await,
                    None => std::future::pending().await,
                }
            } => continue,
        };
        let Ok((action, frame)) = frame else { return Ok(()) };
        if action == ironrdp_pdu::Action::FastPath {
            let events =
                decode::<FastPathInput>(&frame).map(|i| i.input_events().to_vec()).unwrap_or_default();
            update(log, index, |r| {
                r.fast_path_inputs += 1;
                r.fast_path_events.extend(events);
            });
            continue;
        }
        if active.handle_x224(&mut framed, &frame).await? {
            return Ok(());
        }
        active.after_channel_io(&mut framed).await?;
    }
}

type ServerFramed = Framed<TokioStream<tokio_rustls::server::TlsStream<TcpStream>>>;

async fn send_svc(
    framed: &mut ServerFramed,
    user_channel_id: u16,
    channel_id: u16,
    messages: Vec<SvcMessage>,
) -> Result<()> {
    if messages.is_empty() {
        return Ok(());
    }
    let bytes = server_encode_svc_messages(messages, channel_id, user_channel_id)
        .map_err(|e| err("encode svc", e))?;
    framed.write_all(&bytes).await.map_err(|e| err("write svc", e))
}

/// Attaches the scripted virtual channels to the acceptor.
fn attach_channels(acceptor: &mut Acceptor, channels: Channels, state: &SharedState, desktop: (u16, u16)) {
    if channels.any_dynamic() {
        let mut dvc = DrdynvcServer::new();
        if channels.display_control {
            dvc = dvc.with_dynamic_channel(FakeDisplayControl { state: Arc::clone(state) });
        }
        if channels.drift_mac {
            let link = drift_ext::channel::Link::new();
            fake_channels::lock(state).drift_mac = Some(link.clone());
            dvc = dvc.with_dynamic_channel(drift_ext::channel::DriftMacServer::new(
                link,
                drift_ext::wire::Capabilities::SCROLL
                    | drift_ext::wire::Capabilities::STATUS
                    | drift_ext::wire::Capabilities::PAIRING,
            ));
        }
        if channels.gfx {
            dvc = dvc.with_dynamic_channel(FakeGraphics {
                state: Arc::clone(state),
                desktop: (u32::from(desktop.0), u32::from(desktop.1)),
            });
        }
        acceptor.attach_static_channel(dvc);
    }
    if channels.clipboard {
        acceptor.attach_static_channel(CliprdrServer::new(Box::new(FakeClipboardBackend {
            state: Arc::clone(state),
        })));
    }
}

/// An activated leg: static channels and scripted clipboard contents.
struct Active<'a> {
    channels: StaticChannelSet,
    user_channel_id: u16,
    io_channel_id: u16,
    state: SharedState,
    clip_formats: Vec<ServerClipFormat>,
    hold_clipboard: bool,
    held_requests: Vec<u32>,
    log: &'a Mutex<FakeServerLog>,
    index: usize,
}

impl Active<'_> {
    async fn start_channels(&mut self, framed: &mut ServerFramed) -> Result<()> {
        let mut out = Vec::new();
        for (_, channel, id) in self.channels.iter_mut() {
            if let Some(id) = id {
                out.push((id, channel.start().map_err(|e| err("svc start", e))?));
            }
        }
        for (id, messages) in out {
            send_svc(framed, self.user_channel_id, id, messages).await?;
        }
        Ok(())
    }

    /// Handles a slow-path frame; `true` when the client disconnected.
    async fn handle_x224(&mut self, framed: &mut ServerFramed, frame: &[u8]) -> Result<bool> {
        let Ok(X224(message)) = decode::<X224<mcs::McsMessage<'_>>>(frame) else { return Ok(false) };
        let data = match message {
            mcs::McsMessage::SendDataRequest(data) => data,
            mcs::McsMessage::DisconnectProviderUltimatum(_) => return Ok(true),
            _ => return Ok(false),
        };
        if data.channel_id == self.io_channel_id {
            let Ok(header) = decode::<ShareControlHeader>(data.user_data.as_ref()) else { return Ok(false) };
            let ShareControlPdu::Data(ShareDataHeader { share_data_pdu, .. }) = header.share_control_pdu
            else {
                return Ok(false);
            };
            match share_data_pdu {
                ShareDataPdu::ShutdownRequest => {
                    update(self.log, self.index, |r| r.shutdown_requested = true);
                    let denied = share_data_frame(
                        ShareDataPdu::ShutdownDenied,
                        self.user_channel_id,
                        self.io_channel_id,
                    )?;
                    framed.write_all(&denied).await.map_err(|e| err("write shutdown denied", e))?;
                }
                ShareDataPdu::SuppressOutput(pdu) => {
                    let rect = pdu.desktop_rect.map(|r| [r.left, r.top, r.right, r.bottom]);
                    update(self.log, self.index, |r| r.suppress_output.push(rect));
                }
                _ => {}
            }
            return Ok(false);
        }
        let channel_id = data.channel_id;
        let responses = match self.channels.get_by_channel_id_mut(channel_id) {
            Some(svc) => svc.process(data.user_data.as_ref()).map_err(|e| err("svc process", e))?,
            None => return Ok(false),
        };
        send_svc(framed, self.user_channel_id, channel_id, responses).await?;
        Ok(false)
    }

    /// Mirrors channel state into the log and reacts to layouts and clipboard callbacks.
    async fn after_channel_io(&mut self, framed: &mut ServerFramed) -> Result<()> {
        let (resets, events) = {
            let mut s = fake_channels::lock(&self.state);
            let resets = std::mem::take(&mut s.pending_resets);
            let events = std::mem::take(&mut s.clip_events);
            update(self.log, self.index, |r| {
                r.monitor_layouts.clone_from(&s.layouts);
                r.gfx_caps_advertised = s.gfx_caps_advertised;
                r.gfx_frame_acks.clone_from(&s.gfx_frame_acks);
                r.drift_mac_negotiated = s.drift_mac.as_ref().and_then(drift_ext::channel::Link::negotiated);
                r.gfx_suspend_acks = s.gfx_suspend_acks;
                r.gfx_resets_sent.clone_from(&s.gfx_resets_sent);
            });
            (resets, events)
        };
        for (w, h) in resets {
            self.gfx_reset(framed, w, h).await?;
        }
        for event in events {
            match event {
                ClipEvent::Ready => update(self.log, self.index, |r| r.clipboard_ready = true),
                ClipEvent::ClientFormatList(list) => {
                    update(self.log, self.index, |r| r.client_format_lists.push(list));
                }
                ClipEvent::DataRequest(id) => {
                    update(self.log, self.index, |r| r.client_data_requests.push(id));
                    if self.hold_clipboard {
                        self.held_requests.push(id);
                    } else {
                        self.answer_data_request(framed, id).await?;
                    }
                }
                ClipEvent::DataResponse(data) => {
                    update(self.log, self.index, |r| r.client_data_responses.push(data));
                }
            }
        }
        Ok(())
    }

    fn cliprdr(&mut self) -> Result<(&mut CliprdrServer, u16)> {
        let id = self.channels.get_channel_id_by_type::<CliprdrServer>().ok_or("no CLIPRDR channel")?;
        let cliprdr = self
            .channels
            .get_by_type_mut::<CliprdrServer>()
            .and_then(|c| c.channel_processor_downcast_mut::<CliprdrServer>())
            .ok_or("no CLIPRDR channel")?;
        Ok((cliprdr, id))
    }

    async fn clipboard_copy(
        &mut self,
        framed: &mut ServerFramed,
        formats: Vec<ServerClipFormat>,
    ) -> Result<()> {
        let list: Vec<ClipboardFormat> = formats
            .iter()
            .map(|f| {
                let format = ClipboardFormat::new(ClipboardFormatId(f.id));
                match &f.name {
                    Some(name) => format.with_name(ClipboardFormatName::new(name.clone())),
                    None => format,
                }
            })
            .collect();
        self.clip_formats = formats;
        let (cliprdr, id) = self.cliprdr()?;
        let messages = cliprdr.initiate_copy(&list).map_err(|e| err("initiate copy", e))?;
        send_svc(framed, self.user_channel_id, id, messages.into()).await
    }

    async fn clipboard_paste(&mut self, framed: &mut ServerFramed, format_id: u32) -> Result<()> {
        let (cliprdr, id) = self.cliprdr()?;
        let messages =
            cliprdr.initiate_paste(ClipboardFormatId(format_id)).map_err(|e| err("initiate paste", e))?;
        send_svc(framed, self.user_channel_id, id, messages.into()).await
    }

    async fn answer_data_request(&mut self, framed: &mut ServerFramed, format_id: u32) -> Result<()> {
        let response = match self.clip_formats.iter().find(|f| f.id == format_id) {
            Some(f) => OwnedFormatDataResponse::new_data(f.data.clone()),
            None => OwnedFormatDataResponse::new_error(),
        };
        let (cliprdr, id) = self.cliprdr()?;
        let messages = cliprdr.submit_format_data(response).map_err(|e| err("submit format data", e))?;
        send_svc(framed, self.user_channel_id, id, messages.into()).await
    }

    async fn gfx_reset(&mut self, framed: &mut ServerFramed, width: u32, height: u32) -> Result<()> {
        let message = fake_channels::gfx_message(&redraw_pdus(&self.state, width, height))
            .map_err(|e| err("encode gfx", e))?;
        self.gfx_send(framed, message).await?;
        let sent = fake_channels::lock(&self.state).gfx_resets_sent.clone();
        update(self.log, self.index, |r| r.gfx_resets_sent = sent);
        Ok(())
    }

    /// Sends one message on the graphics channel; a no-op while the channel is not open.
    async fn gfx_send(&mut self, framed: &mut ServerFramed, message: DvcMessage) -> Result<()> {
        let Some(drdynvc_id) = self.channels.get_channel_id_by_type::<DrdynvcServer>() else { return Ok(()) };
        let Some(drdynvc) = self
            .channels
            .get_by_type_mut::<DrdynvcServer>()
            .and_then(|c| c.channel_processor_downcast_mut::<DrdynvcServer>())
        else {
            return Ok(());
        };
        let Some(gfx_id) =
            drdynvc.get_channel_id_by_type::<FakeGraphics>().filter(|id| drdynvc.is_channel_opened(*id))
        else {
            return Ok(());
        };
        let messages = encode_dvc_messages(gfx_id, vec![message], ChannelFlags::empty())
            .map_err(|e| err("encode dvc", e))?;
        send_svc(framed, self.user_channel_id, drdynvc_id, messages).await
    }
}

fn share_data_frame(pdu: ShareDataPdu, user_channel_id: u16, io_channel_id: u16) -> Result<Vec<u8>> {
    let header = ShareControlHeader {
        share_control_pdu: ShareControlPdu::Data(ShareDataHeader {
            share_data_pdu: pdu,
            stream_priority: StreamPriority::Undefined,
            compression_flags: CompressionFlags::empty(),
            compression_type: CompressionType::K8,
        }),
        pdu_source: io_channel_id,
        share_id: 0,
    };
    let user_data = encode_vec(&header).map_err(|e| err("encode share data", e))?;
    let sdi = mcs::SendDataIndication {
        initiator_id: user_channel_id,
        channel_id: io_channel_id,
        user_data: Cow::Owned(user_data),
    };
    encode_vec(&X224(sdi)).map_err(|e| err("encode send data indication", e))
}

/// Encodes a Server Redirection PDU as the X.224/MCS frame g-r-d sends (plan §1.3).
pub fn redirection_frame(
    pdu: &ServerRedirectionPdu,
    user_channel_id: u16,
    io_channel_id: u16,
) -> Result<Vec<u8>> {
    let header = ShareControlHeader {
        share_control_pdu: ShareControlPdu::ServerRedirect(pdu.clone()),
        pdu_source: io_channel_id,
        share_id: 0,
    };
    let user_data = encode_vec(&header).map_err(|e| err("encode redirect", e))?;
    let sdi = mcs::SendDataIndication {
        initiator_id: user_channel_id,
        channel_id: io_channel_id,
        user_data: Cow::Owned(user_data),
    };
    encode_vec(&X224(sdi)).map_err(|e| err("encode send data indication", e))
}

fn prime_acceptor(acceptor: &mut Acceptor, protocol: SecurityProtocol) -> Result<()> {
    let request = nego::ConnectionRequest {
        nego_data: None,
        flags: nego::RequestFlags::empty(),
        protocol,
        correlation_info: None,
    };
    let bytes = encode_vec(&X224(request)).map_err(|e| err("encode synthetic request", e))?;
    let mut discard = WriteBuf::new();
    acceptor.step(&bytes, None, &mut discard).map_err(|e| err("prime request", e))?;
    acceptor.step(&[], None, &mut discard).map_err(|e| err("prime confirm", e))?;
    if acceptor.reached_security_upgrade().is_none() {
        return Err("acceptor did not reach the security upgrade".into());
    }
    acceptor.mark_security_upgrade_as_done();
    Ok(())
}

fn record_connect_initial(pdu: &[u8], log: &Mutex<FakeServerLog>, index: usize) {
    let Ok(X224(data)) = decode::<X224<X224Data<'_>>>(pdu) else { return };
    let Ok(initial) = decode::<mcs::ConnectInitial>(data.data.as_ref()) else { return };
    let blocks = initial.conference_create_request.into_gcc_blocks();
    let gfx = blocks
        .core
        .optional_data
        .early_capability_flags
        .is_some_and(|f| f.contains(gcc::ClientEarlyCapabilityFlags::SUPPORT_DYN_VC_GFX_PROTOCOL));
    let name = blocks.core.client_name.trim_end_matches('\0').to_owned();
    let desktop = (blocks.core.desktop_width, blocks.core.desktop_height);
    let scale = blocks.core.optional_data.desktop_scale_factor;
    update(log, index, |r| {
        r.client_name = Some(name);
        r.client_desktop = Some(desktop);
        r.client_desktop_scale = scale;
        r.gfx_protocol_advertised = gfx;
    });
}

async fn read_tpkt(tcp: &mut TcpStream) -> Result<Vec<u8>> {
    let mut header = [0u8; 4];
    tcp.read_exact(&mut header).await.map_err(|e| err("read tpkt header", e))?;
    let len = usize::from(u16::from_be_bytes([header[2], header[3]]));
    if len < 4 {
        return Err(format!("bad TPKT length {len}"));
    }
    let mut frame = header.to_vec();
    frame.resize(len, 0);
    tcp.read_exact(&mut frame[4..]).await.map_err(|e| err("read tpkt body", e))?;
    Ok(frame)
}

async fn read_rdstls_auth_request<S: tokio::io::AsyncRead + Unpin>(s: &mut S) -> Result<RdstlsRequest> {
    async fn u16le<S: tokio::io::AsyncRead + Unpin>(s: &mut S) -> Result<u16> {
        let mut b = [0u8; 2];
        s.read_exact(&mut b).await.map_err(|e| err("read rdstls", e))?;
        Ok(u16::from_le_bytes(b))
    }
    async fn blob<S: tokio::io::AsyncRead + Unpin>(s: &mut S) -> Result<Vec<u8>> {
        let len = usize::from(u16le(s).await?);
        let mut v = vec![0u8; len];
        s.read_exact(&mut v).await.map_err(|e| err("read rdstls field", e))?;
        Ok(v)
    }
    fn utf16(b: &[u8]) -> String {
        let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units).trim_end_matches('\0').to_owned()
    }
    let (version, pdu_type, data_type) = (u16le(s).await?, u16le(s).await?, u16le(s).await?);
    if (version, pdu_type, data_type) != (1, 2, 1) {
        return Err(format!("unexpected RDSTLS header {version}/{pdu_type}/{data_type}"));
    }
    let redirection_guid = blob(s).await?;
    let username = utf16(&blob(s).await?);
    let domain = utf16(&blob(s).await?);
    let password = blob(s).await?;
    Ok(RdstlsRequest { redirection_guid, username, domain, password })
}

/// Capabilities of the fake server's Demand Active (a subset of `ironrdp-server`'s).
fn server_capabilities(size: DesktopSize) -> Vec<CapabilitySet> {
    vec![
        CapabilitySet::General(caps::General {
            extra_flags: caps::GeneralExtraFlags::FASTPATH_OUTPUT_SUPPORTED,
            suppress_output_support: true,
            ..Default::default()
        }),
        CapabilitySet::Bitmap(caps::Bitmap {
            pref_bits_per_pix: 32,
            desktop_width: size.width,
            desktop_height: size.height,
            desktop_resize_flag: true,
            drawing_flags: caps::BitmapDrawingFlags::empty(),
        }),
        CapabilitySet::Order(caps::Order::new(
            caps::OrderFlags::empty(),
            caps::OrderSupportExFlags::empty(),
            2048,
            224,
        )),
        CapabilitySet::Pointer(caps::Pointer { color_pointer_cache_size: 2048, pointer_cache_size: 2048 }),
        CapabilitySet::Input(caps::Input {
            input_flags: caps::InputFlags::SCANCODES
                | caps::InputFlags::MOUSEX
                | caps::InputFlags::FASTPATH_INPUT
                | caps::InputFlags::UNICODE
                | caps::InputFlags::FASTPATH_INPUT_2,
            keyboard_layout: 0,
            keyboard_type: None,
            keyboard_subtype: 0,
            keyboard_function_key: 128,
            keyboard_ime_filename: String::new(),
        }),
        CapabilitySet::VirtualChannel(caps::VirtualChannel {
            flags: caps::VirtualChannelFlags::NO_COMPRESSION,
            chunk_size: None,
        }),
        CapabilitySet::MultiFragmentUpdate(caps::MultifragmentUpdate { max_request_size: 8 * 1024 * 1024 }),
    ]
}

/// A Server Redirection PDU shaped like g-r-d's (plan §1.3): `redirFlags` `0x1C016`, routing
/// token `Cookie: msts=<token>\r\n`, a one-time 16-char user name, a 34-byte opaque password,
/// a 50-byte GUID and a target certificate container holding `target_cert` (DER). No target
/// address, so the client reconnects to the same host and port. All values are synthetic;
/// `seed` varies the one-time credentials.
pub fn redirection_pdu(token: u32, target_cert: &[u8], seed: u8) -> ServerRedirectionPdu {
    use ironrdp_pdu::rdp::server_redirection::{
        ServerRedirectionFlags as F, TargetCertificateContainer, TargetCertificateElement,
    };
    let container = TargetCertificateContainer {
        elements: vec![TargetCertificateElement {
            element_type: TargetCertificateElement::TYPE_CERTIFICATE,
            encoding: TargetCertificateElement::ENCODING_ASN1_DER,
            data: target_cert.to_vec(),
        }],
    };
    let username: String =
        (0..16u8).map(|i| char::from(b'a' + (i.wrapping_mul(7).wrapping_add(seed)) % 26)).collect();
    ServerRedirectionPdu {
        session_id: 0,
        redirection_flags: F::LOAD_BALANCE_INFO
            | F::USERNAME
            | F::PASSWORD
            | F::PASSWORD_IS_PK_ENCRYPTED
            | F::REDIRECTION_GUID
            | F::TARGET_CERTIFICATE,
        load_balance_info: Some(format!("Cookie: msts={token}\r\n").into_bytes()),
        username: Some(username),
        password: Some((0..34u8).map(|i| i ^ seed).collect()),
        redirection_guid: Some((0..50u8).map(|i| b'A' + (i.wrapping_add(seed)) % 26).collect()),
        target_certificate: container.encode_wire().ok(),
        ..Default::default()
    }
}

/// CredSSP network client for NTLM-only acceptors: Kerberos is never used on loopback.
struct NoNetwork;

impl NetworkClient for NoNetwork {
    async fn send(&mut self, _request: &NetworkRequest) -> ConnectorResult<Vec<u8>> {
        Err(general_err!("FakeServer has no network client (NTLM only)"))
    }
}
