//! Starting the host from its config directory, shared by the `drift-host` command and the
//! menu-bar app (HOST-9).

use std::net::SocketAddr;
use std::path::Path;

use std::sync::Arc;

use drift_core::{SecretRole, SystemClock};
use uuid::Uuid;

use crate::advert::{HostAnnouncement, local_host_name};
use crate::config::HostConfig;
use crate::frames::TestPattern;
use crate::identity::HostIdentity;
use crate::pairing::{DeviceStore, Pairing};
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
    /// Device pairing (HOST-8); `None` when it is off (the host then stays on loopback).
    pub pairing: Option<Pairing>,
    /// What the host advertises and puts in pairing codes.
    pub announcement: HostAnnouncement,
    /// The Bonjour advert, registered while pairing is on; deregistered on drop.
    pub advert: Option<drift_apple::bonjour::Advertisement>,
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

/// Virtual displays when enabled and the private API passes its self-test; otherwise the
/// physical display (HOST-10).
fn display_manager(config: &HostConfig) -> Box<dyn crate::display::DisplayManager> {
    if !config.virtual_display {
        return Box::new(crate::display::PhysicalOnly);
    }
    if let Err(e) = crate::cg_virtual::self_test() {
        tracing::warn!(error = %e, "CGVirtualDisplay unavailable; capturing the physical display");
        return Box::new(crate::display::PhysicalOnly);
    }
    match crate::display::VirtualDisplays::start("Drift Host") {
        Ok(d) => Box::new(d),
        Err(e) => {
            tracing::warn!(error = %e, "could not start the virtual display thread");
            Box::new(crate::display::PhysicalOnly)
        }
    }
}

/// Loads `dir`'s configuration (giving the host its stable id on first start), creating it if
/// needed.
pub fn load_config(dir: &Path, listen: Option<SocketAddr>) -> Result<HostConfig, String> {
    let mut config = HostConfig::load(dir).map_err(|e| e.to_string())?;
    if let Some(listen) = listen {
        config.listen = listen;
    }
    if config.host_id.is_nil() {
        config.host_id = Uuid::new_v4();
        config.save(dir).map_err(|e| e.to_string())?;
    }
    Ok(config)
}

/// Loads `dir`'s configuration, the password, the identity and the paired devices, starts
/// listening (beyond loopback only with pairing on, HOST-8) and advertises the host.
pub fn start(dir: &Path, listen: Option<SocketAddr>) -> Result<Launched, String> {
    let config = load_config(dir, listen)?;
    let password = keychain()
        .get(ACCOUNT_ID, SecretRole::RdpUser)
        .map_err(|e| e.to_string())?
        .ok_or("no password stored: run `drift-host set-password` first")?;
    let identity = HostIdentity::load_or_create(dir).map_err(|e| e.to_string())?;
    // The screen when Screen Recording is granted, else the test pattern (HOST-pipeline); input
    // is injected when Accessibility allows it or the config says so (HOST-6).
    let live = crate::setup::capture_or_pattern();
    let input = crate::setup::input_for(config.inject_input, live.as_ref().map(|l| l.1.clone()));
    if !config.pairing && config.listen != config.effective_listen() {
        tracing::warn!(listen = %config.listen, "pairing is off: listening on loopback only");
    }
    let store = DeviceStore::load(dir).map_err(|e| e.to_string())?;
    let pairing = config.pairing.then(|| Pairing::new(store, Arc::new(SystemClock)));
    let host = Host::start(HostOptions {
        listen: config.effective_listen(),
        identity: identity.clone(),
        username: config.username.clone(),
        password,
        desktop: config.desktop,
        frames: Box::new(|| Box::new(TestPattern::new())),
        live: live.map(|l| l.0),
        input,
        // HOST-11: the Mac's pasteboard over CLIPRDR, its system audio over rdpsnd.
        clipboard: Some(Box::new(|| Box::new(drift_clipboard::pasteboard::NsPasteboard::general()))),
        audio: Some(Box::new(|| Box::new(crate::audio::SystemAudio::default()))),
        displays: display_manager(&config),
        pairing: pairing.clone(),
    })
    .map_err(|e| format!("cannot listen on {}: {e}", config.listen))?;
    let host_name = local_host_name();
    let name = if config.name.trim().is_empty() {
        host_name.trim_end_matches(".local").to_owned()
    } else {
        config.name.clone()
    };
    let announcement = HostAnnouncement {
        host_id: config.host_id,
        name,
        host: host_name,
        port: host.port(),
        fingerprint: identity.fingerprint(),
    };
    // Discovery only makes sense where devices may connect: with pairing.
    let advert = pairing.as_ref().and_then(|_| announcement.advertise());
    Ok(Launched { host, config, fingerprint: identity.fingerprint(), pairing, announcement, advert })
}
