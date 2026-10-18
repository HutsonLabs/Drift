//! The frontmost Mac app's menu bar for a Drift client (task **EXT-3**; ADR
//! `EXT-3-menu-mirroring`).
//!
//! * [`snapshot_from_ax`] turns an Accessibility menu-bar tree ([`AxNode`]) into the wire's
//!   [`MenuSnapshot`] and remembers where each item sits, so a press can find its element again.
//! * [`MenuSource`] is what the responder reads from: [`ax::AxMenus`] on a real Mac (macOS only),
//!   [`StaticMenus`] in tests.
//! * [`MenuResponder`] answers the client's polls and presses on `Drift::Mac::v1`, but only while
//!   the connection is authorized (HOST-8): a stranger learns nothing about the Mac's apps and
//!   cannot choose a menu item.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use drift_ext::channel::Responder;
use drift_ext::menus::{KeyEquivalent, MenuItem, MenuMessage, MenuSnapshot, Modifiers};
use drift_ext::wire::Message;

use crate::gate::Authorized;

#[cfg(target_os = "macos")]
pub mod ax;

/// One element of an Accessibility menu tree, read into plain values.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AxNode {
    /// `AXRole`: `AXMenuBar`, `AXMenuBarItem`, `AXMenu` or `AXMenuItem`.
    pub role: String,
    /// `AXTitle`.
    pub title: String,
    /// `AXEnabled`.
    pub enabled: bool,
    /// Whether `AXMenuItemMarkChar` is set (a checkmark).
    pub marked: bool,
    /// `AXMenuItemCmdChar`.
    pub cmd_char: Option<String>,
    /// `AXMenuItemCmdModifiers`.
    pub cmd_modifiers: Option<i64>,
    /// `AXChildren`.
    pub children: Vec<AxNode>,
}

/// Where an item sits in the [`AxNode`] tree: child indices from the menu bar down.
pub type AxPath = Vec<usize>;

/// The title Accessibility gives the Apple menu, which belongs to the device, not the Mac app.
const APPLE_MENU: &str = "Apple";

/// Turns a menu bar (`AXMenuBar`) into a snapshot at `revision`, with the path of every item.
///
/// Ids are handed out in depth-first order from 1, so the same menu bar always gets the same ids.
/// An untitled, disabled item without a submenu is a separator.
pub fn snapshot_from_ax(app: &str, bar: &AxNode, revision: u32) -> (MenuSnapshot, HashMap<u32, AxPath>) {
    let mut paths = HashMap::new();
    let mut next = 1u32;
    let mut menus = Vec::new();
    for (i, bar_item) in bar.children.iter().enumerate() {
        if bar_item.title.is_empty() || (i == 0 && bar_item.title == APPLE_MENU) {
            continue;
        }
        let mut path = vec![i];
        menus.push(convert(bar_item, &mut path, &mut next, &mut paths));
    }
    (MenuSnapshot { revision, app: app.to_owned(), menus }, paths)
}

fn convert(node: &AxNode, path: &mut AxPath, next: &mut u32, paths: &mut HashMap<u32, AxPath>) -> MenuItem {
    let id = *next;
    *next += 1;
    paths.insert(id, path.clone());
    // A menu item's submenu is its one `AXMenu` child; the menu's children are the items.
    let mut children = Vec::new();
    if let Some((menu_index, menu)) = node.children.iter().enumerate().find(|(_, c)| c.role == "AXMenu") {
        for (i, child) in menu.children.iter().enumerate() {
            path.extend([menu_index, i]);
            children.push(convert(child, path, next, paths));
            path.truncate(path.len() - 2);
        }
    }
    let separator = node.title.is_empty() && !node.enabled && children.is_empty();
    let key = node.cmd_char.as_ref().filter(|c| !c.is_empty() && !separator).map(|c| KeyEquivalent {
        key: c.to_lowercase(),
        modifiers: Modifiers::from_ax(node.cmd_modifiers.unwrap_or(0)),
    });
    MenuItem {
        id,
        title: node.title.clone(),
        enabled: node.enabled,
        checked: node.marked,
        separator,
        key,
        children,
    }
}

/// Where the responder gets the menu and sends presses.
pub trait MenuSource: Send + Sync {
    /// The current menu, if the frontmost app has one.
    fn snapshot(&self) -> Option<Arc<MenuSnapshot>>;

    /// Chooses item `id` of snapshot `revision`. Returns whether it was pressed (a stale revision
    /// or an unknown id is ignored).
    fn press(&self, revision: u32, id: u32) -> bool;
}

/// A fixed menu that records presses (tests, and the loopback e2e).
#[derive(Debug, Default)]
pub struct StaticMenus {
    current: Mutex<Option<Arc<MenuSnapshot>>>,
    presses: Mutex<Vec<(u32, u32)>>,
}

impl StaticMenus {
    /// Serves `snapshot`.
    pub fn new(snapshot: Option<MenuSnapshot>) -> Self {
        Self { current: Mutex::new(snapshot.map(Arc::new)), presses: Mutex::default() }
    }

    /// Replaces the menu (the frontmost app changed).
    pub fn set(&self, snapshot: Option<MenuSnapshot>) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = snapshot.map(Arc::new);
    }

    /// Every press so far, as `(revision, id)`.
    pub fn presses(&self) -> Vec<(u32, u32)> {
        self.presses.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl MenuSource for StaticMenus {
    fn snapshot(&self) -> Option<Arc<MenuSnapshot>> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn press(&self, revision: u32, id: u32) -> bool {
        self.presses.lock().unwrap_or_else(PoisonError::into_inner).push((revision, id));
        true
    }
}

/// Answers menu polls and presses on `Drift::Mac::v1`, while the connection is authorized.
pub struct MenuResponder {
    source: Arc<dyn MenuSource>,
    authorized: Authorized,
}

impl std::fmt::Debug for MenuResponder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuResponder").finish_non_exhaustive()
    }
}

impl MenuResponder {
    /// Reads from `source`; `authorized` says whether this connection may.
    pub fn new(source: Arc<dyn MenuSource>, authorized: Authorized) -> Self {
        Self { source, authorized }
    }
}

impl Responder for MenuResponder {
    fn on_message(&mut self, message: &Message) -> Option<Vec<Message>> {
        let decoded = MenuMessage::from_message(message)?;
        if !(self.authorized)() {
            return Some(Vec::new());
        }
        let reply = match decoded {
            Ok(MenuMessage::Request { known_revision }) => match self.source.snapshot() {
                Some(s) if s.revision == known_revision => MenuMessage::Unchanged { revision: s.revision },
                Some(s) => MenuMessage::Snapshot((*s).clone()),
                None => MenuMessage::Empty,
            },
            Ok(MenuMessage::Press { revision, id }) => {
                if !self.source.press(revision, id) {
                    tracing::debug!(revision, id, "ignored a press for a menu that has changed");
                }
                return Some(Vec::new());
            }
            Ok(_) => return Some(Vec::new()),
            Err(e) => {
                tracing::warn!(error = %e, "ignoring a malformed menu message");
                return Some(Vec::new());
            }
        };
        Some(vec![reply.to_message()])
    }
}
