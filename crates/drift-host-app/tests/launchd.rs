//! HOST-13 Red: the launchd property lists the bundle carries for the daemon, the per-user agent
//! and the login-window agent, registered through `SMAppService` (never installed by tests).
#![allow(missing_docs, clippy::unwrap_used)]

use drift_host_app::services::{AGENT_PLIST, DAEMON_PLIST, LOGINWINDOW_PLIST, Service, all};

fn read(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("launchd").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn has(plist: &str, key: &str, value: &str) -> bool {
    let marker = format!("<key>{key}</key>");
    plist.find(&marker).is_some_and(|i| plist[i + marker.len()..].trim_start().starts_with(value))
}

#[test]
fn three_services_with_their_plists() {
    let names: Vec<&str> = all().iter().map(Service::plist).collect();
    assert_eq!(names, [DAEMON_PLIST, AGENT_PLIST, LOGINWINDOW_PLIST]);
    assert_eq!(DAEMON_PLIST, "com.hutsonlabs.drift.host.daemon.plist");
}

#[test]
fn the_daemon_runs_the_broker_from_the_bundle() {
    let p = read(DAEMON_PLIST);
    assert!(has(&p, "Label", "<string>com.hutsonlabs.drift.host.daemon</string>"), "{p}");
    assert!(has(&p, "BundleProgram", "<string>Contents/MacOS/drift-host</string>"), "{p}");
    assert!(p.contains("<string>daemon</string>"), "{p}");
    assert!(has(&p, "AssociatedBundleIdentifiers", "<array>"), "{p}");
    assert!(p.contains("<string>com.hutsonlabs.drift.host</string>"));
    assert!(has(&p, "KeepAlive", "<true/>"));
}

#[test]
fn the_agents_run_in_the_user_session_and_at_the_login_window() {
    let aqua = read(AGENT_PLIST);
    assert!(has(&aqua, "LimitLoadToSessionType", "<string>Aqua</string>"), "{aqua}");
    assert!(aqua.contains("<string>agent</string>") && aqua.contains("<string>aqua</string>"));
    let lw = read(LOGINWINDOW_PLIST);
    assert!(has(&lw, "LimitLoadToSessionType", "<string>LoginWindow</string>"), "{lw}");
    assert!(lw.contains("<string>loginwindow</string>"));
    for p in [&aqua, &lw] {
        assert!(has(p, "BundleProgram", "<string>Contents/MacOS/drift-host</string>"));
        assert!(has(p, "RunAtLoad", "<true/>"));
    }
}
