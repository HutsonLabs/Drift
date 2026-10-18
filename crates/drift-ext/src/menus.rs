//! Menu mirroring on `Drift::Mac::v1` (task **EXT-3**; ADR `IOS-0` decision 3, ADR
//! `EXT-3-menu-mirroring`).
//!
//! The host reads the frontmost Mac app's menu bar through Accessibility; an iPad shows it in its
//! own menu bar and chooses items remotely. The exchange, after both Hellos agreed on
//! [`Capabilities::MENUS`]:
//!
//! ```text
//! client → Request { known_revision }       (every POLL_INTERVAL while the session is in front)
//! host   → Snapshot { … } | Unchanged { revision } | Empty
//! client → Press { revision, id }           (the user chose an item; no reply)
//! ```
//!
//! The client polls rather than the host pushing: a DVC processor can only send in reply to what
//! it receives, so polling needs no change to `ironrdp-server`'s event loop, and a menu that
//! appears within a second is soon enough.
//!
//! This module is pure: the model, its strict encoding (fuzzed with the rest of the channel),
//! and the client's side ([`MenuMirror`]). The host's side lives in `drift-host`.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::Negotiated;
use crate::wire::{Capabilities, MAX_PAYLOAD, Message, MessageType, WireError};

/// How often the client asks for the menu while its session is in front.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// The deepest nesting accepted: a top-level menu is depth 1.
pub const MAX_DEPTH: usize = 8;

/// The most items in one snapshot (all levels).
pub const MAX_ITEMS: usize = 4096;

/// The longest title (bytes of UTF-8); the app's name has the same limit.
pub const MAX_TITLE: usize = 256;

/// The longest key equivalent (bytes of UTF-8).
pub const MAX_KEY: usize = 16;

/// Message types of the menu range (`0x02xx`).
pub mod types {
    use crate::wire::MessageType;

    /// Client → host: send the menu if it is not `known_revision`.
    pub const REQUEST: MessageType = MessageType(0x0201);
    /// Host → client: the frontmost app's menu bar.
    pub const SNAPSHOT: MessageType = MessageType(0x0202);
    /// Host → client: the client's revision is current.
    pub const UNCHANGED: MessageType = MessageType(0x0203);
    /// Client → host: choose an item.
    pub const PRESS: MessageType = MessageType(0x0204);
    /// Host → client: there is no menu to show (no frontmost app, or it has none).
    pub const EMPTY: MessageType = MessageType(0x0205);
}

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

    /// From Accessibility's `AXMenuItemCmdModifiers` (`kAXMenuItemModifier*`): Shift 1,
    /// Option 2, Control 4, and 8 for "no Command" (Command is implied otherwise).
    pub const fn from_ax(bits: i64) -> Self {
        let mut m = if bits & 8 == 0 { Self::COMMAND.0 } else { 0 };
        if bits & 1 != 0 {
            m |= Self::SHIFT.0;
        }
        if bits & 2 != 0 {
            m |= Self::OPTION.0;
        }
        if bits & 4 != 0 {
            m |= Self::CONTROL.0;
        }
        Self(m)
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
    /// The key, as the Mac app names it (`s`, `,`, …).
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

/// One item of a mirrored menu; a top-level menu is an item with children.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MenuItem {
    /// Stable within one snapshot revision; [`MenuMessage::Press`] names it.
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

/// The frontmost app's menu bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSnapshot {
    /// Bumped by the host whenever the content changes.
    pub revision: u32,
    /// The app's name.
    pub app: String,
    /// Its top-level menus, left to right (the Apple menu excluded).
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

/// A decoded menu message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuMessage {
    /// [`types::REQUEST`].
    Request {
        /// The revision the client shows (0: none).
        known_revision: u32,
    },
    /// [`types::SNAPSHOT`].
    Snapshot(MenuSnapshot),
    /// [`types::UNCHANGED`].
    Unchanged {
        /// The current revision.
        revision: u32,
    },
    /// [`types::PRESS`].
    Press {
        /// The snapshot the client chose from.
        revision: u32,
        /// The item.
        id: u32,
    },
    /// [`types::EMPTY`].
    Empty,
}

const FLAG_ENABLED: u8 = 1;
const FLAG_CHECKED: u8 = 1 << 1;
const FLAG_SEPARATOR: u8 = 1 << 2;
const FLAG_KEY: u8 = 1 << 3;
const FLAGS: u8 = FLAG_ENABLED | FLAG_CHECKED | FLAG_SEPARATOR | FLAG_KEY;

