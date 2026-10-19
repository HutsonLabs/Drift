//! HOST-9 Red: the AppKit humble object — the `NSMenu` mirrors the model (titles, separators,
//! submenus, check marks, disabled status line) and a status item can be created and removed.
//! Runs on the main thread (`support/harness.rs`); needs a window-server session.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[path = "support/harness.rs"]
mod harness;

use std::process::ExitCode;

use drift_host_app::model::{DeviceSetupInfo, MenuAction, MenuEntry, device_setup};
use drift_host_app::setup_window::SetupWindow;
use drift_host_app::status_item::{StatusItem, build_menu};
use objc2::MainThreadMarker;
use objc2_app_kit::NSControlStateValueOn;

fn main() -> ExitCode {
    harness::run(&[
        ("ns_menu_mirrors_the_model", ns_menu_mirrors_the_model),
        ("status_item_can_be_created_and_removed", status_item_can_be_created_and_removed),
        ("setup_window_shows_the_code_and_qr", setup_window_shows_the_code_and_qr),
        ("an_unknown_account_cannot_be_checked", an_unknown_account_cannot_be_checked),
    ])
}

fn model() -> Vec<MenuEntry> {
    vec![
        MenuEntry::Text("Waiting on 127.0.0.1:3389 (this Mac only)".into()),
        MenuEntry::Separator,
        MenuEntry::Submenu {
            title: "Paired Devices".into(),
            entries: vec![MenuEntry::Text("iPad Pro".into())],
        },
        MenuEntry::Item {
            title: "Start at Login".into(),
            action: MenuAction::ToggleStartAtLogin,
            checked: true,
        },
        MenuEntry::Separator,
        MenuEntry::Item { title: "Quit Drift Host".into(), action: MenuAction::Quit, checked: false },
    ]
}

fn ns_menu_mirrors_the_model() {
    let mtm = MainThreadMarker::new().unwrap();
    let menu = build_menu(mtm, &model(), |_| {});
    assert_eq!(menu.numberOfItems(), 6);
    let item = |i| menu.itemAtIndex(i).unwrap();
    assert_eq!(item(0).title().to_string(), "Waiting on 127.0.0.1:3389 (this Mac only)");
    assert!(!item(0).isEnabled(), "the status line is not clickable");
    assert!(item(1).isSeparatorItem());
    let sub = item(2).submenu().expect("submenu");
    assert_eq!(sub.itemAtIndex(0).unwrap().title().to_string(), "iPad Pro");
    assert_eq!(item(3).state(), NSControlStateValueOn);
    assert!(item(3).isEnabled());
    assert_eq!(item(5).title().to_string(), "Quit Drift Host");
}

fn status_item_can_be_created_and_removed() {
    let mtm = MainThreadMarker::new().unwrap();
    let item = StatusItem::new(mtm);
    item.set_menu(&build_menu(mtm, &model(), |_| {}));
    item.set_attention(true);
    assert!(item.has_button());
    drop(item);
}

// HOST-15: the device window opens with the code and QR, and closes.
fn setup_window_shows_the_code_and_qr() {
    let mtm = MainThreadMarker::new().unwrap();
    let setup = device_setup(&DeviceSetupInfo {
        mac_name: "Studio".into(),
        host: "studio.local".into(),
        port: 3389,
        account: "andrew".into(),
        code: Some("1234 5678".into()),
        minutes_left: Some(5),
    });
    let width = 25;
    let dark: Vec<bool> = (0..width * width).map(|i| i % 3 == 0).collect();
    let window = SetupWindow::show(mtm, &setup, Some("1234 5678"), Some(&(width, dark)), |_| {});
    assert!(window.is_visible());
    window.close();
    assert!(!window.is_visible());
}

// HOST-15: the password check answers without a prompt, and cannot tell for a stranger.
fn an_unknown_account_cannot_be_checked() {
    assert_eq!(drift_host_app::account::verify_password("no-such-user-drift-test", "fake"), None);
}
