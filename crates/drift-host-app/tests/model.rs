//! HOST-9 Red: the menu-bar app's view model (pure): the status line, the menu, the icon state,
//! and plain-language first-run guidance for the permissions Drift Host needs.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_host_app::model::{
    Grant, HostStatus, IconState, MenuAction, MenuEntry, MenuInput, PairedDevice, Permission, Permissions,
    first_run, guidance, icon_state, menu, status_line,
};

fn all_granted() -> Permissions {
    Permissions {
        screen_recording: Grant::Granted,
        accessibility: Grant::Granted,
        local_network: Grant::Granted,
    }
}

fn input(status: HostStatus) -> MenuInput {
    MenuInput { status, permissions: all_granted(), devices: None, start_at_login: false }
}

fn titles(entries: &[MenuEntry]) -> Vec<String> {
    let mut out = Vec::new();
    for e in entries {
        match e {
            MenuEntry::Text(t) => out.push(t.clone()),
            MenuEntry::Item { title, .. } => out.push(title.clone()),
            MenuEntry::Submenu { title, entries } => {
                out.push(title.clone());
                out.extend(titles(entries).into_iter().map(|t| format!("  {t}")));
            }
            MenuEntry::Separator => out.push("—".into()),
        }
    }
    out
}

fn actions(entries: &[MenuEntry]) -> Vec<MenuAction> {
    let mut out = Vec::new();
    for e in entries {
        match e {
            MenuEntry::Item { action, .. } => out.push(*action),
            MenuEntry::Submenu { entries, .. } => out.extend(actions(entries)),
            _ => {}
        }
    }
    out
}

#[test]
fn status_lines_say_what_the_host_is_doing() {
    assert_eq!(status_line(&HostStatus::Starting), "Starting…");
    let local = HostStatus::Listening { addr: "127.0.0.1:3389".into(), loopback_only: true };
    assert_eq!(status_line(&local), "Waiting on 127.0.0.1:3389 (this Mac only)");
    let lan = HostStatus::Listening { addr: "0.0.0.0:3389".into(), loopback_only: false };
    assert_eq!(status_line(&lan), "Waiting for a device on port 3389");
    assert_eq!(
        status_line(&HostStatus::Connected { device: Some("Andrew's iPad".into()) }),
        "Connected: Andrew's iPad"
    );
    assert_eq!(status_line(&HostStatus::Connected { device: None }), "A device is connected");
    let stopped = HostStatus::Stopped { error: "port 3389 is in use".into() };
    assert_eq!(status_line(&stopped), "Stopped: port 3389 is in use");
}

#[test]
fn the_menu_starts_with_the_status_and_ends_with_quit() {
    let m = menu(&input(HostStatus::Listening { addr: "127.0.0.1:3389".into(), loopback_only: true }));
    let t = titles(&m);
    assert_eq!(t.first().map(String::as_str), Some("Waiting on 127.0.0.1:3389 (this Mac only)"));
    assert_eq!(t.last().map(String::as_str), Some("Quit Drift Host"));
    assert_eq!(actions(&m).last(), Some(&MenuAction::Quit));
    assert!(matches!(m.first(), Some(MenuEntry::Text(_))), "the status line is not clickable");
}

#[test]
fn without_pairing_the_menu_says_when_it_arrives() {
    let t = titles(&menu(&input(HostStatus::Starting)));
    assert!(t.iter().any(|s| s == "Paired devices: none yet (pairing comes in a later version)"), "{t:?}");
}

#[test]
fn paired_devices_are_listed_in_a_submenu() {
    let mut i = input(HostStatus::Starting);
    i.devices = Some(vec![
        PairedDevice { name: "iPad Pro".into(), last_seen: Some("today".into()) },
        PairedDevice { name: "iPhone".into(), last_seen: None },
    ]);
    let t = titles(&menu(&i));
    let at = t.iter().position(|s| s == "Paired Devices").expect("submenu");
    assert_eq!(t[at + 1], "  iPad Pro — last seen today");
    assert_eq!(t[at + 2], "  iPhone");
    i.devices = Some(Vec::new());
    assert!(titles(&menu(&i)).iter().any(|s| s == "No paired devices"));
}

#[test]
fn missing_permissions_get_an_item_that_opens_their_settings() {
    let mut i = input(HostStatus::Starting);
    i.permissions.screen_recording = Grant::Missing;
    i.permissions.accessibility = Grant::Missing;
    let m = menu(&i);
    let t = titles(&m);
    assert!(t.iter().any(|s| s == "Allow Screen Recording…"), "{t:?}");
    assert!(t.iter().any(|s| s == "Allow Accessibility…"), "{t:?}");
    let a = actions(&m);
    assert!(a.contains(&MenuAction::OpenSettings(Permission::ScreenRecording)));
    assert!(a.contains(&MenuAction::OpenSettings(Permission::Accessibility)));
    assert!(!a.contains(&MenuAction::OpenSettings(Permission::LocalNetwork)));
    // All granted: no permission items at all.
    let t = titles(&menu(&input(HostStatus::Starting)));
    assert!(!t.iter().any(|s| s.starts_with("Allow ")), "{t:?}");
}

