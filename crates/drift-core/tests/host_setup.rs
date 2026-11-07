//! WIZ-2 Red: `drift_core::setup` — the wizard's options, the SSH run of
//! `host/drift-host-setup.sh` they become, its output lines, and the connections it leaves
//! (ADR `GNOME-WIZARD-headless-setup`).

use drift_core::link;
use drift_core::profile::{CertFingerprint, ConnectMode};
use drift_core::setup::{
    DEFAULT_HEADLESS_PORT, Outcome, PASSWORD_LEN, RunMode, SetupIssue, SetupLine, SetupOptions, SetupReport,
    Sudo, connection_profiles, generate_password, outcome, parse_line, remote_command, remote_stdin,
    shell_quote,
};

const FP: &str =
    "48:a5:ff:8e:c8:3f:b3:06:d9:69:cc:22:9d:ea:86:76:88:54:57:11:7c:b3:dc:52:a5:22:b3:52:cf:e2:8e:2a";
const SYSTEM_FP: &str =
    "f3:e7:a2:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:01:02:03:04:05:06:07:08:09:0a:0b:0c:0d";

fn fp(s: &str) -> CertFingerprint {
    s.parse().unwrap_or_else(|e| panic!("{s}: {e}"))
}

fn options() -> SetupOptions {
    let mut o = SetupOptions::defaults("dev.lan", "alice");
    o.rdp_password = Some("Secret-pass1".into());
    o
}

#[test]
fn defaults_follow_drift_gnome_setup() {
    let o = SetupOptions::defaults("dev.lan", "alice");
    assert_eq!(o.name, "dev");
    assert_eq!(o.account, "alice");
    assert_eq!(o.port, DEFAULT_HEADLESS_PORT);
    assert_eq!(o.rdp_username, "alice");
    assert_eq!(o.rdp_password, None, "the wizard fills in a generated password");
    assert!(!o.remote_login);
    assert!(o.install_desktop);
    assert!(o.open_firewall);
    assert_eq!(o.allow_from, None);
    // An IP address is its own name.
    assert_eq!(SetupOptions::defaults("10.0.2.5", "me").name, "10.0.2.5");
    assert_eq!(options().validate(), Ok(()));
}

#[test]
fn validation_names_every_bad_field() {
    let mut o = options();
    o.name = "  ".into();
    o.account = "Bad User".into();
    o.port = 0;
    o.rdp_username = String::new();
    o.rdp_password = Some("short".into());
    o.allow_from = Some("10.0.2.0/99".into());
    assert_eq!(
        o.validate(),
        Err(vec![
            SetupIssue::Name,
            SetupIssue::Account,
            SetupIssue::Port,
            SetupIssue::RdpUsername,
            SetupIssue::RdpPassword,
            SetupIssue::AllowFrom
        ])
    );

    let mut o = options();
    o.remote_login = true;
    o.port = 3389;
    assert_eq!(o.validate(), Err(vec![SetupIssue::PortTakenByRemoteLogin]));

    for account in ["root2", "_svc", "a-b_c", "drifttest2"] {
        let mut o = options();
        o.account = account.into();
        assert_eq!(o.validate(), Ok(()), "{account}");
    }
    for account in ["", "9lives", "me;reboot", "a'b", &"x".repeat(33)] {
        let mut o = options();
        o.account = account.into();
        assert_eq!(o.validate(), Err(vec![SetupIssue::Account]), "{account:?}");
    }
    for net in ["10.0.0.0/21", "10.0.2.5", "fd00::/64", "fe80::1"] {
        let mut o = options();
        o.allow_from = Some(net.into());
        assert_eq!(o.validate(), Ok(()), "{net}");
    }
    for net in ["10.0.2.256", "10.0.2.0/33", "1.2.3.4;reboot", "fd00::/129", "box.lan", ""] {
        let mut o = options();
        o.allow_from = Some(net.into());
        assert_eq!(o.validate(), Err(vec![SetupIssue::AllowFrom]), "{net:?}");
    }
    let mut o = options();
    o.rdp_password = Some("has\na newline".into());
    assert_eq!(o.validate(), Err(vec![SetupIssue::RdpPassword]));
    o.rdp_password = None;
    assert_eq!(o.validate(), Ok(()), "keeping the machine's credentials is fine");
}

