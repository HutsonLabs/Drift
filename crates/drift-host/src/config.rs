//! The host's configuration file, `host.toml` in the host's config directory.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use drift_core::Size;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// File name of the configuration in the config directory.
pub const CONFIG_FILE: &str = "host.toml";

/// The RDP port.
pub const DEFAULT_PORT: u16 = 3389;

/// Settings the host reads at start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostConfig {
    /// Where the listener binds: every interface by default. It only applies with
    /// [`Self::pairing`]; without pairing the host stays on loopback ([`Self::effective_listen`]).
    pub listen: SocketAddr,
    /// The account clients sign in with (NLA). Its password lives in the Keychain.
    pub username: String,
    /// The desktop size offered before a client sends its own layout.
    pub desktop: Size<u32>,
    /// Inject the client's keyboard and mouse into this Mac (HOST-6). Unset means automatic: on
    /// when the host has Accessibility, off (input logged only) when it has not.
    pub inject_input: Option<bool>,
    /// Give each client a virtual display at its own size (HOST-10). Falls back to the physical
    /// display when macOS does not offer one.
    pub virtual_display: bool,
    /// Device pairing (HOST-8): only devices that paired with a code shown on this Mac are served.
    /// On by default; turning it off keeps the host on loopback.
    pub pairing: bool,
    /// The host's stable id in Bonjour adverts and pairing codes (created on first start).
    pub host_id: Uuid,
    /// The name devices see; empty means the Mac's name.
    pub name: String,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from((Ipv4Addr::UNSPECIFIED, DEFAULT_PORT)),
            username: "drift".to_owned(),
            desktop: Size::new(1920, 1080),
            inject_input: None,
            virtual_display: true,
            pairing: true,
            host_id: Uuid::nil(),
            name: String::new(),
        }
    }
}

/// Reading or writing `host.toml` failed.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read or written.
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    /// The file is not a valid configuration.
    #[error("{0} is not a valid host configuration: {1}")]
    Parse(PathBuf, String),
}

impl HostConfig {
    /// Reads `dir/host.toml`; a missing file is the default configuration.
    pub fn load(dir: &Path) -> Result<Self, ConfigError> {
        let path = dir.join(CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| ConfigError::Parse(path, e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(ConfigError::Io(path, e)),
        }
    }

    /// Writes `dir/host.toml`, creating `dir`.
    pub fn save(&self, dir: &Path) -> Result<(), ConfigError> {
        let path = dir.join(CONFIG_FILE);
        std::fs::create_dir_all(dir).map_err(|e| ConfigError::Io(dir.to_owned(), e))?;
        let text = toml::to_string(self).map_err(|e| ConfigError::Parse(path.clone(), e.to_string()))?;
        std::fs::write(&path, text).map_err(|e| ConfigError::Io(path, e))
    }

    /// Where the listener really binds: [`Self::listen`] with pairing, loopback (same port)
    /// without it (ADR `IOS-0` Consequences: the host is reachable from the network only when
    /// unpaired devices are refused).
    pub fn effective_listen(&self) -> SocketAddr {
        if self.pairing { self.listen } else { SocketAddr::from((Ipv4Addr::LOCALHOST, self.listen.port())) }
    }

    /// Whether the listener is reachable from other machines.
    pub fn listens_beyond_loopback(&self) -> bool {
        !self.effective_listen().ip().is_loopback()
    }
}

/// `~/Library/Application Support/com.hutsonlabs.drift.host`, or `None` without a home directory.
pub fn default_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/com.hutsonlabs.drift.host"))
}
