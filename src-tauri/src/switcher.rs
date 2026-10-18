//! The in-app session switcher (task **IOS-11**, pure).
//!
//! On iPad each session window is a scene of its own and iPadOS switches between them (ADR
//! IOS-5). On iPhone the windows stack in the app's one scene, so Drift offers a switcher inside
//! the app: a button over every session window (`drift_ios::switcher`) whose menu is built from
//! [`entries`], and a choice is resolved with [`target`].

use crate::connections::{CONNECTIONS_WINDOW, ConnectionStatus};
use crate::present;

/// One switcher menu entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherEntry {
    /// The window label (Connections' is [`CONNECTIONS_WINDOW`]).
    pub id: String,
    /// What the menu shows.
    pub title: String,
    /// The connection's status in words; `None` for Connections and idle windows.
    pub subtitle: Option<String>,
    /// The window the menu is shown in.
    pub current: bool,
}

/// Where a choice leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The Connections window.
    Connections,
    /// A session window, by label.
    Session(String),
}

/// The status word shown under a session's name.
fn status_word(status: ConnectionStatus) -> Option<&'static str> {
    match status {
        ConnectionStatus::Idle => None,
        ConnectionStatus::Connecting => Some("Connecting"),
        ConnectionStatus::Live => Some("Live"),
        ConnectionStatus::Reconnecting => Some("Reconnecting"),
        ConnectionStatus::Failed => Some("Disconnected"),
    }
}

/// The menu shown in window `current`: Connections, then `sessions` (`label, profile name,
/// status`, in opening order).
pub fn entries(current: &str, sessions: &[(String, String, ConnectionStatus)]) -> Vec<SwitcherEntry> {
    let connections = SwitcherEntry {
        id: CONNECTIONS_WINDOW.to_owned(),
        title: "Connections".to_owned(),
        subtitle: None,
        current: current == CONNECTIONS_WINDOW,
    };
    std::iter::once(connections)
        .chain(sessions.iter().map(|(label, name, status)| SwitcherEntry {
            id: label.clone(),
            title: present::display_name(name),
            subtitle: status_word(*status).map(str::to_owned),
            current: label == current,
        }))
        .collect()
}

/// Resolves a chosen entry id against the session windows that still exist.
pub fn target(id: &str, sessions: &[String]) -> Option<Target> {
    if id == CONNECTIONS_WINDOW {
        return Some(Target::Connections);
    }
    sessions.iter().any(|s| s == id).then(|| Target::Session(id.to_owned()))
}
