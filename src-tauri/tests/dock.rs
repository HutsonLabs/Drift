//! UI-windows Red: the Dock menu model (ADR UI-windows-gallery decision 10).
#![allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures

use drift_app::connections::ConnectionStatus;
use drift_app::dock::{DockAction, DockItem, dock_menu};
use drift_app::menu::SessionItem;

fn item(window: &str, name: &str, glyph: char, status: ConnectionStatus) -> SessionItem {
    SessionItem { window: window.into(), name: name.into(), glyph, status }
}

#[test]
fn sessions_then_connections_and_new_connection() {
    let sessions = [
        item("session-2", "Homelab", '●', ConnectionStatus::Live),
        item("session-0", "Kiosk", '↻', ConnectionStatus::Reconnecting),
    ];
    assert_eq!(
        dock_menu(&sessions),
        [
            DockItem::Header("Sessions".into()),
            DockItem::Action {
                action: DockAction::Focus("session-2".into()),
                title: "● Homelab".into(),
                key: None
            },
            DockItem::Action {
                action: DockAction::Focus("session-0".into()),
                title: "↻ Kiosk".into(),
                key: None
            },
            DockItem::Separator,
            DockItem::Action {
                action: DockAction::ShowConnections,
                title: "Connections".into(),
                key: Some('0')
            },
            DockItem::Action {
                action: DockAction::NewConnection,
                title: "New Connection…".into(),
                key: Some('n')
            },
        ]
    );
}

#[test]
fn without_sessions_only_the_two_commands_remain() {
    assert_eq!(
        dock_menu(&[]),
        [
            DockItem::Action {
                action: DockAction::ShowConnections,
                title: "Connections".into(),
                key: Some('0')
            },
            DockItem::Action {
                action: DockAction::NewConnection,
                title: "New Connection…".into(),
                key: Some('n')
            },
        ]
    );
}

#[test]
fn clicks_route_back_by_id() {
    for action in
        [DockAction::Focus("session-12".into()), DockAction::ShowConnections, DockAction::NewConnection]
    {
        assert_eq!(DockAction::from_id(&action.id()), Some(action.clone()), "{action:?}");
    }
    for bad in ["", "drift.dock.focus.", "drift.dock.focus.connections", "drift.dock.nope", "session-1"] {
        assert_eq!(DockAction::from_id(bad), None, "{bad}");
    }
}
