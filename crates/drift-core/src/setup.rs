//! Setting up a Linux machine from inside Drift (ADR `GNOME-WIZARD-headless-setup`): the
//! wizard's options, how they become a run of `host/drift-host-setup.sh` over SSH, the script's
//! output lines, and the connections the setup leaves behind. Pure; the SSH transport is
//! `drift-ssh`.

use serde::{Deserialize, Serialize};

use crate::profile::{CertFingerprint, ConnectionProfile};

/// The headless session's RDP port unless the user picks another (as `host/drift-gnome-setup`).
pub const DEFAULT_HEADLESS_PORT: u16 = 3390;
/// Remote Login's port (the system daemon), which a headless session cannot share.
pub const REMOTE_LOGIN_PORT: u16 = 3389;
/// Length of a generated RDP password.
pub const PASSWORD_LEN: usize = 24;

/// What the wizard sets up on one machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct SetupOptions {
    /// The connection's name in Drift.
    pub name: String,
    /// The Linux account that gets the persistent headless session.
    pub account: String,
    /// The headless session's RDP port.
    pub port: u16,
    /// The RDP user name.
    pub rdp_username: String,
    /// The RDP password to set; `None` keeps the credentials already on the machine.
    pub rdp_password: Option<String>,
    /// Also set up Remote Login (GNOME's login screen on port 3389).
    pub remote_login: bool,
    /// Install a minimal GNOME when GNOME Shell or GDM is missing.
    pub install_desktop: bool,
    /// Open the ports in ufw or firewalld.
    pub open_firewall: bool,
    /// Open them only to this network (`10.1.0.0/21`, `fd00::/64`).
    pub allow_from: Option<String>,
}

/// A setup option the wizard must ask about again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum SetupIssue {
    /// The connection needs a name.
    Name,
    /// Not a Linux account name.
    Account,
    /// Not a TCP port.
    Port,
    /// The headless port is Remote Login's.
    PortTakenByRemoteLogin,
    /// The RDP user name is empty or has control characters.
    RdpUsername,
    /// The RDP password is shorter than 8 characters or has control characters.
    RdpPassword,
    /// Not a network address with an optional prefix length.
    AllowFrom,
}

/// Whether a run only reports (`--check`) or changes the machine (`--apply`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum RunMode {
    /// Change nothing; list what would change.
    Check,
    /// Make the changes.
    Apply,
}

/// How the run gets root on the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum Sudo {
    /// The account has passwordless sudo (`sudo -n`).
    Passwordless,
    /// sudo reads the account's password from the first line of stdin (`sudo -S`).
    Password,
}

impl SetupOptions {
    /// The wizard's defaults for `account` on `host`.
    pub fn defaults(host: &str, account: &str) -> Self {
        let _ = (host, account);
        Self {
            name: String::new(),
            account: String::new(),
            port: 0,
            rdp_username: String::new(),
            rdp_password: None,
            remote_login: false,
            install_desktop: false,
            open_firewall: false,
            allow_from: None,
        }
    }

    /// Every problem with the options, in field order.
    pub fn validate(&self) -> Result<(), Vec<SetupIssue>> {
        Ok(())
    }

    /// The arguments for `drift-host-setup.sh`.
    pub fn script_args(&self, mode: RunMode) -> Vec<String> {
        let _ = mode;
        Vec::new()
    }

    /// The `export NAME='value'` lines that hand the credentials to the script; empty when the
    /// password is kept.
    pub fn env_exports(&self) -> String {
        String::new()
    }
}

/// `s` as one single-quoted shell word.
pub fn shell_quote(s: &str) -> String {
    s.to_owned()
}

/// The command run over SSH: `sudo` (as `how`) running bash on the script from stdin.
pub fn remote_command(how: Sudo, args: &[String]) -> String {
    let _ = (how, args);
    String::new()
}

/// What the command reads on stdin: the sudo password line (with [`Sudo::Password`]), the
/// credential exports, then the script.
pub fn remote_stdin(sudo_password: Option<&str>, options: &SetupOptions, script: &str) -> String {
    let _ = (sudo_password, options, script);
    String::new()
}

/// A password of [`PASSWORD_LEN`] letters and digits from `random` bytes (unbiased: bytes that
/// would favour some characters are skipped). `None` when `random` runs out first.
pub fn generate_password(random: &[u8]) -> Option<String> {
    let _ = random;
    None
}

/// One line of `drift-host-setup.sh` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SetupLine {
    /// `drift-host-setup: mode=…`.
    Mode {
        /// The run's mode.
        mode: RunMode,
    },
    /// `ok:` — already as wanted.
    Ok {
        /// What is fine.
        text: String,
    },
    /// `would:` — `--check` would change this.
    Would {
        /// What would change.
        text: String,
    },
    /// The `$ command` line under a `would:`.
    Command {
        /// The command.
        text: String,
    },
    /// `change:` — `--apply` is changing this.
    Change {
        /// What changes.
        text: String,
    },
    /// `skip:` — not checked yet.
    Skip {
        /// What was skipped.
        text: String,
    },
    /// `wait:` — waiting for something to start.
    Wait {
        /// What it waits for.
        text: String,
    },
    /// `error:`.
    Error {
        /// The error.
        text: String,
    },
    /// `fingerprint: OWNER aa:bb:…` — a daemon's certificate.
    Fingerprint {
        /// `system` or the headless account.
        owner: String,
        /// The certificate's SHA-256.
        fingerprint: CertFingerprint,
    },
    /// `drift-host-setup: N change(s) …, M error(s)`.
    Summary {
        /// Changes pending or applied.
        changes: u32,
        /// Errors.
        errors: u32,
    },
    /// Anything else (package manager output).
    Output {
        /// The line.
        text: String,
    },
}

/// Parses one output line (a trailing `\r` from a PTY is ignored).
pub fn parse_line(line: &str) -> SetupLine {
    SetupLine::Output { text: line.to_owned() }
}

/// Everything a run reported, fed line by line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupReport {
    /// Fingerprints by owner, last one wins.
    pub fingerprints: Vec<(String, CertFingerprint)>,
    /// The `error:` lines.
    pub errors: Vec<String>,
    /// The summary line's counts, once seen.
    pub summary: Option<(u32, u32)>,
}

impl SetupReport {
    /// Parses and records one line, and returns it parsed.
    pub fn feed(&mut self, line: &str) -> SetupLine {
        parse_line(line)
    }

    /// The certificate reported for `owner` (`system` or an account).
    pub fn fingerprint(&self, owner: &str) -> Option<CertFingerprint> {
        let _ = owner;
        None
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// `--check`: nothing would change.
    Converged,
    /// `--check`: changes are pending.
    ChangesPending,
    /// `--apply` succeeded.
    Applied,
    /// The script reported errors, or the command did not finish.
    Failed,
}

/// The outcome of a run in `mode` that exited with `exit` (`None`: no exit status).
pub fn outcome(mode: RunMode, exit: Option<u32>) -> Outcome {
    let _ = (mode, exit);
    Outcome::Failed
}

/// The unsaved connections a finished setup leaves: the headless session, and Remote Login
/// when it was set up, each with the certificate the run reported pinned.
pub fn connection_profiles(options: &SetupOptions, host: &str, report: &SetupReport) -> Vec<ConnectionProfile> {
    let _ = (options, host, report);
    Vec::new()
}
