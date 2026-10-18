//! The native menu bar (tasks **M6-2**, **M2-4**, **M8-3**, **UI-windows**).
//!
//! [`menu_spec`] describes the menus as plain data (tested); `crate::windows` turns it into a
//! Tauri menu, rebuilds it whenever the session windows, their status or the key window change,
//! and routes clicks by [`MenuAction::id`]. Every keyboard shortcut is one of `drift-input`'s
//! allow-listed [`MenuShortcut`]s, so the RemoteView lets it through to the menu instead of
//! sending it to the remote (M2-4). Session actions have no shortcut: every other Command combo
//! belongs to the remote desktop.
//!
//! The Window menu's "Sessions" section is Drift's own list (ADR UI-windows-gallery
//! decision 9): the submenu is not registered as `NSApp.windowsMenu`, so AppKit adds no second,
//! unordered window list.

use drift_input::MenuShortcut;

use crate::connections::ConnectionStatus;

/// A session window as the Window and Dock menus list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionItem {
    /// Window label (`session-<n>`).
    pub window: String,
    /// Cleaned profile name.
    pub name: String,
    /// Text stand-in for the status (`● ◐ ◌ ↻ ⚠`).
    pub glyph: char,
    /// Where the connection stands.
    pub status: ConnectionStatus,
}

impl SessionItem {
    /// The menu title: glyph and name.
    pub fn title(&self) -> String {
        format!("{} {}", self.glyph, self.name)
    }
}

/// A Drift menu command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuAction {
    /// File ▸ New Connection… (Cmd+N): Connections with the New Connection sheet.
    NewConnection,
    /// File ▸ Show Connections, Window ▸ Connections (Cmd+0).
    ShowConnections,
    /// File ▸ Edit <name>… (Cmd+E): Connections with the key session's edit sheet.
    EditConnection,
    /// File ▸ Disconnect <name> (Shift+Cmd+D): closes the key session window without asking.
    Disconnect,
    /// File ▸ Close Window (Cmd+W): like the red button.
    CloseWindow,
    /// Window ▸ Sessions ▸ the n-th session window (1-based; Cmd+1 … Cmd+9 on the first nine).
    SelectSession(u32),
    /// Session ▸ Send Ctrl+Alt+Del.
    SendCtrlAltDel,
    /// Session ▸ Reconnect.
    Reconnect,
    /// Session ▸ Show Statistics: the fps HUD over the live picture (M1, M9-1).
    ToggleStats,
    /// Drift ▸ Quit Drift (Cmd+Q): asks once if sessions are open, then shuts every session down
    /// gracefully (2 s cap) and exits.
    Quit,
    /// Debug ▸ Record Session (experimental) — only built with the `recording` feature.
    ToggleRecording,
}

const SELECT_SESSION_PREFIX: &str = "drift.select-session.";

impl MenuAction {
    /// The menu item id.
    pub fn id(self) -> String {
        match self {
            Self::NewConnection => "drift.new-connection".to_owned(),
            Self::ShowConnections => "drift.show-connections".to_owned(),
            Self::EditConnection => "drift.edit-connection".to_owned(),
            Self::Disconnect => "drift.disconnect".to_owned(),
            Self::CloseWindow => "drift.close-window".to_owned(),
            Self::SelectSession(n) => format!("{SELECT_SESSION_PREFIX}{n}"),
            Self::SendCtrlAltDel => "drift.send-ctrl-alt-del".to_owned(),
            Self::Reconnect => "drift.reconnect".to_owned(),
            Self::ToggleStats => "drift.toggle-stats".to_owned(),
            Self::Quit => "drift.quit".to_owned(),
            Self::ToggleRecording => "drift.toggle-recording".to_owned(),
        }
    }