impl MenuMessage {
    /// As a wire message. A snapshot over the limits becomes [`MenuMessage::Empty`]; use
    /// [`Self::try_to_message`] to see why.
    pub fn to_message(&self) -> Message {
        self.try_to_message().unwrap_or_else(|_| Message::Unknown { kind: types::EMPTY, payload: Vec::new() })
    }

    /// As a wire message, refusing a snapshot over [`MAX_DEPTH`], [`MAX_ITEMS`], [`MAX_TITLE`],
    /// [`MAX_KEY`] or the channel's payload limit.
    pub fn try_to_message(&self) -> Result<Message, WireError> {
        let (kind, payload) = match self {
            Self::Request { known_revision } => (types::REQUEST, known_revision.to_le_bytes().to_vec()),
            Self::Unchanged { revision } => (types::UNCHANGED, revision.to_le_bytes().to_vec()),
            Self::Press { revision, id } => {
                let mut p = revision.to_le_bytes().to_vec();
                p.extend(id.to_le_bytes());
                (types::PRESS, p)
            }
            Self::Empty => (types::EMPTY, Vec::new()),
            Self::Snapshot(s) => (types::SNAPSHOT, encode_snapshot(s)?),
        };
        Ok(Message::Unknown { kind, payload })
    }

    /// Decodes a message of the menu range; `None` for any other type.
    pub fn from_message(message: &Message) -> Option<Result<Self, WireError>> {
        let Message::Unknown { kind, payload } = message else { return None };
        if kind.0 >> 8 != 0x02 {
            return None;
        }
        Some(decode_menu(*kind, payload))
    }
}

fn put_str(out: &mut Vec<u8>, s: &str, max: usize) -> Result<(), WireError> {
    if s.len() > max {
        return Err(WireError::TooLarge(s.len()));
    }
    let len = u16::try_from(s.len()).map_err(|_| WireError::TooLarge(s.len()))?;
    out.extend(len.to_le_bytes());
    out.extend(s.as_bytes());
    Ok(())
}

fn encode_snapshot(s: &MenuSnapshot) -> Result<Vec<u8>, WireError> {
    if s.item_count() > MAX_ITEMS {
        return Err(WireError::TooLarge(s.item_count()));
    }
    let mut out = s.revision.to_le_bytes().to_vec();
    put_str(&mut out, &s.app, MAX_TITLE)?;
    encode_items(&mut out, &s.menus, 1)?;
    if out.len() > MAX_PAYLOAD {
        return Err(WireError::TooLarge(out.len()));
    }
    Ok(out)
}

fn encode_items(out: &mut Vec<u8>, items: &[MenuItem], depth: usize) -> Result<(), WireError> {
    if depth > MAX_DEPTH && !items.is_empty() {
        return Err(WireError::Malformed("menu nested too deeply"));
    }
    let count = u16::try_from(items.len()).map_err(|_| WireError::TooLarge(items.len()))?;
    out.extend(count.to_le_bytes());
    for i in items {
        out.extend(i.id.to_le_bytes());
        let mut flags = 0;
        if i.enabled {
            flags |= FLAG_ENABLED;
        }
        if i.checked {
            flags |= FLAG_CHECKED;
        }
        if i.separator {
            flags |= FLAG_SEPARATOR;
        }
        if i.key.is_some() {
            flags |= FLAG_KEY;
        }
        out.push(flags);
        put_str(out, &i.title, MAX_TITLE)?;
        if let Some(k) = &i.key {
            put_str(out, &k.key, MAX_KEY)?;
            out.push(k.modifiers.bits());
        }
        encode_items(out, &i.children, depth + 1)?;
    }
    Ok(())
}

/// A cursor that never panics.
struct Reader<'a> {
    rest: &'a [u8],
    items: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.rest.len() < n {
            return Err(WireError::Malformed("menu message truncated"));
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, WireError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn string(&mut self, max: usize) -> Result<String, WireError> {
        let len = usize::from(self.u16()?);
        if len > max {
            return Err(WireError::Malformed("menu string longer than allowed"));
        }
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| WireError::Malformed("menu string is not UTF-8"))
    }

    fn items(&mut self, depth: usize) -> Result<Vec<MenuItem>, WireError> {
        let count = usize::from(self.u16()?);
        if count > 0 && depth > MAX_DEPTH {
            return Err(WireError::Malformed("menu nested too deeply"));
        }
        if self.items + count > MAX_ITEMS {
            return Err(WireError::Malformed("too many menu items"));
        }
        self.items += count;
        let mut out = Vec::with_capacity(count.min(64));
        for _ in 0..count {
            let id = self.u32()?;
            let flags = self.u8()?;
            if flags & !FLAGS != 0 {
                return Err(WireError::Malformed("unknown menu item flags"));
            }
            let title = self.string(MAX_TITLE)?;
            let key = if flags & FLAG_KEY != 0 {
                let key = self.string(MAX_KEY)?;
                let modifiers =
                    Modifiers::from_bits(self.u8()?).ok_or(WireError::Malformed("unknown modifiers"))?;
                Some(KeyEquivalent { key, modifiers })
            } else {
                None
            };
            let children = self.items(depth + 1)?;
            out.push(MenuItem {
                id,
                title,
                enabled: flags & FLAG_ENABLED != 0,
                checked: flags & FLAG_CHECKED != 0,
                separator: flags & FLAG_SEPARATOR != 0,
                key,
                children,
            });
        }
        Ok(out)
    }

    fn finish(&self) -> Result<(), WireError> {
        if self.rest.is_empty() { Ok(()) } else { Err(WireError::Malformed("bytes after the menu message")) }
    }
}

