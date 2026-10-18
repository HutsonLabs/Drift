//! The host's TLS identity: a self-signed certificate created on first start and kept in the
//! config directory. Paired devices pin it (HOST-8), so it must never change by itself.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::sync::Arc;

use drift_core::CertFingerprint;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use zeroize::Zeroizing;

/// The certificate (DER) in the config directory.
pub const CERT_FILE: &str = "host-cert.der";
/// The private key (PKCS#8 DER, mode 0600) in the config directory.
pub const KEY_FILE: &str = "host-key.der";

/// The names the certificate is issued for. Drift pins the certificate, not the name.
const SUBJECT_NAMES: &[&str] = &["drift-host.local"];

/// Creating or loading the identity failed.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    /// The files could not be read or written.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The key or certificate is not usable.
    #[error("{0}")]
    Crypto(String),
}

/// The host's certificate and key.
#[derive(Clone)]
pub struct HostIdentity {
    cert_der: Vec<u8>,
    key_pkcs8: Arc<Zeroizing<Vec<u8>>>,
    public_key: Vec<u8>,
}

impl std::fmt::Debug for HostIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostIdentity").field("fingerprint", &self.fingerprint()).finish_non_exhaustive()
    }
}

impl HostIdentity {
    /// Loads `dir`'s identity, or creates one (and `dir`) on first start.
    pub fn load_or_create(dir: &Path) -> Result<Self, IdentityError> {
        let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        if cert_path.exists() && key_path.exists() {
            let cert_der = std::fs::read(&cert_path)?;
            let key = Zeroizing::new(std::fs::read(&key_path)?);
            return Self::from_parts(cert_der, key);
        }
        std::fs::create_dir_all(dir)?;
        let names: Vec<String> = SUBJECT_NAMES.iter().map(|s| (*s).to_owned()).collect();
        let certified =
            rcgen::generate_simple_self_signed(names).map_err(|e| IdentityError::Crypto(e.to_string()))?;
        let key = Zeroizing::new(certified.signing_key.serialize_der());
        let mut file =
            std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&key_path)?;
        file.write_all(&key)?;
        std::fs::write(&cert_path, certified.cert.der())?;
        Self::from_parts(certified.cert.der().to_vec(), key)
    }

    fn from_parts(cert_der: Vec<u8>, key: Zeroizing<Vec<u8>>) -> Result<Self, IdentityError> {
        let pair =
            rcgen::KeyPair::try_from(key.as_slice()).map_err(|e| IdentityError::Crypto(e.to_string()))?;
        let public_key = pair.public_key_raw().to_vec();
        Ok(Self { cert_der, key_pkcs8: Arc::new(key), public_key })
    }

    /// SHA-256 of the certificate, as Drift pins it.
    pub fn fingerprint(&self) -> CertFingerprint {
        CertFingerprint::of_der(&self.cert_der)
    }

    /// The certificate, DER.
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// The raw `subjectPublicKey` bits, which CredSSP binds to.
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    /// A TLS acceptor presenting this certificate (rustls with ring, as the client uses).
    pub fn acceptor(&self) -> Result<TlsAcceptor, IdentityError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_pkcs8.to_vec()));
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .and_then(|b| {
                b.with_no_client_auth()
                    .with_single_cert(vec![CertificateDer::from(self.cert_der.clone())], key)
            })
            .map_err(|e| IdentityError::Crypto(e.to_string()))?;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}
