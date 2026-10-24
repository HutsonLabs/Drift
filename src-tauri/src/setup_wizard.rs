//! "Set Up a Linux Machine…" (ADR `GNOME-WIZARD-headless-setup`): signs in over SSH with the
//! device key (`drift-ssh`), runs `host/drift-host-setup.sh` with the wizard's options
//! (`drift_core::setup`), streams its lines, and hands back the connections it leaves.
//! Framework-free so it is tested against `drift_testkit::ssh_server`; the Tauri commands in
//! [`crate::commands`] are thin wrappers.

use std::path::PathBuf;

use drift_core::ConnectionProfile;
use drift_core::setup::{Outcome, RunMode, SetupIssue, SetupLine, SetupOptions, Sudo};
use drift_ssh::{Diagnosis, KeyStore, SshError, Target, Trust};
use serde::{Deserialize, Serialize};

/// The setup script the app runs on the machine (the same file `host/drift-gnome-setup` uploads).
pub const SETUP_SCRIPT: &str = include_str!("../../host/drift-host-setup.sh");

/// Where signing in got to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SignIn {
    /// Signed in; `sudo` says whether the run needs the account's password.
    Ready {
        /// How the run gets root.
        sudo: Sudo,
    },
    /// Not signed in, and why (the wizard offers the fix).
    Problem {
        /// What went wrong.
        diagnosis: Diagnosis,
    },
}

/// Why a run did not start or did not finish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SetupError {
    /// Sign in first.
    #[error("not signed in to the machine")]
    NotSignedIn,
    /// The options need fixing.
    #[error("the setup options are not valid")]
    Invalid {
        /// The fields to fix.
        issues: Vec<SetupIssue>,
    },
    /// The account needs its password for sudo.
    #[error("sudo needs the account's password")]
    SudoPasswordNeeded,
    /// sudo did not accept the password.
    #[error("sudo did not accept the password")]
    SudoPasswordRejected,
    /// The connection failed.
    #[error("{diagnosis}")]
    Ssh {
        /// What went wrong.
        diagnosis: Diagnosis,
    },
}

/// What a finished run reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SetupResult {
    /// How it ended.
    pub outcome: Outcome,
    /// The script's `error:` lines.
    pub errors: Vec<String>,
    /// Changes pending (check) or applied (apply), from the summary line.
    pub changes: Option<u32>,
    /// After a successful apply: the unsaved connections, certificates pinned.
    pub profiles: Vec<ConnectionProfile>,
}

/// The wizard's SSH session and key store.
pub struct SetupService {
    keys: KeyStore,
    comment: String,
    script: String,
}

impl std::fmt::Debug for SetupService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupService").field("keys", &self.keys).finish_non_exhaustive()
    }
}

impl SetupService {
    /// A service keeping its SSH files in `dir`, labelling the device key `comment`, and
    /// running `script` (normally [`SETUP_SCRIPT`]).
    pub fn new(dir: impl Into<PathBuf>, comment: &str, script: &str) -> Result<Self, SshError> {
        Ok(Self { keys: KeyStore::open(dir)?, comment: comment.to_owned(), script: script.to_owned() })
    }

    /// The wizard's defaults for `account` on `host`, with a freshly generated RDP password.
    pub fn defaults(&self, host: &str, account: &str) -> SetupOptions {
        SetupOptions::defaults(host, account)
    }

    /// Signs in to `target` with the device key, trusting a new host key only with
    /// [`Trust::TrustNew`].
    pub async fn connect(&self, target: Target, trust: Trust) -> SignIn {
        let _ = (target, trust);
        SignIn::Problem { diagnosis: Diagnosis::Failed { detail: "not implemented".into() } }
    }

    /// Installs the device key with the account `password` (used once), then signs in.
    pub async fn install_key(&self, target: Target, password: &str) -> SignIn {
        let _ = (target, password);
        SignIn::Problem { diagnosis: Diagnosis::Failed { detail: "not implemented".into() } }
    }

    /// Forgets the recorded host key of `host:port`.
    pub fn forget_host(&self, host: &str, port: u16) -> Result<(), SshError> {
        self.keys.forget_host(host, port)
    }

    /// The device key's public line (for "Copy Public Key").
    pub fn public_key(&self) -> Result<String, SshError> {
        self.keys.public_key(&self.comment)
    }

    /// Runs the setup script in `mode` on the signed-in machine, calling `on_line` for each
    /// output line.
    pub async fn run(
        &self,
        options: &SetupOptions,
        mode: RunMode,
        sudo_password: Option<&str>,
        on_line: &mut (dyn FnMut(SetupLine) + Send),
    ) -> Result<SetupResult, SetupError> {
        let _ = (options, mode, sudo_password, on_line, &self.script);
        Err(SetupError::NotSignedIn)
    }

    /// Ends the SSH session.
    pub async fn disconnect(&self) {}
}
