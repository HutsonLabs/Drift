//! `cargo xtask host-setup-check` (task M0-4).
//!
//! Runs `host/drift-host-report.sh` on the GNOME host over SSH (read-only), parses its
//! `### <section>` report and checks ports, daemons, the headless session and certificates
//! against [`Expectations`]. Parsing and evaluation are pure and unit-tested on reports
//! captured from the homelab host (`xtask/tests/data/host/`).

use std::str::FromStr;

use anyhow::{Context, Result, bail};

/// The report script, run on the host with `bash -s -- <specs>`.
pub const REPORT_SCRIPT: &str = include_str!("../../host/drift-host-report.sh");

/// A headless daemon Drift expects: `USER:PORT[:persistent]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessSpec {
    /// Linux user owning the headless daemon.
    pub user: String,
    /// Fixed RDP port.
    pub port: u16,
    /// Whether `gnome-headless-session@USER.service` must be enabled and running.
    pub persistent: bool,
}

impl FromStr for HeadlessSpec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let mut parts = s.split(':');
        let user = parts.next().unwrap_or_default();
        let port = parts.next().context("expected USER:PORT[:persistent]")?;
        let persistent = match parts.next() {
            None => false,
            Some("persistent") => true,
            Some(other) => bail!("unknown flag {other:?} (expected `persistent`)"),
        };
        if parts.next().is_some() {
            bail!("expected USER:PORT[:persistent], got {s:?}");
        }
        if user.is_empty() || !user.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            bail!("bad user name in {s:?}");
        }
        let port: u16 = port.parse().with_context(|| format!("bad port in {s:?}"))?;
        if port == 0 {
            bail!("port 0 in {s:?}");
        }
        Ok(Self { user: user.to_owned(), port, persistent })
    }
}

impl std::fmt::Display for HeadlessSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.user, self.port)?;
        if self.persistent {
            write!(f, ":persistent")?;
        }
        Ok(())
    }
}

/// What the host must look like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expectations {
    /// Remote Login (system daemon) port.
    pub system_port: u16,
    /// Whether Remote Login should be on (`false` checks that it is off: `drift-host-setup.sh
    /// --no-system`, a headless-only machine).
    pub system: bool,
    /// Headless daemons.
    pub headless: Vec<HeadlessSpec>,
}

impl Default for Expectations {
    /// The homelab test host (plan §5.2).
    fn default() -> Self {
        Self {
            system_port: 3389,
            system: true,
            headless: vec![
                HeadlessSpec { user: "drifttest".into(), port: 3391, persistent: false },
                HeadlessSpec { user: "drifttest2".into(), port: 3392, persistent: true },
            ],
        }
    }
}

/// One `### name args…` block of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Section name (`ss-ltnp`, `grdctl-headless`, …).
    pub name: String,
    /// Words after the name.
    pub args: Vec<String>,
    /// Body lines.
    pub body: Vec<String>,
}

/// Parsed `grdctl status` output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrdStatus {
    /// `Overall: Unit status: active`.
    pub unit_active: bool,
    /// `RDP: Status: enabled|disabled`.
    pub enabled: Option<bool>,
    /// Configured port.
    pub port: Option<u16>,
    /// `Negotiate port: yes|no` (absent for the system daemon).
    pub negotiate_port: Option<bool>,
    /// TLS certificate path, if set.
    pub tls_cert: Option<String>,
    /// TLS key path, if set.
    pub tls_key: Option<String>,
    /// TLS SHA-256 fingerprint (`aa:bb:…`), if set.
    pub tls_fingerprint: Option<String>,
    /// Username is set (`(hidden)`).
    pub username_set: bool,
    /// Password is set (`(hidden)`).
    pub password_set: bool,
}

/// A listening TCP socket from `ss -ltnp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    /// Local port.
    pub port: u16,
    /// Process name (as truncated by `ss`).
    pub process: String,
    /// Process id.
    pub pid: u32,
}

/// A process line from `ps -o pid=,user:32=,args=`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    /// Process id.
    pub pid: u32,
    /// User name.
    pub user: String,
    /// Command line.
    pub args: String,
}

