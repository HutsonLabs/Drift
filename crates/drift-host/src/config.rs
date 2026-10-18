//! The host's configuration file, `host.toml` in the host's config directory.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use drift_core::Size;
use serde::{Deserialize, Serialize};

/// File name of the configuration in the config directory.
pub const CONFIG_FILE: &str = "host.toml";

/// The RDP port. The listener binds it on loopback only unless the user widens `listen`.
pub const DEFAULT_PORT: u16 = 3389;

/// Settings the host reads at start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostConfig {
    /// Where the listener binds. Loopback by default: until pairing (HOST-8) exists, reaching the
    /// host from the network is an explicit choice.
    pub listen: SocketAddr,
    /// The account clients sign in with (NLA). Its password lives in the Keychain.
    pub username: String,
    /// The desktop size offered before a client sends its own layout.
    pub desktop: Size<u32>,
<<<<<<< HEAD
    /// Inject the client's keyboard and mouse into this Mac (HOST-6). Unset means automatic: on
    /// when the host has Accessibility, off (input logged only) when it has not.
    pub inject_input: Option<bool>,
=======
    /// Inject the client's keyboard and mouse into this Mac (HOST-6). Off in the skeleton.
    pub inject_input: bool,
    /// Give each client a virtual display at its own size (HOST-10). Falls back to the physical
    /// display when macOS does not offer one.
    pub virtual_display: bool,
>>>>>>> task/HOST-10-virtual-display
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_PORT)),
            username: "drift".to_owned(),
            desktop: Size::new(1920, 1080),
<<<<<<< HEAD
            inject_input: None,
=======
            inject_input: false,
            virtual_display: true,
>>>>>>> task/HOST-10-virtual-display
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

    /// Whether the listener is reachable from other machines.
    pub fn listens_beyond_loopback(&self) -> bool {
        !self.listen.ip().is_loopback()
    }
}

/// `~/Library/Application Support/com.hutsonlabs.drift.host`, or `None` without a home directory.
pub fn default_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/com.hutsonlabs.drift.host"))
}