#[test]
fn script_args_match_the_script_flags() {
    let o = options();
    assert_eq!(
        o.script_args(RunMode::Check),
        ["--check", "--desktop", "--install", "--firewall", "--headless", "alice:3390:persistent"]
    );
    let mut o = options();
    o.install_desktop = false;
    o.open_firewall = true;
    o.allow_from = Some("10.0.0.0/21".into());
    o.remote_login = true;
    assert_eq!(
        o.script_args(RunMode::Apply),
        [
            "--apply",
            "--install",
            "--firewall",
            "--allow-from",
            "10.0.0.0/21",
            "--headless",
            "alice:3390:persistent",
            "--system"
        ]
    );
    let mut o = options();
    o.open_firewall = false;
    o.allow_from = Some("10.0.0.0/21".into());
    assert!(
        !o.script_args(RunMode::Apply).iter().any(|a| a.starts_with("--firewall") || a == "--allow-from")
    );
}

#[test]
fn credentials_travel_as_quoted_exports() {
    let mut o = options();
    o.account = "drift-test2".into();
    o.rdp_username = "o'brien".into();
    assert_eq!(
        o.env_exports(),
        "export DRIFT_HEADLESS_RDP_USER_DRIFT_TEST2='o'\\''brien'\n\
         export DRIFT_HEADLESS_RDP_PASS_DRIFT_TEST2='Secret-pass1'\n"
    );
    o.remote_login = true;
    assert!(o.env_exports().ends_with(
        "export DRIFT_SYSTEM_RDP_USER='o'\\''brien'\nexport DRIFT_SYSTEM_RDP_PASS='Secret-pass1'\n"
    ));
    o.rdp_password = None;
    assert_eq!(o.env_exports(), "", "kept credentials are not touched");
}

#[test]
fn remote_command_never_carries_a_secret() {
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
    assert_eq!(shell_quote(""), "''");
    let o = options();
    let args = o.script_args(RunMode::Check);
    assert_eq!(
        remote_command(Sudo::Passwordless, &args),
        "sudo -n bash -s -- '--check' '--desktop' '--install' '--firewall' '--headless' 'alice:3390:persistent'"
    );
    let with_password = remote_command(Sudo::Password, &args);
    // -k: with a cached sudo timestamp, `sudo -S` would not read the password line, and bash
    // would run the password as a command (and print it in "command not found").
    assert!(with_password.starts_with("sudo -k -S -p '' bash -s -- '--check'"), "{with_password}");
    assert!(!with_password.contains("Secret-pass1"));

    let script = "#!/usr/bin/env bash\necho hi\n";
    assert_eq!(
        remote_stdin(None, &o, script),
        format!("{}{script}", o.env_exports()),
        "exports first, then the script"
    );
    assert_eq!(
        remote_stdin(Some("sudo pw"), &o, script),
        format!("sudo pw\n{}{script}", o.env_exports()),
        "sudo -S reads the first line"
    );
}

#[test]
fn generated_passwords_are_letters_and_digits() {
    let random: Vec<u8> = (0..=255).cycle().take(512).collect();
    let p = generate_password(&random).expect("enough bytes");
    assert_eq!(p.len(), PASSWORD_LEN);
    assert!(p.bytes().all(|b| b.is_ascii_alphanumeric()), "{p}");
    assert_eq!(&p[..10], "ABCDEFGHIJ", "bytes map in order: A–Z, a–z, 0–9");
    // Bytes from 248 up would favour the first characters; they are skipped.
    let mut biased = vec![255u8; 100];
    biased.extend(std::iter::repeat_n(0u8, PASSWORD_LEN));
    assert_eq!(generate_password(&biased).as_deref(), Some("A".repeat(PASSWORD_LEN).as_str()));
    assert_eq!(generate_password(&[1, 2, 3]), None);
    let mut o = options();
    o.rdp_password = Some(p);
    assert_eq!(o.validate(), Ok(()));
}

