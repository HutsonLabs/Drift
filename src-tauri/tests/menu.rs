//! M6-2 / M2-4 / M8-3 / UI-windows Red: the menu bar model.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures

use drift_app::connections::ConnectionStatus;
use drift_app::menu::{
    MenuAction, MenuEntry, SessionItem, SubmenuSpec, accelerator, file_titles, menu_spec, window_sessions,
};
use drift_input::MenuShortcut;

/// `(menu, action, title, shortcut, enabled, checked)` of every Drift command.
type Row = (String, MenuAction, String, Option<MenuShortcut>, bool, Option<bool>);

fn actions(spec: &[SubmenuSpec]) -> Vec<Row> {
    spec.iter()
        .flat_map(|m| {
            m.entries.iter().filter_map(|e| match e {
                MenuEntry::Action { action, title, shortcut, enabled, checked } => {
                    Some((m.title.clone(), *action, title.clone(), *shortcut, *enabled, *checked))
                }
                _ => None,
            })
        })
        .collect()
}

fn find(spec: &[SubmenuSpec], action: MenuAction) -> Row {
    actions(spec)
        .into_iter()
        .find(|row| row.1 == action)
        .unwrap_or_else(|| panic!("{action:?} is not in the menu"))
}

fn menu<'a>(spec: &'a [SubmenuSpec], title: &str) -> &'a SubmenuSpec {
    spec.iter().find(|m| m.title == title).unwrap()
}

fn item(window: &str, name: &str, glyph: char, status: ConnectionStatus) -> SessionItem {
    SessionItem { window: window.into(), name: name.into(), glyph, status }
}

fn three() -> Vec<SessionItem> {
    vec![
        item("session-4", "Homelab", '●', ConnectionStatus::Live),
        item("session-1", "Build box", '◌', ConnectionStatus::Connecting),
        item("session-9", "Kiosk", '⚠', ConnectionStatus::Failed),
    ]
}

#[test]
fn action_ids_round_trip() {
    let mut all = vec![
        MenuAction::NewConnection,
        MenuAction::ShowConnections,
        MenuAction::EditConnection,
        MenuAction::Disconnect,
        MenuAction::CloseWindow,
        MenuAction::SendCtrlAltDel,
        MenuAction::Reconnect,
        MenuAction::ToggleStats,
        MenuAction::Quit,
        MenuAction::ToggleRecording,
    ];
    all.extend([1, 2, 9, 10, 42].map(MenuAction::SelectSession));
    let mut ids = std::collections::HashSet::new();
    for a in all {
        assert!(ids.insert(a.id()), "duplicate id {}", a.id());
        assert_eq!(MenuAction::from_id(&a.id()), Some(a), "{a:?}");
    }
    for bad in
        ["", "nope", "drift.select-session.0", "drift.select-session.x", "drift.new-tab", "drift.close-tab"]
    {
        assert_eq!(MenuAction::from_id(bad), None, "{bad}");
    }
}

/// UI-windows decision 9: File ▸ New Connection…, Show Connections, Edit <name>…,
/// Disconnect <name>, Close Window — New Tab and New Window are gone.
#[test]
fn file_menu() {
    let sessions = three();
    let spec = menu_spec(&sessions, Some("session-4"));
    let file = menu(&spec, "File");
    let rows: Vec<_> = actions(std::slice::from_ref(file))
        .into_iter()
        .map(|(_, action, title, shortcut, enabled, _)| (action, title, shortcut, enabled))
        .collect();
    assert_eq!(
        rows,
        [
            (
                MenuAction::NewConnection,
                "New Connection…".to_owned(),
                Some(MenuShortcut::NewConnection),
                true
            ),
            (
                MenuAction::ShowConnections,
                "Show Connections".to_owned(),
                Some(MenuShortcut::ShowConnections),
                true
            ),
            (
                MenuAction::EditConnection,
                "Edit Homelab…".to_owned(),
                Some(MenuShortcut::EditConnection),
                true
            ),
            (MenuAction::Disconnect, "Disconnect Homelab".to_owned(), Some(MenuShortcut::Disconnect), true),
            (MenuAction::CloseWindow, "Close Window".to_owned(), Some(MenuShortcut::CloseWindow), true),
        ]
    );
    let separators = file.entries.iter().filter(|e| **e == MenuEntry::Separator).count();
    assert_eq!(separators, 2, "New/Show | Edit/Disconnect | Close");
    let titles: Vec<String> = actions(&spec).into_iter().map(|r| r.2).collect();
    for gone in ["New Tab", "New Window", "Close Tab", "Show Previous Tab", "Show Next Tab"] {
        assert!(!titles.iter().any(|t| t == gone), "{gone} is gone");
    }
}

