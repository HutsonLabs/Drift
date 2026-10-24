//! In-app SSH for setting up a Linux machine (ADR `GNOME-WIZARD-headless-setup`), the way
//! term.hut connects on iPhone: russh in-process, one ed25519 device key in the app's own
//! directory, a known_hosts file next to it, strict host keys with an explicit "trust", and the
//! account password used once to install the device key. Nothing calls a system tool or reads
//! `~/.ssh`, so it works inside the App Sandbox and on iOS.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How long connecting and signing in may take.
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A local failure of the key store.
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    /// Reading or writing the store's files.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The device key could not be made or read.
    #[error("device key: {0}")]
    Key(String),
}

/// The app's SSH files: the device key (`id_ed25519`, mode 0600) and `known_hosts`.
#[derive(Debug, Clone)]
pub struct KeyStore {
    dir: PathBuf,
}

impl KeyStore {
    /// Uses `dir` (created with mode 0700) for the store.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, SshError> {
        Ok(Self { dir: dir.into() })
    }

    /// The device key's OpenSSH public line; the key is made on first use, labelled `comment`.
    pub fn public_key(&self, comment: &str) -> Result<String, SshError> {
        let _ = comment;
        Err(SshError::Key("not implemented".into()))
    }

    /// Forgets the recorded host key of `host:port`.
    pub fn forget_host(&self, host: &str, port: u16) -> Result<(), SshError> {
        let _ = (host, port);
        Ok(())
    }

    /// The store's known_hosts file.
    pub fn known_hosts(&self) -> PathBuf {
        self.dir.join("known_hosts")
    }
}

/// The machine and account to sign in to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Target {
    /// Host name or address.
    pub host: String,
    /// SSH port.
    pub port: u16,
    /// Account.
    pub user: String,
}

/// What to do with a host key that is not recorded yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum Trust {
    /// Stop and report it.
    KnownOnly,
    /// Record it (the wizard's "Trust and Connect").
    TrustNew,
}

/// Why connecting, signing in or running a command failed, with the one fix the wizard offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Diagnosis {
    /// No TCP connection (refused, no route, name not found).
    #[error("cannot reach the machine: {detail}")]
    Unreachable {
        /// The system's reason.
        detail: String,
    },
    /// Connecting or signing in took longer than [`CONNECT_TIMEOUT`].
    #[error("the machine did not answer in time")]
    TimedOut,
    /// The host key is not recorded yet.
    #[error("unknown host key {fingerprint}")]
    HostKeyUnknown {
        /// `SHA256:…`.
        fingerprint: String,
    },
    /// The host key differs from the recorded one.
    #[error("the host key changed to {fingerprint}")]
    HostKeyChanged {
        /// `SHA256:…` of the new key.
        fingerprint: String,
    },
    /// The account does not accept the device key.
    #[error("the account does not accept Drift's key")]
    KeyNotAuthorized {
        /// Whether a password login is offered (to install the key with).
        offers_password: bool,
    },
    /// The password was wrong.
    #[error("the password was not accepted")]
    PasswordRejected,
    /// The server does not take passwords.
    #[error("the machine does not accept password logins")]
    PasswordNotOffered,
    /// The connection ended before the command did.
    #[error("the connection was lost")]
    ConnectionLost,
    /// Anything else.
    #[error("{detail}")]
    Failed {
        /// What happened.
        detail: String,
    },
}

/// A signed-in SSH connection.
pub struct Connection {
    _private: (),
}

/// Connects to `target` and signs in with the device key.
pub async fn connect(keys: &KeyStore, target: &Target, trust: Trust, comment: &str) -> Result<Connection, Diagnosis> {
    let _ = (keys, target, trust, comment);
    Err(Diagnosis::Failed { detail: "not implemented".into() })
}

/// Signs in once with `password` and appends the device key to the account's
/// `~/.ssh/authorized_keys` (idempotent). The password is not kept.
pub async fn install_device_key(keys: &KeyStore, target: &Target, password: &str, comment: &str) -> Result<(), Diagnosis> {
    let _ = (keys, target, password, comment);
    Err(Diagnosis::Failed { detail: "not implemented".into() })
}

impl Connection {
    /// Runs `command`, writes `stdin` and closes it, and calls `on_line` for every line of
    /// output (stdout and stderr, without the newline). Returns the exit status (`None` when the
    /// server sent none).
    pub async fn run(
        &self,
        command: &str,
        stdin: &[u8],
        on_line: &mut (dyn FnMut(&str) + Send),
    ) -> Result<Option<u32>, Diagnosis> {
        let _ = (command, stdin, on_line);
        Err(Diagnosis::Failed { detail: "not implemented".into() })
    }

    /// Whether the account can use sudo without a password (`sudo -n true`).
    pub async fn sudo_without_password(&self) -> Result<bool, Diagnosis> {
        Err(Diagnosis::Failed { detail: "not implemented".into() })
    }

    /// Ends the connection.
    pub async fn close(self) {}
}
