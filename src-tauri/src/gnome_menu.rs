//! Drift's own menus for a GNOME session in the iPad's menu bar (task **UI-kinds**, mockup H).
//!
//! A Mac running Drift Host mirrors its front app's menus (EXT-3). A GNOME computer has nothing
//! to mirror, so the bar shows **Session**, **Keys** and **Activities**: GNOME's shortcuts sent
//! as key chords, and Disconnect. Pure: `windows_ios_menus` shows the [`snapshot`] and turns a
//! chosen item into an [`action`].

use drift_ext::menus::{MenuItem, MenuSnapshot};

/// What choosing an item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GnomeAction {
    /// Press these keys together and release them ([`crate::phone::named_chord_events`] names).
    Keys(&'static [&'static str]),
    /// Close the session and its window.
    Disconnect,
}

/// Item ids, far above anything a host sends (ids are local to this menu anyway).
const BASE: u32 = 0x4E00_0000;

/// The items: (menu, id, title, action). Ids are stable.
const ITEMS: [(&str, u32, &str, GnomeAction); 7] = [
    ("Session", BASE + 1, "Disconnect", GnomeAction::Disconnect),
    ("Keys", BASE + 10, "Super", GnomeAction::Keys(&["super"])),
    ("Keys", BASE + 11, "Switch Windows", GnomeAction::Keys(&["alt", "tab"])),
    ("Keys", BASE + 12, "Lock Screen", GnomeAction::Keys(&["super", "l"])),
    ("Keys", BASE + 13, "Ctrl+Alt+Delete", GnomeAction::Keys(&["control", "alt", "delete"])),
    ("Activities", BASE + 20, "Activities Overview", GnomeAction::Keys(&["super"])),
    ("Activities", BASE + 21, "Show Apps", GnomeAction::Keys(&["super", "a"])),
];

/// The top-level menus, left to right, with their ids.
const MENUS: [(&str, u32); 3] = [("Session", BASE + 100), ("Keys", BASE + 101), ("Activities", BASE + 102)];

fn item(id: u32, title: &str, children: Vec<MenuItem>) -> MenuItem {
    MenuItem {
        id,
        title: title.to_owned(),
        enabled: true,
        checked: false,
        separator: false,
        key: None,
        children,
    }
}

/// The menus a GNOME session shows (its revision never changes).
pub fn snapshot() -> MenuSnapshot {
    let menus = MENUS
        .iter()
        .map(|&(menu, id)| {
            let children = ITEMS
                .iter()
                .filter(|i| i.0 == menu)
                .map(|&(_, id, title, _)| item(id, title, Vec::new()))
                .collect();
            item(id, menu, children)
        })
        .collect();
    MenuSnapshot { revision: 1, app: "GNOME".to_owned(), menus }
}

/// What the item with `id` does, or `None` if no item has it.
pub fn action(id: u32) -> Option<GnomeAction> {
    ITEMS.iter().find(|i| i.1 == id).map(|i| i.3)
}
