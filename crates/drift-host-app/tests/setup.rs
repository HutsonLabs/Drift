//! HOST-15 Red: guided setup in the menu-bar app (ADR `HOST-15-guided-setup`). Permissions, then
//! the Mac's password, then a window that tells the user how to add a device (with the code and
//! QR as a fallback), and an Allow / Don't Allow prompt when a device asks.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_host_app::model::{
    DeviceSetupInfo, Grant, HostStatus, MenuAction, MenuEntry, MenuInput, PairedDevice, Permission,
    Permissions, SetupStep, approval_prompt, auto_show, device_setup, menu, password_prompt, setup_step,
    status_line,
};

fn granted() -> Permissions {
    Permissions {
        screen_recording: Grant::Granted,
        accessibility: Grant::Granted,
        local_network: Grant::Unknown,
    }
}

fn listening() -> HostStatus {
    HostStatus::Listening { addr: "0.0.0.0:3389".into(), loopback_only: false }
}

fn actions(entries: &[MenuEntry]) -> Vec<MenuAction> {
    entries
        .iter()
        .filter_map(|e| match e {
            MenuEntry::Item { action, .. } => Some(*action),
            _ => None,
        })
        .collect()
}

#[test]
fn setup_goes_permissions_then_password_then_a_device() {
    let missing = Permissions { screen_recording: Grant::Missing, ..granted() };
    assert_eq!(
        setup_step(&HostStatus::NeedsPassword, &missing, Some(0)),
        SetupStep::Permission(Permission::ScreenRecording)
    );
    let no_ax = Permissions { accessibility: Grant::Missing, ..granted() };
    assert_eq!(setup_step(&listening(), &no_ax, Some(0)), SetupStep::Permission(Permission::Accessibility));
    assert_eq!(setup_step(&HostStatus::NeedsPassword, &granted(), Some(0)), SetupStep::Password);
    assert_eq!(setup_step(&listening(), &granted(), Some(0)), SetupStep::AddDevice);
    assert_eq!(setup_step(&listening(), &granted(), Some(1)), SetupStep::Ready);
    assert_eq!(setup_step(&listening(), &granted(), None), SetupStep::Ready, "pairing off: nothing to add");
    assert_eq!(setup_step(&HostStatus::Starting, &granted(), Some(0)), SetupStep::Ready);
}

#[test]
fn the_next_window_opens_by_itself_once_per_step() {
    assert_eq!(auto_show(None, SetupStep::Password), Some(SetupStep::Password));
    assert_eq!(auto_show(None, SetupStep::AddDevice), Some(SetupStep::AddDevice));
    // Granting the last permission (or saving the password) moves on without a click.
    assert_eq!(
        auto_show(Some(SetupStep::Permission(Permission::Accessibility)), SetupStep::Password),
        Some(SetupStep::Password)
    );
    assert_eq!(auto_show(Some(SetupStep::Password), SetupStep::AddDevice), Some(SetupStep::AddDevice));
    // Closed by the user: not again until the step changes.
    assert_eq!(auto_show(Some(SetupStep::AddDevice), SetupStep::AddDevice), None);
    // The first-run alert covers permissions; Ready needs nothing.
    assert_eq!(auto_show(None, SetupStep::Permission(Permission::ScreenRecording)), None);
    assert_eq!(auto_show(Some(SetupStep::AddDevice), SetupStep::Ready), None);
}

#[test]
fn a_host_without_a_password_asks_for_it_in_the_menu() {
    assert_eq!(status_line(&HostStatus::NeedsPassword), "Set up: enter your Mac’s password");
    let input = MenuInput {
        status: HostStatus::NeedsPassword,
        permissions: granted(),
        devices: Some(Vec::new()),
        start_at_login: false,
    };
    let entries = menu(&input);
    assert_eq!(actions(&entries).first(), Some(&MenuAction::SetPassword));
    assert!(!actions(&entries).contains(&MenuAction::SetUpDevice), "no device before the host runs");
}

#[test]
fn a_running_host_offers_to_set_up_a_device_and_to_update_the_password() {
    let input = MenuInput {
        status: listening(),
        permissions: granted(),
        devices: Some(vec![PairedDevice { name: "iPad".into(), last_seen: None }]),
        start_at_login: false,
    };
    let entries = menu(&input);
    let a = actions(&entries);
    assert_eq!(a.first(), Some(&MenuAction::SetUpDevice), "{entries:?}");
    assert!(a.contains(&MenuAction::SetPassword), "the Mac's password may change");
    let title = entries.iter().find_map(|e| match e {
        MenuEntry::Item { title, action: MenuAction::SetUpDevice, .. } => Some(title.clone()),
        _ => None,
    });
    assert_eq!(title.as_deref(), Some("Set Up a Device…"));
}

#[test]
fn the_password_prompt_names_the_account() {
    let p = password_prompt("andrew", None);
    assert!(p.title.contains("andrew"), "{p:?}");
    assert!(p.body.contains("keychain"), "{p:?}");
    let again = password_prompt("andrew", Some("That password is not right."));
    assert!(again.body.starts_with("That password is not right."), "{again:?}");
}

fn info(code: Option<&str>) -> DeviceSetupInfo {
    DeviceSetupInfo {
        mac_name: "Studio".into(),
        host: "studio.local".into(),
        port: 3389,
        account: "andrew".into(),
        code: code.map(str::to_owned),
        minutes_left: code.map(|_| 5),
    }
}

#[test]
fn the_device_window_walks_through_adding_a_device() {
    let s = device_setup(&info(Some("1234 5678")));
    assert_eq!(s.title, "Set Up a Device");
    assert_eq!(s.steps.len(), 4, "{:?}", s.steps);
    assert!(s.steps[0].contains("Drift"), "{:?}", s.steps);
    assert!(s.steps[1].contains("“Studio”"), "{:?}", s.steps);
    assert!(s.steps[2].contains("andrew"), "{:?}", s.steps);
    assert!(s.steps[3].contains("Allow"), "{:?}", s.steps);
    assert_eq!(
        s.details,
        [
            ("Mac".to_owned(), "Studio".to_owned()),
            ("Address".to_owned(), "studio.local:3389".to_owned()),
            ("Account".to_owned(), "andrew".to_owned()),
            ("Password".to_owned(), "This Mac’s login password".to_owned()),
        ]
    );
    assert!(s.code_note.contains("1234 5678") && s.code_note.contains('5'), "{}", s.code_note);
    let expired = device_setup(&info(None));
    assert!(expired.code_note.contains("New Code"), "{}", expired.code_note);
}

#[test]
fn the_approval_prompt_names_the_device_and_defaults_to_safety() {
    let p = approval_prompt("Andrew’s iPad");
    assert_eq!(p.title, "Allow “Andrew’s iPad” to use this Mac?");
    assert!(p.body.contains("screen") && p.body.contains("keyboard"), "{}", p.body);
    assert_eq!((p.allow.as_str(), p.deny.as_str()), ("Allow", "Don’t Allow"));
}