    /// Parses a menu item id.
    pub fn from_id(id: &str) -> Option<Self> {
        if let Some(n) = id.strip_prefix(SELECT_SESSION_PREFIX) {
            return match n.parse::<u32>() {
                Ok(n) if n >= 1 => Some(Self::SelectSession(n)),
                _ => None,
            };
        }
        match id {
            "drift.new-connection" => Some(Self::NewConnection),
            "drift.show-connections" => Some(Self::ShowConnections),
            "drift.edit-connection" => Some(Self::EditConnection),
            "drift.disconnect" => Some(Self::Disconnect),
            "drift.close-window" => Some(Self::CloseWindow),
            "drift.send-ctrl-alt-del" => Some(Self::SendCtrlAltDel),
            "drift.reconnect" => Some(Self::Reconnect),
            "drift.toggle-stats" => Some(Self::ToggleStats),
            "drift.quit" => Some(Self::Quit),
            "drift.toggle-recording" => Some(Self::ToggleRecording),
            _ => None,
        }
    }
}

/// Standard AppKit items (Tauri predefined menu items).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standard {
    /// About Drift.
    About,
    /// Services submenu.
    Services,
    /// Hide Drift.
    Hide,
    /// Hide Others.
    HideOthers,
    /// Show All.
    ShowAll,
    /// Undo (webview form).
    Undo,
    /// Redo.
    Redo,
    /// Cut.
    Cut,
    /// Copy.
    Copy,
    /// Paste.
    Paste,
    /// Select All.
    SelectAll,
    /// Minimize.
    Minimize,
    /// Zoom.
    Zoom,
    /// Enter/Exit Full Screen.
    Fullscreen,
}

/// One entry of a submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// A Drift command.
    Action {
        /// What it does.
        action: MenuAction,
        /// Title.
        title: String,
        /// Keyboard shortcut (always allow-listed).
        shortcut: Option<MenuShortcut>,
        /// Can be chosen.
        enabled: bool,
        /// `Some` for a check item (the Sessions list), with its state.
        checked: Option<bool>,
    },
    /// A disabled section title.
    Header(String),
    /// A standard item.
    Standard(Standard),
    /// A separator.
    Separator,
}

impl MenuEntry {
    fn action(action: MenuAction, title: &str, shortcut: Option<MenuShortcut>) -> Self {
        Self::Action { action, title: title.to_owned(), shortcut, enabled: true, checked: None }
    }
}

/// One top-level menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmenuSpec {
    /// Title in the menu bar.
    pub title: String,
    /// Items.
    pub entries: Vec<MenuEntry>,
}

fn submenu(title: &str, entries: Vec<MenuEntry>) -> SubmenuSpec {
    SubmenuSpec { title: title.to_owned(), entries }
}

/// The File menu's connection-specific titles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTitles {
    /// "Edit <name>…" or "Edit Connection…".
    pub edit: String,
    /// "Disconnect <name>" or "Disconnect".
    pub disconnect: String,
    /// A session window is key, so both apply to it.
    pub enabled: bool,
}

/// The File menu's Edit / Disconnect titles for the key session window (`None`: Connections or
/// nothing is key, and both items are disabled).
pub fn file_titles(key: Option<&SessionItem>) -> FileTitles {
    match key {
        Some(item) => FileTitles {
            edit: format!("Edit {}…", item.name),
            disconnect: format!("Disconnect {}", item.name),
            enabled: true,
        },
        None => FileTitles {
            edit: "Edit Connection…".to_owned(),
            disconnect: "Disconnect".to_owned(),
            enabled: false,
        },
    }
}

/// The Window menu's "Sessions" section: a header and one check item per session window in
/// opening order, Cmd+1…9 on the first nine, the key window checked. Empty without sessions.
pub fn window_sessions(sessions: &[SessionItem], key: Option<&str>) -> Vec<MenuEntry> {
    if sessions.is_empty() {
        return Vec::new();
    }
    let mut entries = vec![MenuEntry::Header("Sessions".to_owned())];
    for (n, item) in (1u32..).zip(sessions) {
        entries.push(MenuEntry::Action {
            action: MenuAction::SelectSession(n),
            title: item.title(),
            shortcut: u8::try_from(n).ok().filter(|n| *n <= 9).map(MenuShortcut::SelectSession),
            enabled: true,
            checked: Some(key == Some(item.window.as_str())),
        });
    }
    entries
}