fn decode_menu(kind: MessageType, payload: &[u8]) -> Result<MenuMessage, WireError> {
    let mut r = Reader { rest: payload, items: 0 };
    let message = match kind {
        types::REQUEST => MenuMessage::Request { known_revision: r.u32()? },
        types::UNCHANGED => MenuMessage::Unchanged { revision: r.u32()? },
        types::PRESS => MenuMessage::Press { revision: r.u32()?, id: r.u32()? },
        types::EMPTY => MenuMessage::Empty,
        types::SNAPSHOT => {
            let revision = r.u32()?;
            let app = r.string(MAX_TITLE)?;
            let menus = r.items(1)?;
            MenuMessage::Snapshot(MenuSnapshot { revision, app, menus })
        }
        _ => return Err(WireError::Malformed("unknown menu message")),
    };
    r.finish()?;
    Ok(message)
}

/// What a message meant to the [`MenuMirror`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirrorEvent {
    /// Not a menu message (or a malformed one, which is logged and ignored).
    NotOurs,
    /// A menu message that changes nothing.
    NoChange,
    /// The menu to show is now this (`None`: nothing).
    Changed(Option<Arc<MenuSnapshot>>),
}

/// The client's side: when to poll, what is shown, and what a choice sends.
#[derive(Debug, Default)]
pub struct MenuMirror {
    current: Option<Arc<MenuSnapshot>>,
    last_poll: Option<Instant>,
}

impl MenuMirror {
    /// A request to send now, if the host offers menus, the menu is `wanted` (the session is in
    /// front) and [`POLL_INTERVAL`] has passed since the last one.
    pub fn poll(&mut self, now: Instant, negotiated: Option<&Negotiated>, wanted: bool) -> Option<Message> {
        if !wanted || !negotiated.is_some_and(|n| n.capabilities.contains(Capabilities::MENUS)) {
            return None;
        }
        if self.last_poll.is_some_and(|t| now.saturating_duration_since(t) < POLL_INTERVAL) {
            return None;
        }
        self.last_poll = Some(now);
        let known_revision = self.current.as_ref().map_or(0, |s| s.revision);
        Some(MenuMessage::Request { known_revision }.to_message())
    }

    /// Applies a message from the host.
    pub fn on_message(&mut self, message: &Message) -> MirrorEvent {
        let Some(decoded) = MenuMessage::from_message(message) else { return MirrorEvent::NotOurs };
        match decoded {
            Ok(MenuMessage::Snapshot(s)) => {
                if self.current.as_deref() == Some(&s) {
                    return MirrorEvent::NoChange;
                }
                let s = Arc::new(s);
                self.current = Some(Arc::clone(&s));
                MirrorEvent::Changed(Some(s))
            }
            Ok(MenuMessage::Empty) => match self.current.take() {
                Some(_) => MirrorEvent::Changed(None),
                None => MirrorEvent::NoChange,
            },
            Ok(MenuMessage::Unchanged { .. } | MenuMessage::Request { .. } | MenuMessage::Press { .. }) => {
                MirrorEvent::NoChange
            }
            Err(e) => {
                tracing::warn!(error = %e, "ignoring a malformed menu message");
                MirrorEvent::NotOurs
            }
        }
    }

    /// The press for item `id`, if the shown snapshot has it and it can be chosen.
    pub fn press(&self, id: u32) -> Option<Message> {
        let snap = self.current.as_ref()?;
        snap.find(id)
            .filter(|i| i.is_pressable())
            .map(|_| MenuMessage::Press { revision: snap.revision, id }.to_message())
    }

    /// The snapshot shown.
    pub fn current(&self) -> Option<&Arc<MenuSnapshot>> {
        self.current.as_ref()
    }

    /// Forgets everything (a new connection leg).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
