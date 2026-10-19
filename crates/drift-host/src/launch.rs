//! Starting the host from its config directory, shared by the `drift-host` command and the
//! menu-bar app (HOST-9).

use std::net::SocketAddr;
use std::path::Path;

use std::sync::Arc;

use drift_core::{SecretRole, SystemClock};
use uuid::Uuid;

use crate::advert::{HostAnnouncement, local_host_name};
use crate::agent::AgentLink;
use crate::broker::{AgentPolicy, SessionKind};
use crate::config::HostConfig;
use crate::daemon::{Daemon, DaemonOptions};
use crate::frames::TestPattern;
use crate::identity::HostIdentity;
use crate::pairing::DeviceSaver;
use crate::pairing::{DeviceStore, Pairing};
use crate::server::{Host, HostOptions, Incoming};

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

/// Whether the account's password is in the Keychain (checked without reading it, so no prompt;
/// the menu-bar app asks for it when not, HOST-15).
pub fn has_password() -> bool {
    keychain().contains(ACCOUNT_ID, SecretRole::RdpUser).unwrap_or(false)
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
    start_with(dir, listen, Incoming::Bind, None, true)
}

/// Starts an agent (HOST-13): it registers with the daemon at `socket` and serves the
/// connections the daemon hands it, from the shared store `dir`. It neither listens nor
/// advertises (the daemon does both), and pairing writes go through the daemon.
pub fn start_agent(
    dir: &Path,
    socket: &Path,
    kind: SessionKind,
) -> Result<(Launched, Arc<AgentLink>), String> {
    let link = Arc::new(
        AgentLink::connect(socket, kind)
            .map_err(|e| format!("cannot register with the daemon at {}: {e}", socket.display()))?,
    );
    let launched = start_with(dir, None, link.incoming(), Some(link.saver()), false)?;
    Ok((launched, link))
}

/// Starts the root broker daemon (HOST-13) on the shared store `dir`: it creates the host's
/// identity and configuration there (readable by the `admin` group, so agents can load them),
/// listens, hands connections to agents over `socket`, and advertises the host.
pub fn start_daemon(
    dir: &Path,
    listen: Option<SocketAddr>,
    socket: &Path,
) -> Result<(Daemon, Option<drift_apple::bonjour::Advertisement>), String> {
    let config = load_config(dir, listen)?;
    let identity = HostIdentity::load_or_create(dir).map_err(|e| e.to_string())?;
    share_store(dir);
    let daemon = Daemon::start(DaemonOptions {
        listen: config.effective_listen(),
        socket: socket.to_owned(),
        store_dir: dir.to_owned(),
        policy: AgentPolicy::system(),
        console: Box::new(crate::console::console_state),
        wait_for_agent: AGENT_WAIT,
    })
    .map_err(|e| format!("cannot start the broker on {}: {e}", config.listen))?;
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
        port: daemon.addr().port(),
        fingerprint: identity.fingerprint(),
        account: config.username.clone(),
    };
    let advert = config.pairing.then(|| announcement.advertise()).flatten();
    Ok((daemon, advert))
}

/// How long a connection waits for the console's agent (it may be starting after a login).
pub const AGENT_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The `admin` group's gid on macOS.
const ADMIN_GID: u32 = 80;

/// Makes the store readable by agents in the `admin` group: the directory 0750, the private key
/// 0640, both group `admin` (only when running as root; otherwise a no-op, e.g. in tests).
fn share_store(dir: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    for (path, mode) in [(dir.to_owned(), 0o750), (dir.join(crate::identity::KEY_FILE), 0o640)] {
        if let Err(e) = std::os::unix::fs::chown(&path, Some(0), Some(ADMIN_GID))
            .and_then(|()| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)))
        {
            tracing::warn!(path = %path.display(), error = %e, "could not share the store with agents");
        }
    }
}

/// [`start`] with where connections come from, where pairing writes go and whether to advertise.
fn start_with(
    dir: &Path,
    listen: Option<SocketAddr>,
    incoming: Incoming,
    saver: Option<DeviceSaver>,
    advertise: bool,
) -> Result<Launched, String> {
    let config = load_config(dir, listen)?;
    let password = keychain()
        .get(ACCOUNT_ID, SecretRole::RdpUser)
        .map_err(|e| e.to_string())?
        .ok_or("no password stored: choose Enter Your Mac’s Password in Drift Host’s menu, or run `drift-host set-password`")?;
    let identity = HostIdentity::load_or_create(dir).map_err(|e| e.to_string())?;
    // The screen when Screen Recording is granted, else the test pattern (HOST-pipeline); input
    // is injected when Accessibility allows it or the config says so (HOST-6).
    let live = crate::setup::capture_or_pattern();
    let input = crate::setup::input_for(config.inject_input, live.as_ref().map(|l| l.1.clone()));
    if !config.pairing && config.listen != config.effective_listen() {
        tracing::warn!(listen = %config.listen, "pairing is off: listening on loopback only");
    }
    let store = DeviceStore::load(dir).map_err(|e| e.to_string())?;
    let store = match saver {
        Some(saver) => store.with_saver(saver),
        None => store,
    };
    let pairing = config.pairing.then(|| Pairing::new(store, Arc::new(SystemClock)));
    let host = Host::start_with(
        HostOptions {
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
            menus: menus_for(&config),
        },
        incoming,
    )
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
        account: config.username.clone(),
    };
    // Discovery only makes sense where devices may connect: with pairing.
    let advert = pairing.as_ref().filter(|_| advertise).and_then(|_| announcement.advertise());
    Ok(Launched { host, config, fingerprint: identity.fingerprint(), pairing, announcement, advert })
}

/// The frontmost app's menu bar through Accessibility (EXT-3), if the config wants it.
fn menus_for(config: &HostConfig) -> Option<Arc<dyn crate::menus::MenuSource>> {
    if !config.menus {
        return None;
    }
    if !crate::menus::ax::is_trusted() {
        tracing::warn!("no Accessibility permission: clients see no menu bar until it is granted");
    }
    match crate::menus::ax::AxMenus::start() {
        Ok(menus) => Some(menus as Arc<dyn crate::menus::MenuSource>),
        Err(e) => {
            tracing::warn!(error = %e, "could not start the menu reader");
            None
        }
    }
}
