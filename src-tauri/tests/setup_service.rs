//! WIZ-4 Red: the setup wizard's service against an in-process SSH server
//! (`drift_testkit::ssh_server`), a fake `sudo` and a stand-in for `drift-host-setup.sh` (ADR
//! `GNOME-WIZARD-headless-setup`).

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use drift_app::setup_wizard::{SetupError, SetupService, SignIn};
use drift_core::link;
use drift_core::setup::{Outcome, PASSWORD_LEN, RunMode, SetupLine, Sudo};
use drift_ssh::{Diagnosis, Target, Trust};
use drift_testkit::ssh_server::{ServerOptions, TestServer};

const FP: &str = "48:a5:ff:8e:c8:3f:b3:06:d9:69:cc:22:9d:ea:86:76:88:54:57:11:7c:b3:dc:52:a5:22:b3:52:cf:e2:8e:2a";

/// Stands in for drift-host-setup.sh: reports its mode, what reached it, and one change.
const FAKE_SCRIPT: &str = r#"mode=${1#--}
echo "drift-host-setup: mode=$mode"
echo "ok:     GNOME Shell and GDM are installed"
echo "user=${DRIFT_HEADLESS_RDP_USER_TESTER:-unset} pass=${DRIFT_HEADLESS_RDP_PASS_TESTER:+set}"
echo "args: $*"
echo "fingerprint: tester 48:a5:ff:8e:c8:3f:b3:06:d9:69:cc:22:9d:ea:86:76:88:54:57:11:7c:b3:dc:52:a5:22:b3:52:cf:e2:8e:2a"
if [ "$mode" = check ]; then
  echo "would:  tester: enable and start pipewire.socket"
  echo "drift-host-setup: 1 change(s) pending, 0 error(s)"
  exit 2
fi
echo "change: tester: enable and start pipewire.socket"
echo "drift-host-setup: 1 change(s) applied, 0 error(s)"
"#;

/// A `sudo` that logs its arguments; `-n true` exits with `$HOME/sudo-n` (0 when missing),
/// `-n CMD` runs CMD, and `-k -S -p '' CMD` reads one password line first (`sudopw`).
const FAKE_SUDO: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/sudo-argv"
if [ "$1" = -n ] && [ "$2" = true ]; then exit "$(cat "$HOME/sudo-n" 2>/dev/null || echo 0)"; fi
if [ "$1" = -n ]; then shift; exec "$@"; fi
if [ "$1" = -k ] && [ "$2" = -S ] && [ "$3" = -p ]; then
  shift 4
  IFS= read -r pw
  if [ "$pw" = sudopw ]; then exec "$@"; fi
  echo "Sorry, try again." >&2
  echo "sudo: 3 incorrect password attempts" >&2
  exit 1
fi
exit 9
"#;

struct Machine {
    _dir: tempfile::TempDir,
    home: PathBuf,
    service: SetupService,
    server: TestServer,
}

impl Machine {
    fn target(&self) -> Target {
        Target { host: "127.0.0.1".into(), port: self.server.port, user: "tester".into() }
    }
}