#[test]
fn start_at_login_is_a_checkbox() {
    let mut i = input(HostStatus::Starting);
    let find = |m: &[MenuEntry]| {
        m.iter()
            .find_map(|e| match e {
                MenuEntry::Item { title, checked, action } if title == "Start at Login" => {
                    Some((*checked, *action))
                }
                _ => None,
            })
            .expect("Start at Login item")
    };
    assert_eq!(find(&menu(&i)), (false, MenuAction::ToggleStartAtLogin));
    i.start_at_login = true;
    assert!(find(&menu(&i)).0);
}

#[test]
fn the_icon_asks_for_attention_until_capture_and_input_are_allowed() {
    let listening = HostStatus::Listening { addr: "127.0.0.1:3389".into(), loopback_only: true };
    assert_eq!(icon_state(&listening, &all_granted()), IconState::Idle);
    assert_eq!(icon_state(&HostStatus::Connected { device: None }, &all_granted()), IconState::Connected);
    let mut p = all_granted();
    p.accessibility = Grant::Missing;
    assert_eq!(icon_state(&listening, &p), IconState::Attention);
    assert_eq!(icon_state(&HostStatus::Stopped { error: "x".into() }, &all_granted()), IconState::Attention);
    // Local Network cannot be checked in advance; unknown is not an alarm.
    let mut p = all_granted();
    p.local_network = Grant::Unknown;
    assert_eq!(icon_state(&listening, &p), IconState::Idle);
}

#[test]
fn guidance_is_plain_and_points_at_the_right_pane() {
    let s = guidance(Permission::ScreenRecording);
    assert_eq!(s.title, "Allow Screen Recording");
    assert!(s.why.contains("see your screen"), "{}", s.why);
    assert!(s.settings_url.ends_with("Privacy_ScreenCapture"), "{}", s.settings_url);
    assert!(s.steps.iter().any(|step| step.contains("Drift Host")));
    let a = guidance(Permission::Accessibility);
    assert!(a.why.contains("keyboard and mouse"), "{}", a.why);
    assert!(a.settings_url.ends_with("Privacy_Accessibility"));
    let l = guidance(Permission::LocalNetwork);
    assert!(l.settings_url.ends_with("Privacy_LocalNetwork"));
    assert!(l.steps.iter().any(|step| step.contains("Allow")), "{:?}", l.steps);
    for g in [s, a, l] {
        assert!(g.steps.iter().all(|step| step.len() < 120 && !step.contains("TCC")), "{:?}", g.steps);
    }
}

#[test]
fn first_run_lists_only_what_is_still_needed_in_order() {
    let mut p = Permissions {
        screen_recording: Grant::Missing,
        accessibility: Grant::Missing,
        local_network: Grant::Unknown,
    };
    let titles: Vec<_> = first_run(&p).into_iter().map(|g| g.title).collect();
    assert_eq!(titles, ["Allow Screen Recording", "Allow Accessibility", "Allow Local Network"]);
    p.screen_recording = Grant::Granted;
    p.local_network = Grant::Granted;
    let titles: Vec<_> = first_run(&p).into_iter().map(|g| g.title).collect();
    assert_eq!(titles, ["Allow Accessibility"]);
    assert!(first_run(&all_granted()).is_empty());
}

#[test]
fn the_first_run_alert_explains_the_first_missing_permission() {
    use drift_host_app::model::first_run_alert;
    let p = Permissions {
        screen_recording: Grant::Missing,
        accessibility: Grant::Missing,
        local_network: Grant::Unknown,
    };
    let alert = first_run_alert(&p).expect("something is missing");
    assert_eq!(alert.title, "Drift Host needs 2 permissions");
    assert!(alert.body.starts_with("Drift Host needs it to see your screen"), "{}", alert.body);
    assert!(alert.body.contains("1. Open System Settings"), "{}", alert.body);
    assert_eq!(alert.open, Permission::ScreenRecording);
    let mut p = all_granted();
    p.accessibility = Grant::Missing;
    let alert = first_run_alert(&p).unwrap();
    assert_eq!(alert.title, "Drift Host needs Accessibility");
    assert_eq!(alert.open, Permission::Accessibility);
    // Local Network alone is asked by macOS itself when a device connects: no alert.
    let mut p = all_granted();
    p.local_network = Grant::Unknown;
    assert_eq!(first_run_alert(&p), None);
}
