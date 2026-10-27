//! M0-4 Red: `host-setup-check` parsers and evaluation, on reports captured from the homelab
//! host with `host/drift-host-report.sh` (before and after `drift-host-setup.sh --apply`).

use xtask::host_check::{
    Expectations, HeadlessSpec, evaluate, parse_grdctl_status, parse_ps, parse_report, parse_ss_ltnp,
    split_sections,
};

const BEFORE: &str = include_str!("data/host/report_before.txt");
const AFTER: &str = include_str!("data/host/report_after.txt");

fn section_body(report: &str, name: &str, arg: Option<&str>) -> String {
    split_sections(report)
        .into_iter()
        .find(|s| s.name == name && arg.is_none_or(|a| s.args.first().map(String::as_str) == Some(a)))
        .map(|s| s.body.join("\n"))
        .unwrap_or_else(|| panic!("no section {name} {arg:?}"))
}

#[test]
fn headless_spec_parses_and_displays() {
    let s: HeadlessSpec = "drifttest2:3392:persistent".parse().unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(s, HeadlessSpec { user: "drifttest2".into(), port: 3392, persistent: true });
    assert_eq!(s.to_string(), "drifttest2:3392:persistent");
    let s: HeadlessSpec = "drifttest:3391".parse().unwrap_or_else(|e| panic!("{e:#}"));
    assert!(!s.persistent);
    for bad in ["", "drifttest", "drifttest:", "drifttest:abc", ":3391", "u:3391:forever", "u:70000"] {
        assert!(bad.parse::<HeadlessSpec>().is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn sections_split_on_headers() {
    let s = split_sections("noise\n### a x y\n1\n2\n### b\n### end\n");
    assert_eq!(s.len(), 3);
    assert_eq!(s[0].name, "a");
    assert_eq!(s[0].args, vec!["x", "y"]);
    assert_eq!(s[0].body, vec!["1", "2"]);
    assert!(s[1].body.is_empty());
    assert_eq!(split_sections(AFTER).last().map(|s| s.name.as_str()), Some("end"));
}

#[test]
fn parses_grdctl_system_status() {
    let st = parse_grdctl_status(&section_body(AFTER, "grdctl-system", None));
    assert!(st.unit_active);
    assert_eq!(st.enabled, Some(true));
    assert_eq!(st.port, Some(3389));
    assert_eq!(st.negotiate_port, None, "the system daemon has no port negotiation");
    assert_eq!(
        st.tls_cert.as_deref(),
        Some("/var/lib/gnome-remote-desktop/.local/share/gnome-remote-desktop/certificates/rdp-tls.crt")
    );
    assert!(st.tls_key.is_some());
    assert_eq!(
        st.tls_fingerprint.as_deref(),
        Some(
            "f3:e7:a2:68:ee:a5:64:38:f8:14:03:12:50:4d:44:97:6c:dc:c3:ae:f2:4a:98:92:da:97:72:fe:b6:e6:fe:81"
        )
    );
    assert!(st.username_set && st.password_set);
}

#[test]
fn parses_grdctl_headless_status_before_and_after_pinning() {
    let before = parse_grdctl_status(&section_body(BEFORE, "grdctl-headless", Some("drifttest2")));
    assert_eq!(before.port, Some(3389), "unpinned: default port, negotiated at runtime");
    assert_eq!(before.negotiate_port, Some(true));
    let after = parse_grdctl_status(&section_body(AFTER, "grdctl-headless", Some("drifttest2")));
    assert_eq!(after.port, Some(3392));
    assert_eq!(after.negotiate_port, Some(false));
    assert_eq!(after.enabled, Some(true));
}

#[test]
fn grdctl_status_handles_unset_values() {
    let text = "Overall:\n\tUnit status: inactive\nRDP:\n\tStatus: disabled\n\tPort: 3389\n\
                \tTLS certificate: (null)\n\tTLS fingerprint: (null)\n\tTLS key: \n\
                \tNegotiate port: yes\n\tUsername: (empty)\n\tPassword: (empty)\n";
    let st = parse_grdctl_status(text);
    assert!(!st.unit_active);
    assert_eq!(st.enabled, Some(false));
    assert_eq!(st.tls_cert, None);
    assert_eq!(st.tls_key, None);
    assert_eq!(st.tls_fingerprint, None);
    assert!(!st.username_set && !st.password_set);
}

#[test]
fn parses_ss_ltnp_listeners() {
    let l = parse_ss_ltnp(&section_body(AFTER, "ss-ltnp", None));
    let grd: Vec<(u16, u32)> =
        l.iter().filter(|x| x.process == "gnome-remote-de").map(|x| (x.port, x.pid)).collect();
    let ports: Vec<u16> = grd.iter().map(|x| x.0).collect();
    for p in [3389, 3390, 3391, 3392] {
        assert!(ports.contains(&p), "port {p} in {ports:?}");
    }
    assert!(l.iter().any(|x| x.port == 22 && x.process == "sshd"));
    // Garbage and the header are skipped.
    assert!(parse_ss_ltnp("State Recv-Q\ngarbage line\n").is_empty());
}

#[test]
fn parses_ps_lines() {
    let p = parse_ps(&section_body(AFTER, "ps", None));
    assert!(p.iter().any(|x| x.user == "gnome-remote-desktop" && x.args.ends_with("--system")));
    assert!(p.iter().any(|x| x.user == "drifttest2" && x.args.ends_with("--headless")));
    assert!(p.iter().all(|x| x.pid > 0));
}

#[test]
fn full_report_parses() {
    let r = parse_report(AFTER).unwrap_or_else(|e| panic!("{e:#}"));
    assert!(r.complete);
    assert!(r.system.is_some());
    assert_eq!(r.headless.len(), 2);
    assert_eq!(r.certs.len(), 3);
    assert!(r.certs.iter().all(|(_, s)| s == "valid"));
    assert!(r.unit_props.iter().any(|(u, props)| u == "gnome-headless-session@drifttest2.service"
        && props.contains(&("DynamicUser".to_string(), "no".to_string()))));
    let truncated = AFTER.replace("### end", "");
    assert!(!parse_report(&truncated).unwrap_or_else(|e| panic!("{e:#}")).complete);
}

#[test]
fn host_after_setup_passes_every_check() {
    let r = parse_report(AFTER).unwrap_or_else(|e| panic!("{e:#}"));
    let checks = evaluate(&r, &Expectations::default());
    let failed: Vec<_> = checks.iter().filter(|c| !c.ok).collect();
    assert!(failed.is_empty(), "{failed:#?}");
    assert!(checks.len() >= 18, "expected a full set of checks, got {}", checks.len());
    for needle in [":3389", ":3391", ":3392", "drop-in", "gnome-headless-session@drifttest2", "TLS"] {
        assert!(checks.iter().any(|c| c.name.contains(needle)), "no check mentions {needle}");
    }
}

#[test]
fn host_before_setup_fails_exactly_the_pending_changes() {
    let r = parse_report(BEFORE).unwrap_or_else(|e| panic!("{e:#}"));
    let checks = evaluate(&r, &Expectations::default());
    let mut failed: Vec<&str> = checks.iter().filter(|c| !c.ok).map(|c| c.name.as_str()).collect();
    failed.sort();
    assert_eq!(
        failed,
        vec![
            "drifttest2: RDP port pinned to 3392",
            "drifttest2: gnome-headless-session@drifttest2.service enabled and active",
            "drifttest2: headless session restarts whenever it ends (Restart=always)",
            "drifttest: RDP port pinned to 3391",
        ]
    );
}

#[test]
fn broken_hosts_are_detected() {
    let exp = Expectations::default();
    let failing = |report: &str| -> Vec<String> {
        let r = parse_report(report).unwrap_or_else(|e| panic!("{e:#}"));
        evaluate(&r, &exp).into_iter().filter(|c| !c.ok).map(|c| c.name).collect()
    };

    // Headless daemon for drifttest2 is not listening.
    let no_listener: String =
        AFTER.lines().filter(|l| !l.contains("*:3392 ")).map(|l| format!("{l}\n")).collect();
    assert!(failing(&no_listener).iter().any(|n| n.contains(":3392")));

    // System RDP disabled and its certificate expired.
    let mut broken = String::new();
    let mut in_system = false;
    let mut cert_owner = String::new();
    for l in AFTER.lines() {
        if let Some(h) = l.strip_prefix("### ") {
            in_system = h == "grdctl-system";
            cert_owner = h.strip_prefix("cert ").unwrap_or_default().to_string();
            broken.push_str(l);
        } else if in_system && l.trim() == "Status: enabled" {
            broken.push_str("\tStatus: disabled");
        } else if cert_owner == "system" {
            broken.push_str("expired");
        } else {
            broken.push_str(l);
        }
        broken.push('\n');
    }
    let f = failing(&broken);
    assert!(f.iter().any(|n| n == "system: RDP enabled"), "{f:?}");
    assert!(f.iter().any(|n| n == "system: TLS certificate valid"), "{f:?}");

    // Drop-in missing: the unit runs with DynamicUser=yes.
    let dynamic = AFTER.replace("DynamicUser=no", "DynamicUser=yes");
    assert!(failing(&dynamic).iter().any(|n| n.contains("drop-in")));

    // GNOME-WIZARD: a root drop-in (what the setup installs now) passes as well as User=gdm.
    let root = AFTER.replace("User=gdm", "User=root");
    assert!(!failing(&root).iter().any(|n| n.contains("drop-in")), "{:?}", failing(&root));
    let other = AFTER.replace("User=gdm", "User=nobody");
    assert!(failing(&other).iter().any(|n| n.contains("drop-in")));

    // The shipped Restart=on-failure leaves the session dead once it ends (10.1.2.5, 2026-10-24).
    let on_failure = AFTER.replace("Restart=always", "Restart=on-failure");
    assert!(failing(&on_failure).iter().any(|n| n.contains("Restart=always")), "{:?}", failing(&on_failure));

    // A truncated report fails as a whole.
    assert!(failing(&AFTER.replace("### end", "")).iter().any(|n| n.contains("report complete")));
}

/// `--no-system` (a headless-only machine, 10.1.2.5): Remote Login must be off, and the
/// headless checks still run.
#[test]
fn no_system_checks_that_remote_login_is_off() {
    let exp = Expectations { system: false, ..Expectations::default() };
    let failing = |report: &str| -> Vec<String> {
        let r = parse_report(report).unwrap_or_else(|e| panic!("{e:#}"));
        evaluate(&r, &exp).into_iter().filter(|c| !c.ok).map(|c| c.name).collect()
    };
    // AFTER has Remote Login on: exactly the two "off" checks fail.
    let mut on = failing(AFTER);
    on.sort();
    assert_eq!(
        on,
        vec![
            "system: RDP disabled (Remote Login off)",
            "system: gnome-remote-desktop.service disabled and stopped (Remote Login off)",
        ]
    );
    let r = parse_report(AFTER).unwrap_or_else(|e| panic!("{e:#}"));
    assert!(evaluate(&r, &exp).iter().any(|c| c.name.contains("Restart=always")), "headless checks still run");
}
