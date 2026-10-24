//! M0-4 Red: the host scripts are shellcheck-clean and the setup script embeds exactly the
//! committed systemd drop-in.

use std::path::PathBuf;
use std::process::Command;

fn host(file: &str) -> PathBuf {
    xtask::repo::root().join("host").join(file)
}

fn read(file: &str) -> String {
    std::fs::read_to_string(host(file)).unwrap_or_else(|e| panic!("host/{file}: {e}"))
}

#[test]
fn host_scripts_are_shellcheck_clean() {
    for script in ["drift-host-setup.sh", "drift-host-report.sh", "drift-gnome-setup"] {
        let path = host(script);
        assert!(path.is_file(), "missing host/{script}");
        let out = Command::new("shellcheck")
            .args(["--shell=bash", "--severity=style"])
            .arg(&path)
            .output()
            .unwrap_or_else(|e| panic!("shellcheck is required (brew install shellcheck): {e}"));
        assert!(
            out.status.success(),
            "shellcheck host/{script}:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn setup_script_is_strict_and_defaults_to_check_mode() {
    let s = read("drift-host-setup.sh");
    assert!(s.starts_with("#!/usr/bin/env bash\n"));
    assert!(s.contains("\nset -euo pipefail\n"));
    assert!(s.contains("\nMODE=check\n"), "default mode must change nothing");
    for flag in ["--check", "--dry-run", "--apply", "--system", "--headless", "--install", "--firewall"] {
        assert!(s.contains(flag), "missing {flag}");
    }
    assert!(s.contains("change_secret"), "credential changes must not echo their command line");
}

#[test]
fn setup_script_embeds_the_committed_dropin() {
    let dropin = read("gnome-headless-session-dropin.conf");
    // GNOME-WIZARD: as `gdm`, polkit refuses gdm-new-session on Ubuntu 26.04 / GDM 50
    // ("Not authorized for action …manage-user-displays"); root passes everywhere.
    assert_eq!(dropin, "[Service]\nDynamicUser=no\nUser=root\nGroup=root\n", "GNOME-WIZARD drop-in");
    let s = read("drift-host-setup.sh");
    let start = s.find("readonly DROPIN_CONTENT='").map(|i| i + "readonly DROPIN_CONTENT='".len());
    let embedded = start.and_then(|i| s[i..].find('\'').map(|j| &s[i..i + j]));
    assert_eq!(embedded.map(|e| format!("{e}\n")), Some(dropin));
}

#[test]
fn report_script_is_read_only() {
    let s = read("drift-host-report.sh");
    for forbidden in
        ["set-credentials", "set-port", " enable", "disable-", "install ", "rm ", "gsettings set"]
    {
        assert!(!s.contains(forbidden), "report script must not contain {forbidden:?}");
    }
    assert!(!s.contains("--show-credentials"));
    assert_eq!(xtask::host_check::REPORT_SCRIPT, s);
}

/// GNOME-only: the setup reports each daemon's certificate fingerprint in a line the Mac side
/// parses, and turns off the screen lock that would swallow a headless session's input.
#[test]
fn setup_script_reports_fingerprints_and_unlocks_headless_sessions() {
    let s = read("drift-host-setup.sh");
    assert!(s.contains("printf 'fingerprint: %s %s\\n'"), "machine-readable fingerprint line");
    assert!(s.contains("org.gnome.desktop.screensaver lock-enabled false"));
    for manager in ["apt-get", "dnf", "zypper", "pacman"] {
        assert!(s.contains(manager), "--install knows {manager}");
    }
    for firewall in ["ufw allow", "firewall-cmd --permanent --add-port"] {
        assert!(s.contains(firewall), "--firewall uses {firewall}");
    }
}

/// GNOME-only: `drift-gnome-setup` opens a `drift-connect:` link without the password; the
/// same link is parsed by `drift_core::link` (crates/drift-core/tests/connect_link.rs).
#[test]
fn gnome_setup_prints_the_connect_link_without_a_password() {
    let pin =
        "f3:e7:a2:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:01:02:03:04:05:06:07:08:09:0a:0b:0c:0d";
    let out = Command::new(host("drift-gnome-setup"))
        .args(["--print-link", "--name", "Work box", "--port", "3391", "--pin", pin, "me@box.local"])
        .output()
        .expect("run host/drift-gnome-setup");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("drift-connect:?name=Work%20box&host=box.local&port=3391&mode=headless&user=me&pin={pin}\n")
    );
    let s = read("drift-gnome-setup");
    assert!(s.contains("umask 077"), "the password travels in a private file");
    assert!(!s.contains("pass=$(urlencode"), "no password in a link");
}

/// GNOME-WIZARD: `--desktop` installs the smallest GNOME that runs a headless session (no
/// recommends, so no NetworkManager to fight systemd-networkd), and GDM must be up at boot because
/// `gnome-headless-session@.service` requires it.
#[test]
fn setup_script_installs_a_minimal_desktop_and_keeps_gdm_at_boot() {
    let s = read("drift-host-setup.sh");
    assert!(s.contains("--desktop)"), "--desktop flag");
    assert!(
        s.contains("gdm3 ubuntu-session gnome-remote-desktop ptyxis openssl fonts-cantarell"),
        "apt minimal set"
    );
    assert!(s.contains("--no-install-recommends"), "apt without recommends");
    assert!(s.contains("install_weak_deps=False"), "dnf without weak deps");
    assert!(s.contains("gdm gnome-shell gnome-remote-desktop ptyxis openssl"), "dnf/pacman/zypper set");
    assert!(!s.contains("ubuntu-desktop"), "never the full desktop metapackage");
    assert!(s.contains("ensure_unit gdm.service"), "GDM enabled and running");
    assert!(s.contains("systemctl set-default graphical.target"), "boots into graphical.target");
}

/// GNOME-WIZARD: keeping a headless session awake also turns off screen blanking, and
/// `--allow-from CIDR` limits the firewall rule to one network.
#[test]
fn setup_script_blanks_never_and_can_limit_the_firewall() {
    let s = read("drift-host-setup.sh");
    assert!(s.contains("org.gnome.desktop.session idle-delay 0"));
    assert!(s.contains("--allow-from)"), "--allow-from flag");
    assert!(s.contains("ufw allow from \"$ALLOW_FROM\" to any port"), "ufw source rule");
    assert!(s.contains("rich-rule"), "firewalld source rule");
}

/// The setup script refuses a malformed `--allow-from` before touching anything.
#[test]
fn setup_script_rejects_a_bad_allow_from() {
    let out = Command::new("bash")
        .arg(host("drift-host-setup.sh"))
        .args(["--headless", "me:3390", "--firewall", "--allow-from", "10.1.2.0/99"])
        .output()
        .expect("run host/drift-host-setup.sh");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("bad --allow-from"), "{out:?}");
}

