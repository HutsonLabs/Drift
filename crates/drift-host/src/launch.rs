//! Starting the host from its config directory, shared by the `drift-host` command and the
//! menu-bar app (HOST-9).

use std::net::SocketAddr;
use std::path::Path;

use drift_core::SecretRole;
use uuid::Uuid;

use crate::config::HostConfig;
use crate::frames::TestPattern;
use crate::identity::HostIdentity;
use crate::input::RecordingInput;
use crate::server::{Host, HostOptions};

/// Keychain service of the host's own items (not the client's `com.hutsonlabs.drift`).
pub const KEYCHAIN_SERVICE: &str = "com.hutsonlabs.drift.host";
/// The fixed "profile" id the host's NLA password is filed under.
pub const ACCOUNT_ID: Uuid = Uuid::from_u128(0x6472_6966_742d_686f_7374_2d6e_6c61_0001);

/// A started host and what to tell the user about it.
#[derive(Debug)]
pub struct Launched {
    /// The running host.
    pub host: Host,
    /// The configuration it runs with.
    pub config: HostConfig,
    /// The certificate fingerprint devices pin.
    pub fingerprint: drift_core::CertFingerprint,
}

/// The host's Keychain store.
pub fn keychain() -> drift_apple::Keychain {
    drift_apple::Keychain::with_service(KEYCHAIN_SERVICE)
}

/// Stores the account's password in the Keychain and writes `host.toml`.
pub fn set_password(dir: &Path, password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("empty password".into());
    }
    let config = HostConfig::load(dir).map_err(|e| e.to_string())?;
    keychain().set(ACCOUNT_ID, SecretRole::RdpUser, password).map_err(|e| e.to_string())?;
    config.save(dir).map_err(|e| e.to_string())
}

/// Loads `dir`'s configuration, the password and the identity, and starts listening.
pub fn start(dir: &Path, listen: Option<SocketAddr>) -> Result<Launched, String> {
    let mut config = HostConfig::load(dir).map_err(|e| e.to_string())?;
    if let Some(listen) = listen {
        config.listen = listen;
    }
    let password = keychain()
        .get(ACCOUNT_ID, SecretRole::RdpUser)
        .map_err(|e| e.to_string())?
        .ok_or("no password stored: run `drift-host set-password` first")?;
    let identity = HostIdentity::load_or_create(dir).map_err(|e| e.to_string())?;
    if config.inject_input {
        tracing::warn!("inject_input is set, but this build only logs input (HOST-6 wiring comes later)");
    }
    if config.listens_beyond_loopback() {
        tracing::warn!(listen = %config.listen, "listening beyond loopback without device pairing (HOST-8)");
    }
    let host = Host::start(HostOptions {
        listen: config.listen,
        identity: identity.clone(),
        username: config.username.clone(),
        password,
        desktop: config.desktop,
        frames: Box::new(|| Box::new(TestPattern::new())),
        input: Box::new(RecordingInput::default()),
    })
    .map_err(|e| format!("cannot listen on {}: {e}", config.listen))?;
    Ok(Launched { host, config, fingerprint: identity.fingerprint() })
}
