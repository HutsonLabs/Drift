//! The model of a menu bar that Drift shows for a session (the iPad menu bar's GNOME menus,
//! `src-tauri/src/gnome_menu.rs`).

use std::hash::{DefaultHasher, Hash, Hasher};

/// Modifier keys of a key equivalent, as a bit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

impl Modifiers {
    /// No modifier.
    pub const NONE: Self = Self(0);
    /// ⌘.
    pub const COMMAND: Self = Self(1);
    /// ⇧.
    pub const SHIFT: Self = Self(1 << 1);
    /// ⌥.
    pub const OPTION: Self = Self(1 << 2);
    /// ⌃.
    pub const CONTROL: Self = Self(1 << 3);

    const ALL: u8 = 0b1111;

    /// From raw bits; `None` if unknown bits are set.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::ALL == 0 { Some(Self(bits)) } else { None }
    }

    /// The raw bits.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether every modifier of `other` is present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// A menu item's keyboard shortcut.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyEquivalent {
    /// The key (`s`, `,`, …).
    pub key: String,
    /// Its modifiers.
    pub modifiers: Modifiers,
}

impl KeyEquivalent {
    /// The shortcut as a Mac menu shows it: `⌃⌥⇧⌘` in that order, then the key upper-cased.
    pub fn label(&self) -> String {
        let mut out = String::new();
        for (m, glyph) in [
            (Modifiers::CONTROL, '⌃'),
            (Modifiers::OPTION, '⌥'),
            (Modifiers::SHIFT, '⇧'),
            (Modifiers::COMMAND, '⌘'),
        ] {
            if self.modifiers.contains(m) {
                out.push(glyph);
            }
        }
        out.extend(self.key.chars().flat_map(char::to_uppercase));
        out
    }
}

/// One menu item; a top-level menu is an item with children.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MenuItem {
    /// Stable within one snapshot revision; a choice names it.
    pub id: u32,
    /// The title (empty for a separator).
    pub title: String,
    /// Whether it can be chosen.
    pub enabled: bool,
    /// Whether it shows a checkmark.
    pub checked: bool,
    /// A separator line.
    pub separator: bool,
    /// Its keyboard shortcut.
    pub key: Option<KeyEquivalent>,
    /// Its submenu's items.
    pub children: Vec<MenuItem>,
}

impl MenuItem {
    /// Whether choosing it does something: enabled, not a separator, not a submenu title.
    pub fn is_pressable(&self) -> bool {
        self.enabled && !self.separator && self.children.is_empty()
    }
}

/// A menu bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSnapshot {
    /// Bumped whenever the content changes.
    pub revision: u32,
    /// The name shown for the menus' owner.
    pub app: String,
    /// Its top-level menus, left to right.
    pub menus: Vec<MenuItem>,
}

impl MenuSnapshot {
    /// A hash of everything but the revision: equal menus hash equal.
    pub fn content_hash(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.app.hash(&mut h);
        self.menus.hash(&mut h);
        h.finish()
    }

    /// The item with `id`, at any level.
    pub fn find(&self, id: u32) -> Option<&MenuItem> {
        fn walk(items: &[MenuItem], id: u32) -> Option<&MenuItem> {
            items.iter().find_map(|i| if i.id == id { Some(i) } else { walk(&i.children, id) })
        }
        walk(&self.menus, id)
    }

    /// Items at every level.
    pub fn item_count(&self) -> usize {
        fn count(items: &[MenuItem]) -> usize {
            items.iter().map(|i| 1 + count(&i.children)).sum()
        }
        count(&self.menus)
    }
}
