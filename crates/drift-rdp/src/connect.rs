//! Connection establishment for one leg (task **M1-1**): TCP, X.224, TLS with TOFU / pinning
//! on the SHA-256 of the leaf certificate, NLA (CredSSP) or RDSTLS, then capability exchange
//! and finalization.
//!
//! The pure decisions live here as small functions with table tests:
//! [`check_certificate`], [`build_config`], [`client_name_from_hostname`],
//! [`classify_io_error`] and [`classify_connector_error`]. [`connect_leg`] is the thin async
//! driver around IronRDP's `ClientConnector`; every timer goes through the injected
//! [`Clock`] (a `ManualClock` in tests), and the TCP dial goes through a [`Dialer`] so tests
//! can inject errors such as `EHOSTUNREACH`.

use std::future::Future;
use std::io;
use std::time::{Duration, Instant};

use drift_core::{CertFingerprint, Clock, ConnectMode, ConnectStage, DesktopSize, DisconnectReason};
use ironrdp_connector::{ClientConnector, ConnectionResult, ConnectorError, ConnectorErrorKind, Credentials};
use ironrdp_pdu::gcc::{ConnectionType, KeyboardType};
use ironrdp_pdu::rdp::capability_sets::{MajorPlatformType, RailSupportLevel};
use ironrdp_pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use tokio::net::TcpStream;
use zeroize::Zeroizing;

use crate::rdstls::OneTimeCredentials;
use crate::session::CertificateRole;
use crate::tls;

/// How often a pending network phase re-checks its deadline on the injected [`Clock`].
const DEADLINE_TICK: Duration = Duration::from_millis(25);

/// The byte stream a leg runs over (HOST-12): TCP or QUIC's main stream.
pub use drift_transport::stream::TransportStream as BoxedStream;

/// The TLS-upgraded transport of a connected leg.
pub type UpgradedFramed = ironrdp_tokio::TokioFramed<tokio_rustls::client::TlsStream<BoxedStream>>;

/// A dialed connection, before X.224.
pub struct Dialed {
    /// The byte stream.
    pub stream: BoxedStream,
    /// The local address (the RDP client address field).
    pub local_addr: std::net::SocketAddr,
    /// Which transport it is.
    pub transport: drift_transport::plan::Transport,
}

impl std::fmt::Debug for Dialed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dialed")
            .field("local_addr", &self.local_addr)
            .field("transport", &self.transport)
            .finish()
    }
}

impl Dialed {
    /// A TCP connection.
    pub fn tcp(stream: TcpStream) -> io::Result<Self> {
        let local_addr = stream.local_addr()?;
        Ok(Self {
            stream: BoxedStream::Tcp(stream),
            local_addr,
            transport: drift_transport::plan::Transport::Tcp,
        })
    }
}

/// How long a QUIC attempt may take before falling back to TCP: a firewall that drops UDP makes
/// QUIC wait rather than fail (ADR `HOST-12-quic-transport`).
pub const QUIC_ATTEMPT: Duration = Duration::from_secs(3);

/// Desktop size requested when the view geometry is not known yet (plan §1.4 captures).
pub const DEFAULT_DESKTOP: DesktopSize = DesktopSize { width: 1280, height: 800 };

/// Maximum RDP client name length in characters (GCC `clientName`, 16 UTF-16 units with NUL).
pub const MAX_CLIENT_NAME_CHARS: usize = 15;

/// How a leg authenticates.
pub enum LegAuth {
    /// NLA (CredSSP/NTLM) with long-lived credentials: Headless, Desktop Sharing, Remote Login leg 1.
    Nla {
        /// RDP user name.
        username: String,
        /// RDP password.
        password: Zeroizing<String>,
    },
    /// RDSTLS with the one-time credentials of a Server Redirection PDU (Remote Login legs ≥ 2).
    Rdstls(OneTimeCredentials),
}

impl std::fmt::Debug for LegAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nla { username, .. } => {
                f.debug_struct("Nla").field("username", username).field("password", &"<redacted>").finish()
            }
            Self::Rdstls(c) => f.debug_tuple("Rdstls").field(c).finish(),
        }
    }
}

