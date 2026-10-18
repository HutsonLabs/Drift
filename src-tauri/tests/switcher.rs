//! IOS-11 Red: the in-app session switcher (`drift_app::switcher`).
//!
//! On iPhone every session window stacks in the app's one scene, so the only way between them is
//! a switcher inside the app. It lists Connections first, then every session window in opening
//! order with its name and status, and marks the window it is shown in.

use drift_app::connections::{CONNECTIONS_WINDOW, ConnectionStatus};
use drift_app::switcher::{SwitcherEntry, entries, target};

fn s(label: &str, name: &str, status: ConnectionStatus) -> (String, String, ConnectionStatus) {
    (label.to_owned(), name.to_owned(), status)
}

#[test]
fn connections_first_then_sessions_in_order_with_the_current_one_checked() {
    let list = entries(
        "session-2",
        &[
            s("session-0", "Homelab", ConnectionStatus::Live),
            s("session-2", "Office", ConnectionStatus::Reconnecting),
        ],
    );
    assert_eq!(
        list,
        vec![
            SwitcherEntry {
                id: CONNECTIONS_WINDOW.into(),
                title: "Connections".into(),
                subtitle: None,
                current: false
            },
            SwitcherEntry {
                id: "session-0".into(),
                title: "Homelab".into(),
                subtitle: Some("Live".into()),
                current: false
            },
            SwitcherEntry {
                id: "session-2".into(),
                title: "Office".into(),
                subtitle: Some("Reconnecting".into()),
                current: true
            },
        ]
    );
}

#[test]
fn names_are_cleaned_and_every_status_has_a_word() {
    let list = entries(
        CONNECTIONS_WINDOW,
        &[
            s("session-0", " \u{7}", ConnectionStatus::Connecting),
            s("session-1", "B", ConnectionStatus::Failed),
            s("session-3", "C", ConnectionStatus::Idle),
        ],
    );
    assert!(list[0].current, "shown from Connections");
    assert_eq!(list[1].title, "Untitled");
    assert_eq!(list[1].subtitle.as_deref(), Some("Connecting"));
    assert_eq!(list[2].subtitle.as_deref(), Some("Disconnected"));
    assert_eq!(list[3].subtitle, None);
}

#[test]
fn picking_maps_back_to_a_window_or_connections() {
    assert_eq!(
        target(CONNECTIONS_WINDOW, &["session-0".into()]),
        Some(drift_app::switcher::Target::Connections)
    );
    assert_eq!(
        target("session-0", &["session-0".into()]),
        Some(drift_app::switcher::Target::Session("session-0".into()))
    );
    assert_eq!(target("session-9", &["session-0".into()]), None, "a window that closed meanwhile");
}
