//! The Dock menu model (task **UI-windows**, ADR UI-windows-gallery decision 10).
//!
//! Pure data: `drift_macos::dock` turns [`dock_menu`] into the `NSMenu` AppKit asks the
//! application delegate for, and routes clicks back by [`DockAction::id`].

use crate::menu::SessionItem;

/// What a Dock menu item does.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DockAction {
    /// Bring session window `label` forward.
    Focus(String),
    /// Show the Connections window.
    ShowConnections,
    /// Show Connections with the New Connection sheet.
    NewConnection,
}

const FOCUS_PREFIX: &str = "drift.dock.focus.";

impl DockAction {
    /// The item id handed to AppKit.
    pub fn id(&self) -> String {
        match self {
            Self::Focus(label) => format!("{FOCUS_PREFIX}{label}"),
            Self::ShowConnections => "drift.dock.connections".to_owned(),
            Self::NewConnection => "drift.dock.new-connection".to_owned(),
        }
    }

    /// Parses an item id; only session window labels can be focused.
    pub fn from_id(id: &str) -> Option<Self> {
        if let Some(label) = id.strip_prefix(FOCUS_PREFIX) {
            return label.starts_with("session-").then(|| Self::Focus(label.to_owned()));
        }
        match id {
            "drift.dock.connections" => Some(Self::ShowConnections),
            "drift.dock.new-connection" => Some(Self::NewConnection),
            _ => None,
        }
    }
}

/// One entry of the Dock menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockItem {
    /// A disabled section title.
    Header(String),
    /// A command; `key` is the key equivalent shown with Command.
    Action {
        /// What it does.
        action: DockAction,
        /// Title.
        title: String,
        /// Key equivalent shown with Command.
        key: Option<char>,
    },
    /// A separator.
    Separator,
}

/// The Dock menu: a "Sessions" section (glyph + name, opening order) when sessions are open,
/// then Connections (⌘0) and New Connection… (⌘N).
pub fn dock_menu(sessions: &[SessionItem]) -> Vec<DockItem> {
    let mut items = Vec::new();
    if !sessions.is_empty() {
        items.push(DockItem::Header("Sessions".to_owned()));
        items.extend(sessions.iter().map(|s| DockItem::Action {
            action: DockAction::Focus(s.window.clone()),
            title: s.title(),
            key: None,
        }));
        items.push(DockItem::Separator);
    }
    items.push(DockItem::Action {
        action: DockAction::ShowConnections,
        title: "Connections".to_owned(),
        key: Some('0'),
    });
    items.push(DockItem::Action {
        action: DockAction::NewConnection,
        title: "New Connection…".to_owned(),
        key: Some('n'),
    });
    items
}