/// The menu bar: Drift, File, Edit, Session, Window (+ Debug with feature `recording`), for the
/// session windows `sessions` (opening order) and the key window `key`.
pub fn menu_spec(sessions: &[SessionItem], key: Option<&str>) -> Vec<SubmenuSpec> {
    use MenuAction as A;
    use MenuEntry as E;
    use Standard as S;

    let drift = submenu(
        "Drift",
        vec![
            E::Standard(S::About),
            E::Separator,
            E::Standard(S::Services),
            E::Separator,
            E::Standard(S::Hide),
            E::Standard(S::HideOthers),
            E::Standard(S::ShowAll),
            E::Separator,
            E::action(A::Quit, "Quit Drift", Some(MenuShortcut::Quit)),
        ],
    );
    let titles = file_titles(key.and_then(|key| sessions.iter().find(|s| s.window == key)));
    let for_key = |action, title: String, shortcut| E::Action {
        action,
        title,
        shortcut: Some(shortcut),
        enabled: titles.enabled,
        checked: None,
    };
    let file = submenu(
        "File",
        vec![
            E::action(A::NewConnection, "New Connection…", Some(MenuShortcut::NewConnection)),
            E::action(A::ShowConnections, "Show Connections", Some(MenuShortcut::ShowConnections)),
            E::Separator,
            for_key(A::EditConnection, titles.edit.clone(), MenuShortcut::EditConnection),
            for_key(A::Disconnect, titles.disconnect.clone(), MenuShortcut::Disconnect),
            E::Separator,
            E::action(A::CloseWindow, "Close Window", Some(MenuShortcut::CloseWindow)),
        ],
    );
    let edit = submenu(
        "Edit",
        vec![
            E::Standard(S::Undo),
            E::Standard(S::Redo),
            E::Separator,
            E::Standard(S::Cut),
            E::Standard(S::Copy),
            E::Standard(S::Paste),
            E::Standard(S::SelectAll),
        ],
    );
    let session = submenu(
        "Session",
        vec![
            E::action(A::SendCtrlAltDel, "Send Ctrl+Alt+Del", None),
            E::action(A::Reconnect, "Reconnect", None),
            E::action(A::ToggleStats, "Show Statistics", None),
        ],
    );
    let mut window_entries = vec![
        E::Standard(S::Minimize),
        E::Standard(S::Zoom),
        E::Standard(S::Fullscreen),
        E::Separator,
        E::action(A::ShowConnections, "Connections", Some(MenuShortcut::ShowConnections)),
        E::Separator,
    ];
    window_entries.extend(window_sessions(sessions, key));
    let window = submenu("Window", window_entries);

    let mut menus = vec![drift, file, edit, session, window];
    if cfg!(feature = "recording") {
        menus.push(submenu(
            "Debug",
            vec![E::action(A::ToggleRecording, "Record Session (experimental)", None)],
        ));
    }
    menus
}

/// Tauri accelerator string for an allow-listed shortcut.
pub fn accelerator(shortcut: MenuShortcut) -> String {
    match shortcut {
        MenuShortcut::NewConnection => "CmdOrCtrl+N".to_owned(),
        MenuShortcut::ShowConnections => "CmdOrCtrl+0".to_owned(),
        MenuShortcut::EditConnection => "CmdOrCtrl+E".to_owned(),
        MenuShortcut::Disconnect => "CmdOrCtrl+Shift+D".to_owned(),
        MenuShortcut::CloseWindow => "CmdOrCtrl+W".to_owned(),
        MenuShortcut::Quit => "CmdOrCtrl+Q".to_owned(),
        MenuShortcut::SelectSession(n) => format!("CmdOrCtrl+{n}"),
        MenuShortcut::CycleWindows => "CmdOrCtrl+Backquote".to_owned(),
    }
}