#[test]
fn output_lines_parse() {
    assert_eq!(parse_line("drift-host-setup: mode=check"), SetupLine::Mode { mode: RunMode::Check });
    assert_eq!(parse_line("drift-host-setup: mode=apply\r"), SetupLine::Mode { mode: RunMode::Apply });
    assert_eq!(
        parse_line("ok:     gdm.service enabled and active"),
        SetupLine::Ok { text: "gdm.service enabled and active".into() }
    );
    assert_eq!(
        parse_line("would:  alice: enable and start pipewire.socket"),
        SetupLine::Would { text: "alice: enable and start pipewire.socket".into() }
    );
    assert_eq!(
        parse_line("        $ as_user alice systemctl --user enable --now pipewire.socket"),
        SetupLine::Command { text: "as_user alice systemctl --user enable --now pipewire.socket".into() }
    );
    assert_eq!(
        parse_line("change: install a minimal GNOME (GNOME Shell, GDM, gnome-remote-desktop)"),
        SetupLine::Change { text: "install a minimal GNOME (GNOME Shell, GDM, gnome-remote-desktop)".into() }
    );
    assert_eq!(
        parse_line("skip:   me has no session bus yet; re-run after the session has started"),
        SetupLine::Skip { text: "me has no session bus yet; re-run after the session has started".into() }
    );
    assert_eq!(
        parse_line("wait:   me: waiting for the session to start"),
        SetupLine::Wait { text: "me: waiting for the session to start".into() }
    );
    assert_eq!(
        parse_line("error:  user nobody2 does not exist"),
        SetupLine::Error { text: "user nobody2 does not exist".into() }
    );
    assert_eq!(
        parse_line(&format!("fingerprint: alice {FP}")),
        SetupLine::Fingerprint { owner: "alice".into(), fingerprint: fp(FP) }
    );
    assert_eq!(
        parse_line("drift-host-setup: 1 change(s) pending, 0 error(s)"),
        SetupLine::Summary { changes: 1, errors: 0 }
    );
    assert_eq!(
        parse_line("drift-host-setup: 7 change(s) applied, 2 error(s)"),
        SetupLine::Summary { changes: 7, errors: 2 }
    );
    for other in ["Setting up libpipewire-0.3-0t64 ...", "fingerprint: me not-a-fingerprint", ""] {
        assert_eq!(parse_line(other), SetupLine::Output { text: other.into() }, "{other:?}");
    }
}

/// The run against 10.0.2.5 on 2026-10-24 (`--check`, one change pending).
const CHECK_RUN: &str = "drift-host-setup: mode=check
ok:     GNOME Shell and GDM are installed
ok:     gdm.service enabled and active
fingerprint: alice 48:a5:ff:8e:c8:3f:b3:06:d9:69:cc:22:9d:ea:86:76:88:54:57:11:7c:b3:dc:52:a5:22:b3:52:cf:e2:8e:2a
would:  alice: enable and start pipewire.socket
        $ as_user alice systemctl --user enable --now pipewire.socket
drift-host-setup: 1 change(s) pending, 0 error(s)
";

#[test]
fn a_report_collects_fingerprints_errors_and_the_summary() {
    let mut r = SetupReport::default();
    let kinds: Vec<SetupLine> = CHECK_RUN.lines().map(|l| r.feed(l)).collect();
    assert_eq!(kinds.len(), 7);
    assert_eq!(r.fingerprint("alice"), Some(fp(FP)));
    assert_eq!(r.fingerprint("system"), None);
    assert_eq!(r.summary, Some((1, 0)));
    assert!(r.errors.is_empty());
    r.feed("error:  alice: grdctl --headless status failed");
    r.feed(&format!("fingerprint: alice {SYSTEM_FP}"));
    assert_eq!(r.errors, ["alice: grdctl --headless status failed"]);
    assert_eq!(r.fingerprint("alice"), Some(fp(SYSTEM_FP)), "the last fingerprint wins");
}

#[test]
fn outcomes_follow_the_exit_codes() {
    assert_eq!(outcome(RunMode::Check, Some(0)), Outcome::Converged);
    assert_eq!(outcome(RunMode::Check, Some(2)), Outcome::ChangesPending);
    assert_eq!(outcome(RunMode::Check, Some(1)), Outcome::Failed);
    assert_eq!(outcome(RunMode::Apply, Some(0)), Outcome::Applied);
    assert_eq!(outcome(RunMode::Apply, Some(2)), Outcome::Failed);
    assert_eq!(outcome(RunMode::Apply, None), Outcome::Failed);
}

#[test]
fn a_finished_setup_leaves_prefilled_connections() {
    let mut report = SetupReport::default();
    report.feed(&format!("fingerprint: alice {FP}"));
    report.feed(&format!("fingerprint: system {SYSTEM_FP}"));

    let profiles = connection_profiles(&options(), "dev.lan", &report);
    assert_eq!(profiles.len(), 1);
    let headless = &profiles[0];
    // The same link drift-gnome-setup prints for this machine.
    assert_eq!(
        link::format(headless),
        format!("drift-connect:?name=dev&host=dev.lan&port=3390&mode=headless&user=alice&pin={FP}")
    );

    let mut o = options();
    o.remote_login = true;
    let profiles = connection_profiles(&o, "dev.lan", &report);
    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[1].mode, ConnectMode::RemoteLogin);
    assert_eq!(
        link::format(&profiles[1]),
        format!(
            "drift-connect:?name=dev%20login&host=dev.lan&port=3389&mode=remote-login&user=alice&pin={SYSTEM_FP}"
        )
    );

    // No fingerprint reported: Drift asks to trust the certificate on first connect.
    let bare = connection_profiles(&options(), "dev.lan", &SetupReport::default());
    assert_eq!(bare[0].cert_pin, None);
}
