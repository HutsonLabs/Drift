//! TLS upgrade for RDP (M1-1): rustls with the ring provider and a verifier that defers the
//! trust decision to Drift's TOFU pinning.
//!
//! g-r-d presents self-signed certificates, so WebPKI chain validation is meaningless.
//! [`PinningVerifier`] therefore accepts any chain during the handshake but **still verifies
//! the handshake signatures** with the leaf's public key (proof of possession). The caller then
//! checks the leaf DER against the profile pin or the redirect target certificate
//! ([`crate::connect::check_certificate`]) before any credential is sent.

use std::io;
use std::sync::Arc;

use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use tokio_rustls::rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{self, DigitallySignedStruct, SignatureScheme};

/// Accepts any certificate chain; verifies handshake signatures. See the module docs.
#[derive(Debug)]
struct PinningVerifier {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinningVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // Trust is decided after the handshake by comparing the leaf with the pin.
        Ok(ServerCertVerified::assertion())
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

fn client_config() -> Result<rustls::ClientConfig, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinningVerifier { provider }))
        .with_no_client_auth();
    // Every leg is a fresh, independently verified handshake (the spike disabled resumption too).
    config.resumption = rustls::client::Resumption::disabled();
    Ok(config)
}

/// Performs the TLS handshake on `tcp`; returns the stream and the server's leaf DER.
pub(crate) async fn upgrade<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    tcp: S,
    server_name: &str,
) -> io::Result<(TlsStream<S>, Vec<u8>)> {
    let config = client_config().map_err(io::Error::other)?;
    let name = ServerName::try_from(server_name.to_owned())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let stream = TlsConnector::from(Arc::new(config)).connect(name, tcp).await?;
    let leaf = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first())
        .map(|der| der.as_ref().to_vec())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "server sent no certificate"))?;
    Ok((stream, leaf))
}

/// The `subjectPublicKey` bits of a certificate (what CredSSP binds to), or `None` if the DER
/// does not parse.
pub(crate) fn subject_public_key(der: &[u8]) -> Option<Vec<u8>> {
    use x509_cert::der::Decode as _;
    let cert = x509_cert::Certificate::from_der(der).ok()?;
    cert.tbs_certificate().subject_public_key_info().subject_public_key.as_bytes().map(<[u8]>::to_vec)
}