/// `systemctl is-active` / `is-enabled` answers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitState {
    /// `is-active` output (`active`, `inactive`, …).
    pub active: String,
    /// `is-enabled` output (`enabled`, `disabled`, …).
    pub enabled: String,
}

impl UnitState {
    fn from_body(body: &[String]) -> Self {
        let mut it = body.iter().map(|l| l.trim().to_owned());
        Self { active: it.next().unwrap_or_default(), enabled: it.next().unwrap_or_default() }
    }

    /// Enabled and running.
    pub fn is_up(&self) -> bool {
        self.active == "active" && self.enabled == "enabled"
    }
}

/// The parsed report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostReport {
    /// Listening sockets.
    pub listeners: Vec<Listener>,
    /// g-r-d daemon processes.
    pub processes: Vec<Proc>,
    /// System units by name.
    pub units: Vec<(String, UnitState)>,
    /// `systemctl show -p DynamicUser -p User -p Restart` per unit.
    pub unit_props: Vec<(String, Vec<(String, String)>)>,
    /// User units: (user, unit, state).
    pub user_units: Vec<(String, String, UnitState)>,
    /// System daemon status.
    pub system: Option<GrdStatus>,
    /// Headless daemon status per user.
    pub headless: Vec<(String, GrdStatus)>,
    /// Certificate state per owner (`system` or a user): `valid`, `expired`, `unset`, `missing …`.
    pub certs: Vec<(String, String)>,
    /// Whether the report ended with `### end` (not truncated).
    pub complete: bool,
}

/// One check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// What was checked.
    pub name: String,
    /// Passed?
    pub ok: bool,
    /// Observed value / reason.
    pub detail: String,
}

/// Splits a report into sections (lines before the first `###` are ignored).
pub fn split_sections(report: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for line in report.lines() {
        if let Some(header) = line.strip_prefix("### ") {
            let mut words = header.split_whitespace().map(str::to_owned);
            let name = words.next().unwrap_or_default();
            out.push(Section { name, args: words.collect(), body: Vec::new() });
        } else if let Some(cur) = out.last_mut() {
            cur.body.push(line.to_owned());
        }
    }
    out
}

/// A `grdctl status` value meaning "not configured".
fn unset(v: &str) -> bool {
    v.is_empty() || v == "(null)" || v == "(empty)"
}

/// Parses `grdctl status` output.
pub fn parse_grdctl_status(text: &str) -> GrdStatus {
    let mut st = GrdStatus::default();
    let set = |v: &str| (!unset(v)).then(|| v.to_owned());
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once(':') else { continue };
        let value = value.trim();
        match key.trim() {
            "Unit status" => st.unit_active = value == "active",
            "Status" => {
                st.enabled = match value {
                    "enabled" => Some(true),
                    "disabled" => Some(false),
                    _ => None,
                }
            }
            "Port" => st.port = value.parse().ok(),
            "Negotiate port" => st.negotiate_port = Some(value == "yes"),
            "TLS certificate" => st.tls_cert = set(value),
            "TLS key" => st.tls_key = set(value),
            "TLS fingerprint" => st.tls_fingerprint = set(value),
            "Username" => st.username_set = value == "(hidden)",
            "Password" => st.password_set = value == "(hidden)",
            _ => {}
        }
    }
    st
}

/// Parses `ss -ltnp` output.
pub fn parse_ss_ltnp(text: &str) -> Vec<Listener> {
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.first() != Some(&"LISTEN") || cols.len() < 6 {
            continue;
        }
        let Some(port) = cols[3].rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()) else { continue };
        let users = cols[5..].join(" ");
        // users:(("name",pid=123,fd=4),("other",pid=1,fd=5))
        for proc_ in users.split("((").skip(1).flat_map(|s| s.split("),(")) {
            let mut name = None;
            let mut pid = None;
            for field in proc_.trim_end_matches("))").split(',') {
                if let Some(n) = field.strip_prefix('"').and_then(|f| f.strip_suffix('"')) {
                    name = Some(n.to_owned());
                } else if let Some(p) = field.strip_prefix("pid=") {
                    pid = p.parse().ok();
                }
            }
            if let (Some(process), Some(pid)) = (name, pid) {
                out.push(Listener { port, process, pid });
            }
        }
    }
    out
}