#[test]
fn file_titles_follow_the_key_session_window() {
    let homelab = item("session-4", "Homelab", '●', ConnectionStatus::Live);
    let t = file_titles(Some(&homelab));
    assert_eq!(
        (t.edit.as_str(), t.disconnect.as_str(), t.enabled),
        ("Edit Homelab…", "Disconnect Homelab", true)
    );
    let t = file_titles(None);
    assert_eq!(
        (t.edit.as_str(), t.disconnect.as_str(), t.enabled),
        ("Edit Connection…", "Disconnect", false)
    );

    // The Connections window (or no window) is key: the items are generic and disabled.
    for key in [Some("connections"), None, Some("session-77")] {
        let spec = menu_spec(&three(), key);
        let (_, _, title, _, enabled, _) = find(&spec, MenuAction::EditConnection);
        assert_eq!((title.as_str(), enabled), ("Edit Connection…", false), "{key:?}");
        let (_, _, title, _, enabled, _) = find(&spec, MenuAction::Disconnect);
        assert_eq!((title.as_str(), enabled), ("Disconnect", false), "{key:?}");
    }
}

/// UI-windows decision 9: Session keeps Send Ctrl+Alt+Del, Reconnect and Show Statistics, all
/// without shortcuts (every other Command combo belongs to the remote desktop).
#[test]
fn session_menu_has_no_shortcuts() {
    let spec = menu_spec(&[], None);
    let session: Vec<_> = actions(&spec)
        .into_iter()
        .filter(|(m, ..)| m == "Session")
        .map(|(_, a, t, s, ..)| (a, t, s))
        .collect();
    assert_eq!(
        session,
        [
            (MenuAction::SendCtrlAltDel, "Send Ctrl+Alt+Del".to_owned(), None),
            (MenuAction::Reconnect, "Reconnect".to_owned(), None),
            // M1 "Done (manual M1)": the fps HUD over the live picture, off by default.
            (MenuAction::ToggleStats, "Show Statistics".to_owned(), None),
        ]
    );
}

/// UI-windows decision 9: Window ▸ Minimize, Zoom, Full Screen, Connections (Cmd+0), then the
/// "Sessions" section Drift owns; no Show Previous/Next Tab.
#[test]
fn window_menu_lists_connections_and_the_sessions() {
    let spec = menu_spec(&three(), Some("session-1"));
    let window = menu(&spec, "Window");
    use drift_app::menu::Standard as S;
    assert_eq!(
        window.entries[..6],
        [
            MenuEntry::Standard(S::Minimize),
            MenuEntry::Standard(S::Zoom),
            MenuEntry::Standard(S::Fullscreen),
            MenuEntry::Separator,
            MenuEntry::Action {
                action: MenuAction::ShowConnections,
                title: "Connections".into(),
                shortcut: Some(MenuShortcut::ShowConnections),
                enabled: true,
                checked: None,
            },
            MenuEntry::Separator,
        ]
    );
    assert_eq!(window.entries[6..], window_sessions(&three(), Some("session-1"))[..]);
}