/// What the leaf certificate of a leg must be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertExpectation {
    /// The profile's TOFU pin (SHA-256 of the leaf DER).
    Pinned(CertFingerprint),
    /// No pin yet: an unknown certificate is shown to the user (TOFU).
    TrustOnFirstUse,
    /// A Server Redirection target: the leaf DER must equal the certificate from the
    /// redirection PDU's target-certificate container (plan §1.3, M3-1).
    Exact(Vec<u8>),
}

/// Outcome of [`check_certificate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertVerdict {
    /// The certificate is the expected one.
    Trusted,
    /// The certificate differs from the pin / redirect target: abort with `CertMismatch`.
    Mismatch,
    /// Unknown certificate: ask the user.
    Unknown(CertFingerprint),
}

/// Checks a leaf certificate against the expectation. Pure.
pub fn check_certificate(expectation: &CertExpectation, leaf_der: &[u8]) -> CertVerdict {
    let fingerprint = CertFingerprint::of_der(leaf_der);
    match expectation {
        CertExpectation::Pinned(pin) if *pin == fingerprint => CertVerdict::Trusted,
        CertExpectation::Exact(der) if der.as_slice() == leaf_der => CertVerdict::Trusted,
        CertExpectation::Pinned(_) | CertExpectation::Exact(_) => CertVerdict::Mismatch,
        CertExpectation::TrustOnFirstUse => CertVerdict::Unknown(fingerprint),
    }
}

/// Everything needed to connect one leg.
#[derive(Debug)]
pub struct LegRequest {
    /// Connection leg (1 = initial, ≥ 2 after a redirect).
    pub leg: u8,
    /// Profile mode.
    pub mode: ConnectMode,
    /// Host to dial.
    pub host: String,
    /// Port to dial.
    pub port: u16,
    /// TLS server name / host shown in prompts (e.g. `10.1.2.40` behind an SSH forward).
    pub tls_server_name: String,
    /// Authentication.
    pub auth: LegAuth,
    /// Certificate expectation.
    pub expectation: CertExpectation,
    /// Which certificate a prompt is about.
    pub role: CertificateRole,
    /// Routing token from the redirection PDU (`Cookie: msts=<n>`, without CRLF).
    pub routing_token: Option<String>,
    /// RDP client name (the Mac's host name).
    pub client_name: String,
    /// Requested desktop size.
    pub desktop: DesktopSize,
    /// `DesktopScaleFactor` in percent.
    pub scale: u32,
    /// A paired Drift Host's pinned certificate: try QUIC first (HOST-12). `None` for TCP only.
    pub quic_pin: Option<CertFingerprint>,
}

/// The user's answer to a certificate prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertDecision {
    /// Trust it; `pin = true` persists it as the profile pin.
    Accept {
        /// Persist the pin.
        pin: bool,
    },
    /// Reject (ends in `CertMismatch`).
    Reject,
    /// The session was closed while prompting (ends in `UserClosed`).
    Closed,
}

/// Receives progress and answers certificate prompts while a leg connects.
pub trait ConnectObserver: Send {
    /// A new stage of the leg started.
    fn stage(&mut self, stage: ConnectStage);

    /// An unknown certificate needs a decision. Connection timers are paused meanwhile.
    fn decide_certificate(
        &mut self,
        host: &str,
        port: u16,
        fingerprint: CertFingerprint,
        role: CertificateRole,
    ) -> impl Future<Output = CertDecision> + Send;
}

/// Opens the connection of a leg (injectable for tests).
pub trait Dialer: Send + Sync {
    /// Connects to `host:port`. With `quic_pin` (a paired Drift Host's pinned certificate), RDP
    /// over QUIC on the same port (UDP) is tried first, then TCP (ADR `IOS-0` decision 6).
    fn dial(
        &self,
        host: &str,
        port: u16,
        quic_pin: Option<CertFingerprint>,
    ) -> impl Future<Output = io::Result<Dialed>> + Send;
}

/// The real dialer: TCP (`TCP_NODELAY`), or QUIC first for a paired Drift Host.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioDialer;