/// Parses `ps -o pid=,user:32=,args=` output.
pub fn parse_ps(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let user = it.next()?.to_owned();
            let args = it.collect::<Vec<_>>().join(" ");
            Some(Proc { pid, user, args })
        })
        .collect()
}

/// Parses a full report.
pub fn parse_report(report: &str) -> Result<HostReport> {
    let sections = split_sections(report);
    if sections.is_empty() {
        bail!("empty host report (did the SSH command fail?)");
    }
    let mut r = HostReport::default();
    for s in sections {
        let body = s.body.join("\n");
        let arg = |i: usize| s.args.get(i).cloned().unwrap_or_default();
        match s.name.as_str() {
            "ss-ltnp" => r.listeners = parse_ss_ltnp(&body),
            "ps" => r.processes = parse_ps(&body),
            "unit" => r.units.push((arg(0), UnitState::from_body(&s.body))),
            "unit-props" => {
                let props = s
                    .body
                    .iter()
                    .filter_map(|l| l.trim().split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned())))
                    .collect();
                r.unit_props.push((arg(0), props));
            }
            "user-unit" => r.user_units.push((arg(0), arg(1), UnitState::from_body(&s.body))),
            "grdctl-system" => r.system = Some(parse_grdctl_status(&body)),
            "grdctl-headless" => r.headless.push((arg(0), parse_grdctl_status(&body))),
            "cert" => r.certs.push((arg(0), s.body.first().map(|l| l.trim().to_owned()).unwrap_or_default())),
            "end" => r.complete = true,
            _ => {}
        }
    }
    Ok(r)
}

fn check(name: impl Into<String>, ok: bool, detail: impl Into<String>) -> Check {
    Check { name: name.into(), ok, detail: detail.into() }
}

fn show<T: std::fmt::Debug>(v: &Option<T>) -> String {
    v.as_ref().map_or_else(|| "unknown".to_owned(), |x| format!("{x:?}"))
}

/// Checks a g-r-d daemon's status block (shared by the system and headless daemons).
fn status_checks(out: &mut Vec<Check>, who: &str, st: Option<&GrdStatus>, cert: Option<&str>) {
    let missing = GrdStatus::default();
    let st = st.unwrap_or(&missing);
    out.push(check(
        format!("{who}: daemon unit active"),
        st.unit_active,
        format!("unit_active={}", st.unit_active),
    ));
    out.push(check(
        format!("{who}: RDP enabled"),
        st.enabled == Some(true),
        format!("status={}", show(&st.enabled)),
    ));
    out.push(check(
        format!("{who}: TLS certificate valid"),
        st.tls_cert.is_some() && st.tls_key.is_some() && cert == Some("valid"),
        format!("cert={} key={} state={}", show(&st.tls_cert), show(&st.tls_key), cert.unwrap_or("unknown")),
    ));
    out.push(check(
        format!("{who}: RDP credentials set"),
        st.username_set && st.password_set,
        format!("username_set={} password_set={}", st.username_set, st.password_set),
    ));
}

/// Is a g-r-d daemon owned by `user` (and whose args contain `mode`) listening on `port`?
fn listening(r: &HostReport, port: u16, user: &str, mode: &str) -> (bool, String) {
    let on_port: Vec<&Listener> = r.listeners.iter().filter(|l| l.port == port).collect();
    if on_port.is_empty() {
        return (false, "nothing listening".into());
    }
    let owners: Vec<String> = on_port
        .iter()
        .map(|l| match r.processes.iter().find(|p| p.pid == l.pid) {
            Some(p) => format!("{} pid {} ({}: {})", l.process, l.pid, p.user, p.args),
            None => format!("{} pid {}", l.process, l.pid),
        })
        .collect();
    let ok = on_port.iter().any(|l| {
        r.processes
            .iter()
            .any(|p| p.pid == l.pid && p.user == user && p.args.split_whitespace().any(|a| a == mode))
    });
    (ok, owners.join("; "))
}

fn unit<'a>(r: &'a HostReport, name: &str) -> Option<&'a UnitState> {
    r.units.iter().find(|(n, _)| n == name).map(|(_, s)| s)
}

