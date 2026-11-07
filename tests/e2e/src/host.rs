//! Remote-state helpers for the real-host tests (plan §5.3).
//!
//! Everything goes through `ssh` to the GNOME host (`DRIFT_E2E_SSH`, default
//! `homelab@10.0.2.40`), which is also how the tests reach the RDP daemons: macOS Local
//! Network Privacy blocks direct LAN connections from freshly built binaries (plan §1.8), so
//! each test connects to a **local forward** ([`SshForward`]).
//!
//! Test users and their passwords are secrets: nothing here prints a command line or its
//! output, and `cargo xtask e2e` redacts both anyway.

use std::io::Write as _;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The SSH target of the GNOME host.
pub fn ssh_target() -> String {
    crate::var("DRIFT_E2E_SSH").unwrap_or_else(|| "homelab@10.0.2.40".into())
}

/// Runs `command` on the host and returns its standard output.
///
/// # Panics
/// When SSH cannot run or the command fails (the message never contains the command).
pub fn ssh(command: &str) -> String {
    match try_ssh(command) {
        Ok(out) => out,
        Err(e) => panic!("remote command failed: {e}"),
    }
}

/// Multiplexing options: every remote command shares one SSH connection, so a suite that runs
/// hundreds of small commands does not trip the host's `MaxStartups` (which would also kill
/// the forwards the tests connect through).
fn control_args() -> [String; 6] {
    [
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        // Not `TMPDIR`: a unix socket path is capped at 104 bytes and macOS' per-user
        // temporary directory already eats most of that.
        "ControlPath=/tmp/drift-e2e-ssh-%C".into(),
        "-o".into(),
        "ControlPersist=120".into(),
    ]
}

/// Runs `command` on the host, returning its standard output or an error summary.
pub fn try_ssh(command: &str) -> Result<String, String> {
    let output = Command::new("ssh")
        .args(control_args())
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", &ssh_target(), command])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run ssh: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "exit {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_owned())
}

/// A test user's numeric uid.
pub fn uid_of(user: &str) -> u32 {
    ssh(&format!("id -u {user}")).trim().parse().unwrap_or(0)
}

/// Wraps `command` so it runs inside `user`'s GNOME session (Wayland, D-Bus, PipeWire).
pub fn in_session(user: &str, command: &str) -> String {
    let uid = uid_of(user);
    format!(
        "sudo -n -u {user} env XDG_RUNTIME_DIR=/run/user/{uid} WAYLAND_DISPLAY=wayland-0 \
         DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus \
         XDG_SESSION_TYPE=wayland GDK_BACKEND=wayland sh -c {}",
        shell_quote(command)
    )
}

/// Runs `command` inside `user`'s session and waits for it.
pub fn run_in_session(user: &str, command: &str) -> String {
    ssh(&in_session(user, command))
}

/// Starts `command` inside `user`'s session in the background (output to `/tmp/drift_e2e.log`).
pub fn spawn_in_session(user: &str, command: &str) {
    ssh(&in_session(user, &format!("nohup {command} >>/tmp/drift_e2e.log 2>&1 &")));
}

/// Kills every process of `user` whose command line contains `pattern`.
pub fn kill_in_session(user: &str, pattern: &str) {
    let _ = try_ssh(&format!("sudo -n -u {user} pkill -f {} || true", shell_quote(pattern)));
}

/// Copies a local file into `user`'s home directory (mode 755), returning the remote path.
pub fn upload_to_home(user: &str, local: &std::path::Path) -> String {
    let name = local.file_name().and_then(|n| n.to_str()).unwrap_or("drift_helper");
    let remote = format!("/home/{user}/{name}");
    let contents = std::fs::read(local).unwrap_or_else(|e| panic!("cannot read {}: {e}", local.display()));
    let mut child = Command::new("ssh")
        .args(control_args())
        .args([
            "-o",
            "BatchMode=yes",
            &ssh_target(),
            &format!("sudo -n -u {user} tee {remote} >/dev/null && sudo -n chmod 755 {remote}"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot run ssh: {e}"));
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(&contents);
    }
    let status = child.wait().unwrap_or_else(|e| panic!("ssh failed: {e}"));
    assert!(status.success(), "uploading the helper failed");
    remote
}

/// The `loginctl` session id of `user`'s graphical session, if any.
pub fn session_id(user: &str) -> Option<String> {
    sessions().into_iter().find_map(|(id, owner, class)| (owner == user && class == "user").then_some(id))
}

/// How many GDM greeter sessions exist right now (plan §1.9: they must not pile up).
pub fn greeter_count() -> usize {
    sessions().iter().filter(|(_, _, class)| class == "greeter").count()
}

/// `(session id, user, class)` of every logind session.
///
/// `loginctl list-sessions --no-legend` prints `SESSION UID USER SEAT LEADER CLASS TTY …`.
pub fn sessions() -> Vec<(String, String, String)> {
    ssh("loginctl list-sessions --no-legend")
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            match (fields.first(), fields.get(2), fields.get(5)) {
                (Some(id), Some(user), Some(class)) => {
                    Some(((*id).to_owned(), (*user).to_owned(), (*class).to_owned()))
                }
                _ => None,
            }
        })
        .collect()
}