impl TokioDialer {
    async fn dial_quic(host: String, port: u16, pin: CertFingerprint) -> io::Result<Dialed> {
        // Owned: a borrowed host held across these awaits trips rustc's higher-ranked `Send`
        // check for the session actor's future.
        let addr = tokio::net::lookup_host(format!("{host}:{port}"))
            .await?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"))?;
        let session = drift_transport::quic::connect(addr, &host, pin).await.map_err(io::Error::other)?;
        let local_addr =
            session.local_addr().unwrap_or_else(|| std::net::SocketAddr::from(([0, 0, 0, 0], 0)));
        Ok(Dialed {
            stream: BoxedStream::Quic(session.into_stream()),
            local_addr,
            transport: drift_transport::plan::Transport::Quic,
        })
    }
}

impl Dialer for TokioDialer {
    async fn dial(&self, host: &str, port: u16, quic_pin: Option<CertFingerprint>) -> io::Result<Dialed> {
        if let Some(pin) = quic_pin {
            match tokio::time::timeout(QUIC_ATTEMPT, Self::dial_quic(host.to_owned(), port, pin)).await {
                Ok(Ok(dialed)) => {
                    tracing::info!(%host, port, "connected over QUIC");
                    return Ok(dialed);
                }
                Ok(Err(e)) => tracing::info!(error = %e, "QUIC failed; falling back to TCP"),
                Err(_) => tracing::info!("no QUIC answer; falling back to TCP"),
            }
        }
        let stream = TcpStream::connect((host, port)).await?;
        stream.set_nodelay(true)?;
        Dialed::tcp(stream)
    }
}

/// A connected, activated leg.
pub struct ConnectedLeg {
    /// The TLS transport, ready for the active stage.
    pub framed: UpgradedFramed,
    /// IronRDP's connection result (channels, share id, desktop size).
    pub result: ConnectionResult,
    /// The server's leaf certificate DER.
    pub leaf_der: Vec<u8>,
}

impl std::fmt::Debug for ConnectedLeg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectedLeg")
            .field("desktop_size", &self.result.desktop_size)
            .field("leaf", &CertFingerprint::of_der(&self.leaf_der).to_string())
            .finish_non_exhaustive()
    }
}

/// Builds IronRDP's connector configuration for a leg. Pure.
///
/// Fixed by plan §1.3/M1-1: TLS on, NLA for [`LegAuth::Nla`] (RDSTLS legs turn NLA off,
/// set `autologon` and send an empty password), `support_dyn_vc_gfx_protocol = true`,
/// platform `MACINTOSH`, client name as given.
pub fn build_config(req: &LegRequest) -> ironrdp_connector::Config {
    let (credentials, nla) = match &req.auth {
        LegAuth::Nla { username, password } => (
            Credentials::UsernamePassword {
                username: username.clone(),
                password: password.as_str().to_owned(),
            },
            true,
        ),
        // RDSTLS: the Client Info carries the one-time user name, INFO_AUTOLOGON and an empty
        // password (plan §1.3); the one-time password only travels in the RDSTLS AuthRequest.
        LegAuth::Rdstls(creds) => (
            Credentials::UsernamePassword { username: creds.username().to_owned(), password: String::new() },
            false,
        ),
    };
    let dim = |v: u32| u16::try_from(v).unwrap_or(u16::MAX);
    ironrdp_connector::Config {
        credentials,
        domain: None,
        enable_tls: true,
        enable_credssp: nla,
        enable_standard_rdp_security: false,
        keyboard_type: KeyboardType::IBM_ENHANCED,
        keyboard_subtype: 0,
        keyboard_layout: 0x409,
        keyboard_functional_keys_count: 12,
        connection_type: ConnectionType::Lan,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: ironrdp_connector::DesktopSize {
            width: dim(req.desktop.width),
            height: dim(req.desktop.height),
        },
        monitor_layout: None,
        desktop_scale_factor: req.scale,
        bitmap: None,
        client_build: 1,
        client_name: req.client_name.clone(),
        client_dir: "C:\\drift".into(),
        platform: MajorPlatformType::MACINTOSH,
        hardware_id: None,
        request_data: None,
        autologon: !nla,
        enable_audio_playback: false,
        enable_audio_capture: false,
        performance_flags: PerformanceFlags::default(),
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        compression_type: None,
        enable_server_pointer: true,
        pointer_software_rendering: false,
        multitransport_flags: None,
        support_dyn_vc_gfx_protocol: true,
        alternate_shell: String::new(),
        work_dir: String::new(),
        remote_application_mode: false,
        rail_support_level: RailSupportLevel::empty(),
    }
}

