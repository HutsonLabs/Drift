//! Setting up a Linux machine from inside Drift (ADR `GNOME-WIZARD-headless-setup`): the
//! wizard's options, how they become a run of `host/drift-host-setup.sh` over SSH, the script's
//! output lines, and the connections the setup leaves behind. Pure; the SSH transport is
//! `drift-ssh`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

use crate::profile::{CertFingerprint, ConnectMode, ConnectionProfile};

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
    /// Open them only to this network (`10.0.0.0/21`, `fd00::/64`).
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
    /// An IP address is its own name; a host name's first label is the name otherwise.
    pub fn defaults(host: &str, account: &str) -> Self {
        let host = host.trim();
        let name = if host.parse::<IpAddr>().is_ok() {
            host.to_owned()
        } else {
            host.split('.').next().unwrap_or(host).to_owned()
        };
        Self {
            name,
            account: account.to_owned(),
            port: DEFAULT_HEADLESS_PORT,
            rdp_username: account.to_owned(),
            rdp_password: None,
            remote_login: false,
            install_desktop: true,
            open_firewall: true,
            allow_from: None,
        }
    }

    /// Every problem with the options, in field order.
    pub fn validate(&self) -> Result<(), Vec<SetupIssue>> {
        let mut issues = Vec::new();
        if self.name.trim().is_empty() {
            issues.push(SetupIssue::Name);
        }
        if !is_account_name(&self.account) {
            issues.push(SetupIssue::Account);
        }
        if self.port == 0 {
            issues.push(SetupIssue::Port);
        } else if self.remote_login && self.port == REMOTE_LOGIN_PORT {
            issues.push(SetupIssue::PortTakenByRemoteLogin);
        }
        if self.rdp_username.trim().is_empty() || self.rdp_username.chars().any(char::is_control) {
            issues.push(SetupIssue::RdpUsername);
        }
        if let Some(password) = &self.rdp_password
            && (password.chars().count() < 8 || password.chars().any(char::is_control))
        {
            issues.push(SetupIssue::RdpPassword);
        }
        if self.allow_from.as_deref().is_some_and(|net| !is_network(net)) {
            issues.push(SetupIssue::AllowFrom);
        }
        if issues.is_empty() { Ok(()) } else { Err(issues) }
    }

    /// The arguments for `drift-host-setup.sh`.
    pub fn script_args(&self, mode: RunMode) -> Vec<String> {
        let mut args = vec![match mode {
            RunMode::Check => "--check".to_owned(),
            RunMode::Apply => "--apply".to_owned(),
        }];
        if self.install_desktop {
            args.push("--desktop".into());
        }
        args.push("--install".into());
        if self.open_firewall {
            args.push("--firewall".into());
            if let Some(net) = &self.allow_from {
                args.push("--allow-from".into());
                args.push(net.clone());
            }
        }
        args.push("--headless".into());
        args.push(format!("{}:{}:persistent", self.account, self.port));
        if self.remote_login {
            args.push("--system".into());
        }
        args
    }

    /// The `export NAME='value'` lines that hand the credentials to the script; empty when the
    /// password is kept.
    pub fn env_exports(&self) -> String {
        let Some(password) = &self.rdp_password else {
            return String::new();
        };
        let suffix = env_suffix(&self.account);
        let user = shell_quote(&self.rdp_username);
        let password = shell_quote(password);
        let mut out = format!(
            "export DRIFT_HEADLESS_RDP_USER_{suffix}={user}\nexport DRIFT_HEADLESS_RDP_PASS_{suffix}={password}\n"
        );
        if self.remote_login {
            out.push_str(&format!(
                "export DRIFT_SYSTEM_RDP_USER={user}\nexport DRIFT_SYSTEM_RDP_PASS={password}\n"
            ));
        }
        out
    }
}

/// A Linux account name as `useradd` accepts it by default (`[a-z_][a-z0-9_-]*`, at most 32).
fn is_account_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 32
        && bytes.next().is_some_and(|b| b.is_ascii_lowercase() || b == b'_')
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// An IPv4 or IPv6 address with an optional prefix length (what `--allow-from` accepts).
fn is_network(net: &str) -> bool {
    let (addr, prefix) = match net.split_once('/') {
        Some((addr, prefix)) => (addr, Some(prefix)),
        None => (net, None),
    };
    let max = if addr.parse::<Ipv4Addr>().is_ok() {
        32
    } else if addr.parse::<Ipv6Addr>().is_ok() {
        128
    } else {
        return false;
    };
    prefix.is_none_or(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok_and(|p| p <= max))
}

/// The script's environment variable suffix for an account (`drift-test2` → `DRIFT_TEST2`).
fn env_suffix(account: &str) -> String {
    account.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect()
}

/// `s` as one single-quoted shell word.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The command run over SSH: `sudo` (as `how`) running bash on the script from stdin.
pub fn remote_command(how: Sudo, args: &[String]) -> String {
    let sudo = match how {
        Sudo::Passwordless => "sudo -n",
        // -k: a cached timestamp would make sudo skip the password line, and bash would then
        // run it as a command.
        Sudo::Password => "sudo -k -S -p ''",
    };
    let mut command = format!("{sudo} bash -s --");
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command
}