/// A fake `ssh` that answers `ssh -G` and records every other remote command in `log`.
fn fake_ssh(dir: &std::path::Path) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("fake bin dir");
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/usr/bin/env bash\nif [[ $1 == -G ]]; then printf 'user me\\nhostname box.local\\n'; exit 0; fi\n\
             printf '%s\\n' \"${{@: -1}}\" >> '{}'\ncat >/dev/null\n[[ ${{@: -1}} == mktemp* ]] && echo /tmp/drift-setup.X\nexit 0\n",
            dir.join("log").display()
        ),
    )
    .expect("write fake ssh");
    let mut perms = std::fs::metadata(&ssh).expect("fake ssh").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&ssh, perms).expect("chmod fake ssh");
    bin
}

fn gnome_setup_with_fake_ssh(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    let bin = fake_ssh(dir);
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    Command::new(host("drift-gnome-setup"))
        .args(args)
        .env("PATH", path)
        .output()
        .expect("run host/drift-gnome-setup")
}

/// GNOME-WIZARD: the Mac-side command always asks for the minimal desktop (check-first, so a
/// machine that has GNOME is not touched) and passes `--allow-from` through.
#[test]
fn gnome_setup_installs_the_desktop_and_passes_allow_from() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = gnome_setup_with_fake_ssh(dir.path(), &["--check", "--allow-from", "10.1.0.0/21", "me@box.local"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let log = std::fs::read_to_string(dir.path().join("log")).expect("fake ssh log");
    let run = log.lines().find(|l| l.contains("setup.sh")).expect("setup.sh run");
    assert!(run.contains(" --desktop "), "{run}");
    assert!(run.contains(" --firewall --allow-from 10.1.0.0/21 "), "{run}");
}

/// `--allow-from` lands in a remote command line, so anything but a network is refused locally.
#[test]
fn gnome_setup_refuses_an_allow_from_that_is_not_a_network() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = gnome_setup_with_fake_ssh(dir.path(), &["--check", "--allow-from", "1.2.3.4;reboot", "me@box.local"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a network"), "{out:?}");
    assert!(!dir.path().join("log").exists(), "nothing ran over SSH");
}

/// GNOME-WIZARD: g-r-d streams the screen through PipeWire. A user manager that was already
/// running when GNOME was installed never starts `pipewire.socket`, and every RDP login then
/// ends with "Couldn't connect pipewire context" (seen on 10.1.2.5, 2026-10-24).
#[test]
fn setup_script_starts_pipewire_before_remote_desktop() {
    let s = read("drift-host-setup.sh");
    let pipewire = s.find("ensure_user_unit \"$user\" pipewire.socket").expect("pipewire.socket ensured");
    let grd =
        s.find("ensure_user_unit \"$user\" gnome-remote-desktop-headless.service").expect("g-r-d ensured");
    assert!(pipewire < grd, "PipeWire must be up before the headless daemon");
}