/// Derives an RDP client name from a host name: the first DNS label, `[A-Za-z0-9-]` only,
/// at most [`MAX_CLIENT_NAME_CHARS`] characters, `"drift"` when nothing is left. Pure.
pub fn client_name_from_hostname(hostname: &str) -> String {
    let label = hostname.split('.').next().unwrap_or_default();
    let name: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(MAX_CLIENT_NAME_CHARS)
        .collect();
    if name.is_empty() { "drift".to_owned() } else { name }
}

/// The Mac's host name as an RDP client name.
pub fn local_client_name() -> String {
    client_name_from_hostname(&hostname())
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is a valid, writable buffer of `buf.len()` bytes; gethostname writes at
    // most that many bytes, and only the bytes before the first NUL are read afterwards.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// Maps an I/O error at `stage` to a disconnect reason. Pure.
///
/// `EHOSTUNREACH` (errno 65) is how macOS Local Network Privacy denies a connection
/// (plan §1.8) → [`DisconnectReason::LocalNetworkDenied`].
pub fn classify_io_error(error: &io::Error, stage: ConnectStage) -> DisconnectReason {
    use io::ErrorKind as K;
    match error.raw_os_error() {
        Some(libc::EHOSTUNREACH) => return DisconnectReason::LocalNetworkDenied,
        Some(libc::ETIMEDOUT) => return DisconnectReason::Timeout,
        _ => {}
    }
    let dropped =
        matches!(error.kind(), K::UnexpectedEof | K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe);
    match (stage, error.kind()) {
        // g-r-d (FreeRDP) drops the connection when NLA rejects the credentials.
        (ConnectStage::Nla, _) if dropped => DisconnectReason::AuthFailed,
        (_, K::TimedOut) => DisconnectReason::Timeout,
        (ConnectStage::Tcp, _) => DisconnectReason::Network,
        (_, K::UnexpectedEof) => DisconnectReason::TlsEof,
        _ => DisconnectReason::Network,
    }
}

/// Maps an IronRDP connector error at `stage` to a disconnect reason. Pure.
///
/// RDSTLS rejections keep their result code (`0x52E` → `RdstlsFailed(0x52E)`), CredSSP
/// failures are `AuthFailed`, I/O errors in the source chain go through
/// [`classify_io_error`], anything else is a `ProtocolError`.
pub fn classify_connector_error(error: &ConnectorError, stage: ConnectStage) -> DisconnectReason {
    match error.kind() {
        ConnectorErrorKind::RdstlsAuthFailed(code) => DisconnectReason::RdstlsFailed(code.0),
        ConnectorErrorKind::Credssp(_) | ConnectorErrorKind::AccessDenied => DisconnectReason::AuthFailed,
        _ => match io_source(error) {
            Some(io) => classify_io_error(io, stage),
            None => DisconnectReason::ProtocolError(error.to_string()),
        },
    }
}

/// The first `io::Error` in an error's source chain.
fn io_source<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a io::Error> {
    let mut current = error.source();
    while let Some(e) = current {
        if let Some(io) = e.downcast_ref::<io::Error>() {
            return Some(io);
        }
        current = e.source();
    }
    None
}

/// Connects one leg: TCP → X.224 → TLS (certificate check, prompt if unknown) →
/// NLA or RDSTLS → capability exchange → finalization.
///
/// `attach` adds static channels (DRDYNVC with GFX/DISP, CLIPRDR) to the connector.
/// `timeout` bounds each network phase on `clock`; it is paused while the user decides on a
/// certificate.
pub async fn connect_leg(
    req: LegRequest,
    dialer: &impl Dialer,
    clock: &dyn Clock,
    timeout: Duration,
    observer: &mut impl ConnectObserver,
    attach: impl FnOnce(ClientConnector) -> ClientConnector + Send,
) -> Result<ConnectedLeg, DisconnectReason> {
    let config = build_config(&req);
    let LegRequest { host, port, tls_server_name, auth, expectation, role, routing_token, quic_pin, .. } =
        req;

    // TCP.
    observer.stage(ConnectStage::Tcp);
    let deadline = clock.now() + timeout;
    let dialed = until(clock, deadline, dialer.dial(&host, port, quic_pin))
        .await?
        .map_err(|e| classify_io_error(&e, ConnectStage::Tcp))?;
    let client_addr = dialed.local_addr;
    let tcp = dialed.stream;

    let mut connector = attach(ClientConnector::new(config, client_addr));
    if let Some(token) = routing_token {
        connector = connector.with_load_balance_info(token);
    }
    let auth_stage = match auth {
        LegAuth::Nla { .. } => ConnectStage::Nla,
        LegAuth::Rdstls(creds) => {
            connector = connector.with_rdstls_credentials(creds.into_connector());
            ConnectStage::Rdstls
        }
    };

    // X.224 negotiation.
    let mut framed = ironrdp_tokio::TokioFramed::new(tcp);
    let should_upgrade = until(clock, deadline, ironrdp_async::connect_begin(&mut framed, &mut connector))
        .await?
        .map_err(|e| classify_connector_error(&e, ConnectStage::Tcp))?;

    // TLS, then the certificate decision, before any credential leaves the machine.
    observer.stage(ConnectStage::Tls);
    let tcp = framed.into_inner_no_leftover();
    let (tls_stream, leaf_der) = until(clock, deadline, tls::upgrade(tcp, &tls_server_name))
        .await?
        .map_err(|e| classify_io_error(&e, ConnectStage::Tls))?;
    match check_certificate(&expectation, &leaf_der) {
        CertVerdict::Trusted => {}
        CertVerdict::Mismatch => return Err(DisconnectReason::CertMismatch),
        CertVerdict::Unknown(fingerprint) => {
            match observer.decide_certificate(&tls_server_name, port, fingerprint, role).await {
                CertDecision::Accept { .. } => {}
                CertDecision::Reject => return Err(DisconnectReason::CertMismatch),
                CertDecision::Closed => return Err(DisconnectReason::UserClosed),
            }
        }
    }
    let server_public_key = tls::subject_public_key(&leaf_der).ok_or_else(|| {
        DisconnectReason::ProtocolError("server certificate has no parsable public key".into())
    })?;
    let upgraded = ironrdp_async::mark_as_upgraded(should_upgrade, &mut connector);

    // NLA or RDSTLS, capability exchange and finalization (fresh deadline after the prompt).
    observer.stage(auth_stage);
    let deadline = clock.now() + timeout;
    let mut framed = ironrdp_tokio::TokioFramed::new(tls_stream);
    let result = until(
        clock,
        deadline,
        ironrdp_async::connect_finalize(
            upgraded,
            connector,
            &mut framed,
            &mut NoKerberos,
            ironrdp_connector::ServerName::new(tls_server_name),
            server_public_key,
            None,
        ),
    )
    .await?
    .map_err(|e| classify_connector_error(&e, auth_stage))?;

    Ok(ConnectedLeg { framed, result, leaf_der })
}

/// Runs `fut` until it completes or `clock` passes `deadline` ([`DisconnectReason::Timeout`]).
///
/// The deadline is re-checked every [`DEADLINE_TICK`] of real time against the injected
/// clock, so a `ManualClock` fully controls when timeouts fire.
pub(crate) async fn until<F: Future>(
    clock: &dyn Clock,
    deadline: Instant,
    fut: F,
) -> Result<F::Output, DisconnectReason> {
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(DEADLINE_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            out = &mut fut => return Ok(out),
            _ = tick.tick() => {
                if clock.now() >= deadline {
                    return Err(DisconnectReason::Timeout);
                }
            }
        }
    }
}

/// CredSSP network client: Drift authenticates with NTLM (g-r-d) and never talks to a KDC.
struct NoKerberos;

impl ironrdp_async::NetworkClient for NoKerberos {
    async fn send(
        &mut self,
        _request: &ironrdp_connector::sspi::generator::NetworkRequest,
    ) -> ironrdp_connector::ConnectorResult<Vec<u8>> {
        Err(ironrdp_connector::general_err!("Kerberos network requests are not supported"))
    }
}
