//! RDP over QUIC between a Drift client and a Drift Host (task HOST-12, ADR
//! `HOST-12-quic-transport`).
//!
//! * The RDP connection (X.224, MCS, input, clipboard, every DVC) rides **one reliable
//!   bidirectional stream**, the "main" stream: it is the byte stream TLS-over-TCP carries today,
//!   so the connector and the server run over it unchanged. The client opens it and writes first.
//! * Each **GFX frame** rides **its own unidirectional stream** from the host: a lost packet then
//!   delays only the frame it belongs to, never the input or the next frame.
//! * QUIC's TLS 1.3 is rustls with the ring provider. The client **pins** the host certificate
//!   (SHA-256 of its DER, the same fingerprint Drift pins for TCP); there is no CA check.
//!
//! Frame streams start with `id: u32` and `len: u32` (big endian), then `len` bytes.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use drift_core::CertFingerprint;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use quinn::rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use quinn::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use quinn::rustls::{DigitallySignedStruct, SignatureScheme};
use quinn::{Connection, Endpoint, RecvStream, SendStream, rustls};
use tokio::sync::mpsc;

/// ALPN of RDP over QUIC between Drift ends.
pub const ALPN: &[u8] = b"drift-rdp/1";

/// Largest GFX frame accepted from the host (a 4K AVC444 keyframe fits with room to spare).
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

/// Frame streams the host may have open at once.
const MAX_FRAME_STREAMS: u32 = 256;

/// A QUIC failure.
#[derive(Debug, thiserror::Error)]
pub enum QuicError {
    /// The host's certificate is not the pinned one.
    #[error("the host certificate {got} does not match the pinned {expected}")]
    PinMismatch {
        /// The pinned fingerprint.
        expected: CertFingerprint,
        /// What the host presented.
        got: CertFingerprint,
    },
    /// TLS configuration (bad certificate or key).
    #[error("TLS setup: {0}")]
    Tls(String),
    /// Socket setup.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The QUIC handshake or connection failed.
    #[error("QUIC: {0}")]
    Connection(String),
    /// A frame stream was malformed or too large.
    #[error("bad frame stream: {0}")]
    Frame(String),
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The sender's congestion controller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Congestion {
    /// BBR: paces to the measured bottleneck and does not treat random loss as congestion, so
    /// Wi-Fi loss does not collapse the frame rate (ADR `HOST-12-quic-transport`).
    #[default]
    Bbr,
    /// CUBIC (quinn's default): loss-based, like TCP.
    Cubic,
}

fn transport_config(congestion: Congestion) -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    match congestion {
        Congestion::Bbr => {
            t.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
        }
        Congestion::Cubic => {
            t.congestion_controller_factory(Arc::new(quinn::congestion::CubicConfig::default()));
        }
    }
    t.max_concurrent_uni_streams(MAX_FRAME_STREAMS.into());
    t.max_concurrent_bidi_streams(4u32.into());
    t.keep_alive_interval(Some(Duration::from_secs(4)));
    Arc::new(t)
}

/// A Drift Host's QUIC endpoint.
#[derive(Debug)]
pub struct QuicHost {
    endpoint: Endpoint,
}

impl QuicHost {
    /// Listens on `addr` with the host certificate (`cert_der`) and its PKCS#8 key, sending
    /// with BBR.
    pub fn bind(addr: SocketAddr, cert_der: Vec<u8>, key_pkcs8: Vec<u8>) -> Result<Self, QuicError> {
        Self::bind_with(addr, cert_der, key_pkcs8, Congestion::default())
    }

    /// [`Self::bind`] with an explicit congestion controller (measurements).
    pub fn bind_with(
        addr: SocketAddr,
        cert_der: Vec<u8>,
        key_pkcs8: Vec<u8>,
        congestion: Congestion,
    ) -> Result<Self, QuicError> {
        let tls = rustls::ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| QuicError::Tls(e.to_string()))?
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert_der)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pkcs8)),
            )
            .map_err(|e| QuicError::Tls(e.to_string()))?;
        let mut tls = tls;
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let crypto = QuicServerConfig::try_from(tls).map_err(|e| QuicError::Tls(e.to_string()))?;
        let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        config.transport_config(transport_config(congestion));
        Ok(Self { endpoint: Endpoint::server(config, addr)? })
    }

    /// The bound address.
    pub fn local_addr(&self) -> SocketAddr {
        // `Endpoint::local_addr` only fails if the socket is gone, which `self` prevents.
        self.endpoint.local_addr().unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 0)))
    }

    /// Waits for the next client and its main stream.
    pub async fn accept(&self) -> Result<QuicSession, QuicError> {
        let incoming =
            self.endpoint.accept().await.ok_or_else(|| QuicError::Connection("endpoint closed".into()))?;
        let connection = incoming.await.map_err(|e| QuicError::Connection(e.to_string()))?;
        let (send, recv) = connection.accept_bi().await.map_err(|e| QuicError::Connection(e.to_string()))?;
        Ok(QuicSession::new(connection, send, recv, false))
    }
}