fn cert<'a>(r: &'a HostReport, owner: &str) -> Option<&'a str> {
    r.certs.iter().find(|(o, _)| o == owner).map(|(_, s)| s.as_str())
}

/// Evaluates a report against expectations.
pub fn evaluate(report: &HostReport, exp: &Expectations) -> Vec<Check> {
    let r = report;
    let mut out = Vec::new();
    out.push(check("report complete", r.complete, if r.complete { "ok" } else { "report truncated" }));

    // Remote Login: the system daemon, on (`--system`) or off (`--no-system`).
    let sys_unit = unit(r, "gnome-remote-desktop.service");
    if exp.system {
        out.push(check(
            "system: gnome-remote-desktop.service enabled and active",
            sys_unit.is_some_and(UnitState::is_up),
            format!("{sys_unit:?}"),
        ));
        status_checks(&mut out, "system", r.system.as_ref(), cert(r, "system"));
        let sys_port = r.system.as_ref().and_then(|s| s.port);
        out.push(check(
            format!("system: RDP port {}", exp.system_port),
            sys_port == Some(exp.system_port),
            format!("port={}", show(&sys_port)),
        ));
        let (ok, detail) = listening(r, exp.system_port, "gnome-remote-desktop", "--system");
        out.push(check(format!("system: listening on :{}", exp.system_port), ok, detail));
    } else {
        out.push(check(
            "system: gnome-remote-desktop.service disabled and stopped (Remote Login off)",
            sys_unit.is_none_or(|s| s.active != "active" && s.enabled != "enabled"),
            format!("{sys_unit:?}"),
        ));
        let enabled = r.system.as_ref().and_then(|s| s.enabled);
        out.push(check(
            "system: RDP disabled (Remote Login off)",
            enabled != Some(true),
            format!("status={}", show(&enabled)),
        ));
    }

    // Headless daemons.
    for h in &exp.headless {
        let u = h.user.as_str();
        let st = r.headless.iter().find(|(n, _)| n == u).map(|(_, s)| s);
        status_checks(&mut out, u, st, cert(r, u));
        out.push(check(
            format!("{u}: RDP port pinned to {}", h.port),
            st.is_some_and(|s| s.port == Some(h.port) && s.negotiate_port == Some(false)),
            format!(
                "port={} negotiate_port={}",
                show(&st.and_then(|s| s.port)),
                show(&st.and_then(|s| s.negotiate_port))
            ),
        ));
        let (ok, detail) = listening(r, h.port, u, "--headless");
        out.push(check(format!("{u}: listening on :{}", h.port), ok, detail));
        let uu = r
            .user_units
            .iter()
            .find(|(user, name, _)| user == u && name == "gnome-remote-desktop-headless.service")
            .map(|(_, _, s)| s);
        out.push(check(
            format!("{u}: gnome-remote-desktop-headless.service enabled and active"),
            uu.is_some_and(UnitState::is_up),
            format!("{uu:?}"),
        ));

        if h.persistent {
            let name = format!("gnome-headless-session@{u}.service");
            let s = unit(r, &name);
            out.push(check(
                format!("{u}: {name} enabled and active"),
                s.is_some_and(UnitState::is_up),
                format!("{s:?}"),
            ));
            let props = r.unit_props.iter().find(|(n, _)| *n == name).map(|(_, p)| p);
            let has = |k: &str, v: &str| props.is_some_and(|p| p.iter().any(|(pk, pv)| pk == k && pv == v));
            out.push(check(
                // User=gdm passes on AnduinOS; the setup installs User=root since GNOME-WIZARD.
                format!("{u}: headless session drop-in (DynamicUser=no, User=root or gdm)"),
                has("DynamicUser", "no") && (has("User", "root") || has("User", "gdm")),
                format!("{props:?}"),
            ));
            out.push(check(
                // gdm-new-session exits 0 when the session ends; Restart=on-failure leaves it dead.
                format!("{u}: headless session restarts whenever it ends (Restart=always)"),
                has("Restart", "always"),
                format!("{props:?}"),
            ));
        }
    }
    out
}