async fn machine(password_sudo: bool) -> Machine {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("home");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::write(bin.join("sudo"), FAKE_SUDO).expect("fake sudo");
    std::fs::set_permissions(bin.join("sudo"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
    if password_sudo {
        std::fs::write(home.join("sudo-n"), "1").expect("sudo-n");
    }
    let mut options = ServerOptions::new(&home);
    options.password = Some("hunter22".into());
    options.path_prefix = Some(bin);
    let server = TestServer::start(options).await;
    let service = SetupService::new(dir.path().join("ssh"), "drift@test", FAKE_SCRIPT).expect("service");
    Machine { _dir: dir, home, service, server }
}

/// Signs in the way the wizard does on a fresh machine: trust, then the password once.
async fn signed_in(m: &Machine) -> SignIn {
    let _ = m.service.connect(m.target(), Trust::TrustNew).await;
    m.service.install_key(m.target(), "hunter22").await
}

async fn run(m: &Machine, mode: RunMode, sudo: Option<&str>) -> (Vec<SetupLine>, Result<drift_app::setup_wizard::SetupResult, SetupError>) {
    let mut options = m.service.defaults("127.0.0.1", "tester");
    options.rdp_password = Some("Secret-pass1".into());
    let mut lines = Vec::new();
    let result = m.service.run(&options, mode, sudo, &mut |l| lines.push(l)).await;
    (lines, result)
}

fn sudo_argv(home: &Path) -> String {
    std::fs::read_to_string(home.join("sudo-argv")).unwrap_or_default()
}

#[tokio::test]
async fn signing_in_goes_trust_then_password_once_then_ready() {
    let m = machine(false).await;
    assert_eq!(
        m.service.connect(m.target(), Trust::KnownOnly).await,
        SignIn::Problem { diagnosis: Diagnosis::HostKeyUnknown { fingerprint: m.server.fingerprint.clone() } }
    );
    assert_eq!(
        m.service.connect(m.target(), Trust::TrustNew).await,
        SignIn::Problem { diagnosis: Diagnosis::KeyNotAuthorized { offers_password: true } }
    );
    assert_eq!(
        m.service.install_key(m.target(), "wrong").await,
        SignIn::Problem { diagnosis: Diagnosis::PasswordRejected }
    );
    assert_eq!(m.service.install_key(m.target(), "hunter22").await, SignIn::Ready { sudo: Sudo::Passwordless });
    // The key is installed now: the next sign-in needs no password.
    assert_eq!(m.service.connect(m.target(), Trust::KnownOnly).await, SignIn::Ready { sudo: Sudo::Passwordless });
    let key = m.service.public_key().expect("public key");
    assert!(key.starts_with("ssh-ed25519 ") && key.ends_with(" drift@test"), "{key}");
}

#[tokio::test]
async fn check_then_apply_streams_lines_and_leaves_pinned_connections() {
    let m = machine(false).await;
    assert_eq!(signed_in(&m).await, SignIn::Ready { sudo: Sudo::Passwordless });

    let (lines, result) = run(&m, RunMode::Check, None).await;
    let result = result.expect("check");
    assert_eq!(result.outcome, Outcome::ChangesPending);
    assert_eq!(result.changes, Some(1));
    assert!(result.profiles.is_empty(), "nothing to save after a check");
    assert_eq!(lines.first(), Some(&SetupLine::Mode { mode: RunMode::Check }));
    assert!(lines.contains(&SetupLine::Would { text: "tester: enable and start pipewire.socket".into() }));
    let args = lines.iter().find_map(|l| match l {
        SetupLine::Output { text } if text.starts_with("args: ") => Some(text.clone()),
        _ => None,
    });
    assert_eq!(args.as_deref(), Some("args: --check --desktop --install --firewall --headless tester:3390:persistent"));

    let (_, result) = run(&m, RunMode::Apply, None).await;
    let result = result.expect("apply");
    assert_eq!(result.outcome, Outcome::Applied);
    assert_eq!(result.profiles.len(), 1);
    assert_eq!(
        link::format(&result.profiles[0]),
        format!("drift-connect:?name=127.0.0.1&host=127.0.0.1&port=3390&mode=headless&user=tester&pin={FP}")
    );
}

#[tokio::test]
async fn credentials_reach_the_script_but_never_a_command_line() {
    let m = machine(false).await;
    let _ = signed_in(&m).await;
    let (lines, _) = run(&m, RunMode::Apply, None).await;
    assert!(lines.contains(&SetupLine::Output { text: "user=tester pass=set".into() }), "{lines:?}");
    for line in &lines {
        assert!(!format!("{line:?}").contains("Secret-pass1"), "password in output: {line:?}");
    }
    let argv = sudo_argv(&m.home);
    assert!(argv.contains("-n bash -s --"), "{argv}");
    assert!(!argv.contains("Secret-pass1"), "{argv}");
}

#[tokio::test]
async fn a_password_sudo_asks_for_it_and_names_a_wrong_one() {
    let m = machine(true).await;
    assert_eq!(signed_in(&m).await, SignIn::Ready { sudo: Sudo::Password });
    assert_eq!(run(&m, RunMode::Check, None).await.1, Err(SetupError::SudoPasswordNeeded));
    assert_eq!(run(&m, RunMode::Check, Some("nope")).await.1, Err(SetupError::SudoPasswordRejected));
    let (_, result) = run(&m, RunMode::Apply, Some("sudopw")).await;
    assert_eq!(result.map(|r| r.outcome), Ok(Outcome::Applied));
    let argv = sudo_argv(&m.home);
    assert!(argv.contains("-k -S -p  bash -s --"), "{argv}");
    assert!(!argv.contains("sudopw"), "the sudo password never reaches a command line: {argv}");
}

#[tokio::test]
async fn runs_need_a_sign_in_and_valid_options() {
    let m = machine(false).await;
    assert_eq!(run(&m, RunMode::Check, None).await.1, Err(SetupError::NotSignedIn));
    let _ = signed_in(&m).await;
    let mut options = m.service.defaults("127.0.0.1", "tester");
    options.port = 0;
    let result = m.service.run(&options, RunMode::Check, None, &mut |_| {}).await;
    assert_eq!(result, Err(SetupError::Invalid { issues: vec![drift_core::setup::SetupIssue::Port] }));
}

#[tokio::test]
async fn defaults_come_with_a_fresh_password() {
    let m = machine(false).await;
    let a = m.service.defaults("dev.lan", "me");
    let b = m.service.defaults("dev.lan", "me");
    let (pa, pb) = (a.rdp_password.clone().expect("generated"), b.rdp_password.clone().expect("generated"));
    assert_eq!(pa.len(), PASSWORD_LEN);
    assert_ne!(pa, pb);
    assert_eq!(a.validate(), Ok(()));
    assert_eq!(a.name, "dev");
}