#[test]
fn the_sessions_section_numbers_and_checks_the_windows() {
    let entries = window_sessions(&three(), Some("session-1"));
    assert_eq!(entries[0], MenuEntry::Header("Sessions".into()));
    let rows: Vec<_> = entries[1..]
        .iter()
        .map(|e| match e {
            MenuEntry::Action { action, title, shortcut, enabled, checked } => {
                (*action, title.clone(), *shortcut, *enabled, *checked)
            }
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                MenuAction::SelectSession(1),
                "● Homelab".to_owned(),
                Some(MenuShortcut::SelectSession(1)),
                true,
                Some(false)
            ),
            (
                MenuAction::SelectSession(2),
                "◌ Build box".to_owned(),
                Some(MenuShortcut::SelectSession(2)),
                true,
                Some(true)
            ),
            (
                MenuAction::SelectSession(3),
                "⚠ Kiosk".to_owned(),
                Some(MenuShortcut::SelectSession(3)),
                true,
                Some(false)
            ),
        ]
    );
    // No session windows: no section at all.
    assert!(window_sessions(&[], None).is_empty());
    // Cmd+1…9 on the first nine only.
    let many: Vec<_> = (0..11)
        .map(|n| item(&format!("session-{n}"), &format!("S{n}"), '●', ConnectionStatus::Live))
        .collect();
    let entries = window_sessions(&many, None);
    assert_eq!(entries.len(), 12);
    for (n, entry) in entries[1..].iter().enumerate() {
        let MenuEntry::Action { action, shortcut, checked, .. } = entry else { panic!() };
        let n = u32::try_from(n + 1).unwrap();
        assert_eq!(*action, MenuAction::SelectSession(n));
        assert_eq!(*checked, Some(false), "the Connections window is key: nothing checked");
        let want = (n <= 9).then(|| MenuShortcut::SelectSession(u8::try_from(n).unwrap()));
        assert_eq!(*shortcut, want, "#{n}");
    }
}

#[test]
fn quit_stays_in_the_drift_menu() {
    let spec = menu_spec(&[], None);
    let (menu, _, _, shortcut, ..) = find(&spec, MenuAction::Quit);
    assert_eq!((menu.as_str(), shortcut), ("Drift", Some(MenuShortcut::Quit)));
}

#[test]
fn menu_order() {
    let titles: Vec<_> = menu_spec(&[], None).into_iter().map(|m| m.title).collect();
    if cfg!(feature = "recording") {
        assert_eq!(titles, ["Drift", "File", "Edit", "Session", "Window", "Debug"]);
    } else {
        assert_eq!(titles, ["Drift", "File", "Edit", "Session", "Window"]);
    }
}

#[test]
fn record_session_is_hidden_without_the_recording_feature() {
    let spec = menu_spec(&[], None);
    let recording = actions(&spec).into_iter().find(|row| row.1 == MenuAction::ToggleRecording);
    if cfg!(feature = "recording") {
        let (menu, _, title, shortcut, ..) = recording.expect("Debug ▸ Record Session");
        assert_eq!(
            (menu.as_str(), title.as_str(), shortcut),
            ("Debug", "Record Session (experimental)", None)
        );
    } else {
        assert!(recording.is_none());
    }
}

#[test]
fn accelerators() {
    #[rustfmt::skip]
    let table = [
        (MenuShortcut::NewConnection, "CmdOrCtrl+N"),
        (MenuShortcut::ShowConnections, "CmdOrCtrl+0"),
        (MenuShortcut::EditConnection, "CmdOrCtrl+E"),
        (MenuShortcut::Disconnect, "CmdOrCtrl+Shift+D"),
        (MenuShortcut::CloseWindow, "CmdOrCtrl+W"),
        (MenuShortcut::Quit, "CmdOrCtrl+Q"),
        (MenuShortcut::SelectSession(1), "CmdOrCtrl+1"),
        (MenuShortcut::SelectSession(9), "CmdOrCtrl+9"),
        (MenuShortcut::CycleWindows, "CmdOrCtrl+Backquote"),
    ];
    for (s, want) in table {
        assert_eq!(accelerator(s), want, "{s:?}");
        // Tauri hands the string to muda, which parses it at runtime.
        assert!(want.parse::<muda::accelerator::Accelerator>().is_ok(), "{want} parses");
    }
}