/// What the command reads on stdin: the sudo password line (with [`Sudo::Password`]), the
/// credential exports, then the script.
pub fn remote_stdin(sudo_password: Option<&str>, options: &SetupOptions, script: &str) -> String {
    let mut out = String::new();
    if let Some(password) = sudo_password {
        out.push_str(password);
        out.push('\n');
    }
    out.push_str(&options.env_exports());
    out.push_str(script);
    out
}

/// The characters of a generated password, in byte order.
const PASSWORD_ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// A password of [`PASSWORD_LEN`] letters and digits from `random` bytes (unbiased: bytes that
/// would favour some characters are skipped). `None` when `random` runs out first.
pub fn generate_password(random: &[u8]) -> Option<String> {
    // 248 = 4 × 62: bytes below it map evenly onto the alphabet.
    let password: String = random
        .iter()
        .filter(|&&b| b < 248)
        .take(PASSWORD_LEN)
        .map(|&b| char::from(PASSWORD_ALPHABET[usize::from(b) % PASSWORD_ALPHABET.len()]))
        .collect();
    (password.len() == PASSWORD_LEN).then_some(password)
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
    let line = line.strip_suffix('\r').unwrap_or(line);
    let output = || SetupLine::Output { text: line.to_owned() };
    if let Some(rest) = line.strip_prefix("drift-host-setup: ") {
        return match rest {
            "mode=check" => SetupLine::Mode { mode: RunMode::Check },
            "mode=apply" => SetupLine::Mode { mode: RunMode::Apply },
            _ => parse_summary(rest).unwrap_or_else(output),
        };
    }
    if let Some(rest) = line.strip_prefix("fingerprint: ") {
        let mut words = rest.split_whitespace();
        return match (words.next(), words.next().map(str::parse::<CertFingerprint>), words.next()) {
            (Some(owner), Some(Ok(fingerprint)), None) => {
                SetupLine::Fingerprint { owner: owner.to_owned(), fingerprint }
            }
            _ => output(),
        };
    }
    if let Some((tag, rest)) = line.split_once(':') {
        let text = rest.trim_start().to_owned();
        match tag {
            "ok" => return SetupLine::Ok { text },
            "would" => return SetupLine::Would { text },
            "change" => return SetupLine::Change { text },
            "skip" => return SetupLine::Skip { text },
            "wait" => return SetupLine::Wait { text },
            "error" => return SetupLine::Error { text },
            _ => {}
        }
    }
    if line.starts_with(char::is_whitespace)
        && let Some(command) = line.trim_start().strip_prefix("$ ")
    {
        return SetupLine::Command { text: command.to_owned() };
    }
    output()
}

/// `N change(s) pending|applied, M error(s)`.
fn parse_summary(rest: &str) -> Option<SetupLine> {
    let (changes, rest) = rest.split_once(" change(s) ")?;
    let rest = rest.strip_prefix("pending, ").or_else(|| rest.strip_prefix("applied, "))?;
    let errors = rest.strip_suffix(" error(s)")?;
    Some(SetupLine::Summary { changes: changes.parse().ok()?, errors: errors.parse().ok()? })
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
        let parsed = parse_line(line);
        match &parsed {
            SetupLine::Fingerprint { owner, fingerprint } => {
                self.fingerprints.retain(|(o, _)| o != owner);
                self.fingerprints.push((owner.clone(), *fingerprint));
            }
            SetupLine::Error { text } => self.errors.push(text.clone()),
            SetupLine::Summary { changes, errors } => self.summary = Some((*changes, *errors)),
            _ => {}
        }
        parsed
    }

    /// The certificate reported for `owner` (`system` or an account).
    pub fn fingerprint(&self, owner: &str) -> Option<CertFingerprint> {
        self.fingerprints.iter().find(|(o, _)| o == owner).map(|(_, fp)| *fp)
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
    match (mode, exit) {
        (RunMode::Check, Some(0)) => Outcome::Converged,
        (RunMode::Check, Some(2)) => Outcome::ChangesPending,
        (RunMode::Apply, Some(0)) => Outcome::Applied,
        _ => Outcome::Failed,
    }
}

/// The unsaved connections a finished setup leaves: the headless session, and Remote Login
/// when it was set up, each with the certificate the run reported pinned.
pub fn connection_profiles(
    options: &SetupOptions,
    host: &str,
    report: &SetupReport,
) -> Vec<ConnectionProfile> {
    let mut headless = ConnectionProfile::new(options.name.trim(), host, ConnectMode::Headless);
    headless.port = options.port;
    headless.rdp_username.clone_from(&options.rdp_username);
    headless.cert_pin = report.fingerprint(&options.account);
    let mut profiles = vec![headless];
    if options.remote_login {
        let mut login =
            ConnectionProfile::new(format!("{} login", options.name.trim()), host, ConnectMode::RemoteLogin);
        login.port = REMOTE_LOGIN_PORT;
        login.rdp_username.clone_from(&options.rdp_username);
        login.cert_pin = report.fingerprint("system");
        profiles.push(login);
    }
    profiles
}