/// Reads a file from the host, `None` when it does not exist.
///
/// Helper output lands in `/tmp` owned by the test user, so reading and deleting go through
/// the host's passwordless `sudo`.
pub fn read_remote(path: &str) -> Option<String> {
    try_ssh(&format!("sudo -n cat {path} 2>/dev/null || true")).ok().filter(|s| !s.is_empty())
}

/// Deletes a file on the host (ignoring failures).
pub fn remove_remote(path: &str) {
    let _ = try_ssh(&format!("sudo -n rm -f {path}"));
}

/// Waits (up to `within`) for `read` to return a value satisfying `pred`.
pub fn wait_for_remote(path: &str, within: Duration, pred: impl Fn(&str) -> bool) -> Option<String> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(text) = read_remote(path)
            && pred(&text)
        {
            return Some(text);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Makes sure `user`'s headless GNOME session is running and **unlocked**.
///
/// A locked session swallows every input event and stops propagating clipboard changes, which
/// looks exactly like a broken client. GNOME cannot be unlocked over D-Bus without the user's
/// password, so the helper turns the idle lock off for good (`gsettings`) and restarts the
/// headless session when it is locked. Idempotent: an unlocked session is left alone.
pub fn ensure_unlocked_session(user: &str) {
    let settings = concat!(
        "gsettings set org.gnome.desktop.screensaver lock-enabled false; ",
        "gsettings set org.gnome.desktop.screensaver idle-activation-enabled false; ",
        "gsettings set org.gnome.desktop.session idle-delay 0"
    );
    if session_id(user).is_some() {
        let _ = try_ssh(&in_session(user, settings));
        if !is_locked(user) {
            return;
        }
        eprintln!("[e2e] the headless session is locked; restarting it");
        let _ = try_ssh(&format!("sudo -n systemctl stop gnome-headless-session@{user}.service"));
        std::thread::sleep(Duration::from_secs(3));
    }
    let _ = try_ssh(&format!("sudo -n systemctl start gnome-headless-session@{user}.service"));
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if session_id(user).is_some() && !is_locked(user) {
            std::thread::sleep(Duration::from_secs(5));
            let _ = try_ssh(&in_session(user, settings));
            return;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    panic!("the headless session did not come back unlocked");
}

/// Whether `user`'s session is locked (`loginctl`'s hint).
pub fn is_locked(user: &str) -> bool {
    let Some(id) = session_id(user) else { return false };
    try_ssh(&format!("loginctl show-session {id} -p LockedHint --value")).is_ok_and(|v| v.trim() == "yes")
}

/// An `ssh -N -L <local>:localhost:<remote>` forward that a test can kill and restore
/// (the M7-3 reconnect tests cut the transport this way).
#[derive(Debug)]
pub struct SshForward {
    remote_port: u16,
    local_port: u16,
    child: Option<Child>,
}

impl SshForward {
    /// Opens a forward to `remote_port` on a free local port.
    ///
    /// # Panics
    /// When SSH cannot be started or the forward does not come up.
    pub fn open(remote_port: u16) -> Self {
        let local_port = free_port();
        let mut forward = Self { remote_port, local_port, child: None };
        forward.start();
        forward
    }

    /// The local port to connect to.
    pub fn port(&self) -> u16 {
        self.local_port
    }

    /// Starts (or restarts) the forward and waits for the port to accept connections.
    pub fn start(&mut self) {
        if self.child.is_some() {
            return;
        }
        let child = Command::new("ssh")
            .args([
                "-N",
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ServerAliveInterval=15",
                // A dedicated connection: the forward must not die with the command master.
                "-o",
                "ControlPath=none",
                "-L",
                &format!("{}:localhost:{}", self.local_port, self.remote_port),
                &ssh_target(),
            ])
            .stdin(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("cannot start the SSH forward: {e}"));
        self.child = Some(child);
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if std::net::TcpStream::connect(("127.0.0.1", self.local_port)).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the SSH forward on 127.0.0.1:{} did not come up", self.local_port);
    }

    /// Kills the forward: every connection through it dies, like a network drop.
    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for SshForward {
    fn drop(&mut self) {
        self.kill();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map_or(14_000, |a| a.port())
}

/// Single-quotes `text` for `sh -c`.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_quotes() {
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
        assert_eq!(shell_quote("echo hi"), "'echo hi'");
    }
}
