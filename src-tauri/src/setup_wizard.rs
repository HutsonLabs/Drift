//! "Set Up a Linux Machine…" (ADR `GNOME-WIZARD-headless-setup`): signs in over SSH with the
//! device key (`drift-ssh`), runs `host/drift-host-setup.sh` with the wizard's options
//! (`drift_core::setup`), streams its lines, and hands back the connections it leaves.
//! Framework-free so it is tested against `drift_testkit::ssh_server`; the Tauri commands in
//! [`crate::commands`] are thin wrappers.

use std::path::PathBuf;

use drift_core::ConnectionProfile;
use drift_core::setup::{
    Outcome, RunMode, SetupIssue, SetupLine, SetupOptions, SetupReport, Sudo, connection_profiles,
    generate_password, outcome, remote_command, remote_stdin,
};
use drift_ssh::{Connection, Diagnosis, KeyStore, SshError, Target, Trust};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize as _, Zeroizing};

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

/// The machine the wizard is signed in to.
struct Signed {
    target: Target,
    conn: Connection,
    sudo: Sudo,
}

/// The wizard's SSH session and key store.
pub struct SetupService {
    keys: KeyStore,
    comment: String,
    script: String,
    current: tokio::sync::Mutex<Option<Signed>>,
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
        Ok(Self {
            keys: KeyStore::open(dir)?,
            comment: comment.to_owned(),
            script: script.to_owned(),
            current: tokio::sync::Mutex::new(None),
        })
    }

    /// The wizard's defaults for `account` on `host`, with a freshly generated RDP password.
    pub fn defaults(&self, host: &str, account: &str) -> SetupOptions {
        let mut options = SetupOptions::defaults(host, account);
        let mut random = [0u8; 64];
        // 64 bytes give 24 accepted ones but for a chance far below 1e-20; draw again then.
        while options.rdp_password.is_none() {
            if getrandom::fill(&mut random).is_err() {
                break;
            }
            options.rdp_password = generate_password(&random);
        }
        random.zeroize();
        options
    }

    /// Signs in to `target` with the device key, trusting a new host key only with
    /// [`Trust::TrustNew`], and finds out whether sudo needs the account's password.
    pub async fn connect(&self, target: Target, trust: Trust) -> SignIn {
        self.disconnect().await;
        let conn = match drift_ssh::connect(&self.keys, &target, trust, &self.comment).await {
            Ok(conn) => conn,
            Err(diagnosis) => return SignIn::Problem { diagnosis },
        };
        let sudo = match conn.sudo_without_password().await {
            Ok(true) => Sudo::Passwordless,
            Ok(false) => Sudo::Password,
            Err(diagnosis) => return SignIn::Problem { diagnosis },
        };
        *self.current.lock().await = Some(Signed { target, conn, sudo });
        SignIn::Ready { sudo }
    }

    /// Installs the device key with the account `password` (used once), then signs in.
    pub async fn install_key(&self, target: Target, password: &str) -> SignIn {
        match drift_ssh::install_device_key(&self.keys, &target, password, &self.comment).await {
            Ok(()) => self.connect(target, Trust::KnownOnly).await,
            Err(diagnosis) => SignIn::Problem { diagnosis },
        }
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
        options.validate().map_err(|issues| SetupError::Invalid { issues })?;
        let current = self.current.lock().await;
        let signed = current.as_ref().ok_or(SetupError::NotSignedIn)?;
        let sudo_password = match signed.sudo {
            Sudo::Passwordless => None,
            Sudo::Password => {
                Some(sudo_password.filter(|p| !p.is_empty()).ok_or(SetupError::SudoPasswordNeeded)?)
            }
        };
        let command = remote_command(signed.sudo, &options.script_args(mode));
        let stdin = Zeroizing::new(remote_stdin(sudo_password, options, &self.script));

        let mut report = SetupReport::default();
        let (mut started, mut sudo_refused) = (false, false);
        let status = signed
            .conn
            .run(&command, stdin.as_bytes(), &mut |line| {
                let parsed = report.feed(line);
                if matches!(parsed, SetupLine::Mode { .. }) {
                    started = true;
                }
                // sudo's own complaints come before the script says anything.
                if !started
                    && (line.contains("incorrect password attempt") || line.starts_with("Sorry, try again"))
                {
                    sudo_refused = true;
                    return;
                }
                on_line(parsed);
            })
            .await
            .map_err(|diagnosis| SetupError::Ssh { diagnosis })?;
        if sudo_refused && !started {
            return Err(SetupError::SudoPasswordRejected);
        }
        let outcome = outcome(mode, status);
        let profiles = if outcome == Outcome::Applied {
            connection_profiles(options, &signed.target.host, &report)
        } else {
            Vec::new()
        };
        Ok(SetupResult {
            outcome,
            errors: report.errors.clone(),
            changes: report.summary.map(|(c, _)| c),
            profiles,
        })
    }

    /// Ends the SSH session.
    pub async fn disconnect(&self) {
        if let Some(signed) = self.current.lock().await.take() {
            signed.conn.close().await;
        }
    }
}
