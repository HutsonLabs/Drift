//! The menu-bar item and its menu (task **HOST-9**). Humble object: [`build_menu`] turns the
//! model's [`MenuEntry`]s into an `NSMenu` and reports the chosen [`MenuAction`]; nothing here
//! decides anything.

use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::NSString;

use crate::model::{MenuAction, MenuEntry};

type Handler = Rc<dyn Fn(MenuAction)>;

/// Instance variables of the menu target.
pub struct TargetIvars {
    handler: Handler,
    /// The action of each choosable item, indexed by the item's tag.
    actions: RefCell<Vec<MenuAction>>,
}

define_class!(
    // The target of every choosable item: maps the item's tag back to its action.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DriftHostMenuTarget"]
    #[ivars = TargetIvars]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(choose:))]
        fn choose(&self, sender: Option<&AnyObject>) {
            let Some(item) = sender.and_then(|s| s.downcast_ref::<NSMenuItem>()) else { return };
            let action = usize::try_from(item.tag()).ok().and_then(|i| self.ivars().actions.borrow().get(i).copied());
            if let Some(action) = action {
                // Cloned out first: the handler may rebuild the menu (and this target).
                let handler = Rc::clone(&self.ivars().handler);
                handler(action);
            }
        }
    }
);

/// An `NSMenu` built from the model, with the target its items send to (menu items hold their
/// target weakly, so it lives here).
pub struct BuiltMenu {
    menu: Retained<NSMenu>,
    target: Retained<MenuTarget>,
}

impl std::fmt::Debug for BuiltMenu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltMenu").field("items", &self.menu.numberOfItems()).finish()
    }
}

impl Deref for BuiltMenu {
    type Target = NSMenu;
    fn deref(&self) -> &NSMenu {
        &self.menu
    }
}

/// Builds the menu for `entries`; choosing an item calls `on_action` on the main thread.
pub fn build_menu(
    mtm: MainThreadMarker,
    entries: &[MenuEntry],
    on_action: impl Fn(MenuAction) + 'static,
) -> BuiltMenu {
    let target = MenuTarget::alloc(mtm)
        .set_ivars(TargetIvars { handler: Rc::new(on_action), actions: RefCell::new(Vec::new()) });
    // SAFETY: `init` on a fresh NSObject subclass.
    let target: Retained<MenuTarget> = unsafe { msg_send![super(target), init] };
    let menu = fill(mtm, entries, &target);
    BuiltMenu { menu, target }
}

fn fill(mtm: MainThreadMarker, entries: &[MenuEntry], target: &MenuTarget) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    // Enabled state comes from each item, not from responder-chain validation.
    menu.setAutoenablesItems(false);
    for entry in entries {
        let item = match entry {
            MenuEntry::Separator => NSMenuItem::separatorItem(mtm),
            MenuEntry::Text(title) => {
                let item = plain_item(mtm, title);
                item.setEnabled(false);
                item
            }
            MenuEntry::Item { title, action, checked } => {
                let item = plain_item(mtm, title);
                let mut actions = target.ivars().actions.borrow_mut();
                item.setTag(isize::try_from(actions.len()).unwrap_or(isize::MAX));
                actions.push(*action);
                // SAFETY: `choose:` takes the sender and is implemented by `MenuTarget`; the
                // target outlives the menu (both live in `BuiltMenu`).
                unsafe {
                    item.setTarget(Some(target));
                    item.setAction(Some(sel!(choose:)));
                }
                item.setState(if *checked { NSControlStateValueOn } else { NSControlStateValueOff });
                item.setEnabled(true);
                item
            }
            MenuEntry::Submenu { title, entries } => {
                let item = plain_item(mtm, title);
                item.setSubmenu(Some(&fill(mtm, entries, target)));
                item
            }
        };
        menu.addItem(&item);
    }
    menu
}

fn plain_item(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    // SAFETY: a title, no action, empty key equivalent.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            None,
            &NSString::new(),
        )
    }
}

/// The SF Symbol for the icon's state.
const ICON: &str = "display";
const ICON_ATTENTION: &str = "exclamationmark.triangle";

/// The item in the menu bar. Removed from the menu bar on drop.
pub struct StatusItem {
    item: Retained<NSStatusItem>,
    mtm: MainThreadMarker,
    menu: RefCell<Option<BuiltMenu>>,
}

impl std::fmt::Debug for StatusItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatusItem").finish_non_exhaustive()
    }
}

impl StatusItem {
    /// Adds an item to the system menu bar.
    pub fn new(mtm: MainThreadMarker) -> Self {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let this = Self { item, mtm, menu: RefCell::new(None) };
        this.set_attention(false);
        this
    }

    /// Replaces the item's menu (the previous menu and its target are released).
    pub fn set_menu(&self, menu: &BuiltMenu) {
        self.item.setMenu(Some(&menu.menu));
        *self.menu.borrow_mut() =
            Some(BuiltMenu { menu: Retained::clone(&menu.menu), target: Retained::clone(&menu.target) });
    }

    /// Shows the attention icon (a missing permission, a stopped host) or the normal one.
    pub fn set_attention(&self, attention: bool) {
        let Some(button) = self.item.button(self.mtm()) else { return };
        let (symbol, label) =
            if attention { (ICON_ATTENTION, "Drift Host needs attention") } else { (ICON, "Drift Host") };
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&NSString::from_str(label)),
        );
        if let Some(image) = &image {
            image.setTemplate(true);
        }
        button.setImage(image.as_deref());
    }

    /// Whether the item has its button (it always should while in the menu bar).
    pub fn has_button(&self) -> bool {
        self.item.button(self.mtm()).is_some()
    }

    fn mtm(&self) -> MainThreadMarker {
        self.mtm
    }
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}