/// Connects to a Drift Host at `addr`, pinning its certificate to `pin`, and opens the main
/// stream. `server_name` is the name sent in SNI (any name: the pin is what is checked).
pub async fn connect(
    addr: SocketAddr,
    server_name: &str,
    pin: CertFingerprint,
) -> Result<QuicSession, QuicError> {
    let verifier = Arc::new(PinnedVerifier { pin, seen: Mutex::default(), provider: provider() });
    let mut tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| QuicError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = QuicClientConfig::try_from(tls).map_err(|e| QuicError::Tls(e.to_string()))?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(transport_config(Congestion::default()));
    let bind: SocketAddr = if addr.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }
        .parse()
        .map_err(|_| QuicError::Connection("bad bind address".into()))?;
    let mut endpoint = Endpoint::client(bind)?;
    endpoint.set_default_client_config(config);
    let connecting = endpoint.connect(addr, server_name).map_err(|e| QuicError::Connection(e.to_string()))?;
    let connection = match connecting.await {
        Ok(c) => c,
        Err(e) => {
            if let Some(got) = verifier.mismatch() {
                return Err(QuicError::PinMismatch { expected: pin, got });
            }
            return Err(QuicError::Connection(e.to_string()));
        }
    };
    let (send, recv) = connection.open_bi().await.map_err(|e| QuicError::Connection(e.to_string()))?;
    let mut session = QuicSession::new(connection, send, recv, true);
    session.endpoint = Some(endpoint);
    Ok(session)
}

/// One RDP-over-QUIC connection: the main stream plus frame streams.
#[derive(Debug)]
pub struct QuicSession {
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
    frames: Option<mpsc::Receiver<(u32, Vec<u8>)>>,
    /// The client keeps its endpoint alive with the session.
    endpoint: Option<Endpoint>,
}

impl QuicSession {
    fn new(connection: Connection, send: SendStream, recv: RecvStream, client: bool) -> Self {
        let frames = client.then(|| {
            let (tx, rx) = mpsc::channel(64);
            tokio::spawn(receive_frames(connection.clone(), tx));
            rx
        });
        Self { connection, send, recv, frames, endpoint: None }
    }

    /// The main stream's sending half (the RDP byte stream).
    pub fn main_send(&mut self) -> &mut SendStream {
        &mut self.send
    }

    /// The main stream's receiving half.
    pub fn main_recv(&mut self) -> &mut RecvStream {
        &mut self.recv
    }

    /// Host side: sends one GFX frame on its own stream. Returns once it is queued.
    pub async fn send_frame(&self, id: u32, data: &[u8]) -> Result<(), QuicError> {
        let len = u32::try_from(data.len()).map_err(|_| QuicError::Frame("frame too large".into()))?;
        let mut stream =
            self.connection.open_uni().await.map_err(|e| QuicError::Connection(e.to_string()))?;
        let mut header = [0u8; 8];
        header[..4].copy_from_slice(&id.to_be_bytes());
        header[4..].copy_from_slice(&len.to_be_bytes());
        stream.write_all(&header).await.map_err(|e| QuicError::Connection(e.to_string()))?;
        stream.write_all(data).await.map_err(|e| QuicError::Connection(e.to_string()))?;
        stream.finish().map_err(|e| QuicError::Connection(e.to_string()))?;
        Ok(())
    }

    /// Client side: the next complete frame, in the order frames *complete* (not the order they
    /// were sent). `None` once the connection is gone.
    pub async fn recv_frame(&mut self) -> Option<(u32, Vec<u8>)> {
        self.frames.as_mut()?.recv().await
    }

    /// The smoothed round-trip time.
    pub fn rtt(&self) -> Duration {
        self.connection.rtt()
    }
}

/// Reads every frame stream the host opens, each on its own task, and forwards whole frames.
async fn receive_frames(connection: Connection, tx: mpsc::Sender<(u32, Vec<u8>)>) {
    while let Ok(mut stream) = connection.accept_uni().await {
        let tx = tx.clone();
        tokio::spawn(async move {
            match read_frame(&mut stream).await {
                Ok(frame) => {
                    let _ = tx.send(frame).await;
                }
                Err(e) => tracing::warn!(error = %e, "dropped a frame stream"),
            }
        });
    }
}

async fn read_frame(stream: &mut RecvStream) -> Result<(u32, Vec<u8>), QuicError> {
    let mut header = [0u8; 8];
    stream.read_exact(&mut header).await.map_err(|e| QuicError::Frame(e.to_string()))?;
    let id = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let len = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(QuicError::Frame(format!("frame {id} claims {len} bytes")));
    }
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).await.map_err(|e| QuicError::Frame(e.to_string()))?;
    Ok((id, data))
}

/// Accepts exactly the pinned certificate; signatures are still verified.
#[derive(Debug)]
struct PinnedVerifier {
    pin: CertFingerprint,
    seen: Mutex<Option<CertFingerprint>>,
    provider: Arc<CryptoProvider>,
}

impl PinnedVerifier {
    fn mismatch(&self) -> Option<CertFingerprint> {
        (*self.seen.lock().unwrap_or_else(PoisonError::into_inner)).filter(|seen| *seen != self.pin)
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let got = CertFingerprint::of_der(end_entity.as_ref());
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(got);
        if got == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("host certificate does not match the pin".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}
