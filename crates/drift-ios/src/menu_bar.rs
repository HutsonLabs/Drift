//! Session menus in the iPad's menu bar (tasks **EXT-3**, **UI-kinds**): Drift's GNOME menus
//! (`src-tauri/src/gnome_menu.rs`).
//!
//! iPadOS 26 shows an app's menus in a menu bar. tao owns the app delegate, so instead of
//! `buildMenuWithBuilder:` the menus come from `UIMainMenuSystem`'s build handler
//! ([`MenuBar::install`]). While a snapshot is shown, the handler replaces Drift's own File, Edit,
//! Format, View, Window and Help menus with the session's, after Drift's app menu; without one,
//! the standard menus come back.
//!
//! Items are `UIAction`s, not `UIKeyCommand`s: hardware keys already go to the remote desktop (IOS-6), so a
//! key command would fire the shortcut twice. The shortcut shows as the item's subtitle.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use drift_core::menus::{MenuItem, MenuSnapshot};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, Message as _};
use objc2_foundation::{NSArray, NSString};
use objc2_ui_kit::{
    UIAction, UIMainMenuSystem, UIMainMenuSystemConfiguration, UIMenu, UIMenuApplication, UIMenuBuilder,
    UIMenuEdit, UIMenuElement, UIMenuElementAttributes, UIMenuElementState, UIMenuFile, UIMenuFormat,
    UIMenuHelp, UIMenuOptions, UIMenuView, UIMenuWindow,
};

/// Called with the id of the item the user chose.
pub type OnPress = Rc<dyn Fn(u32)>;

/// Prefix of the identifiers of the remote menus.
const IDENTIFIER: &str = "com.hutsonlabs.drift.remote-menu";

/// One `UIMenu` per remote menu, left to right.
pub fn build_menus(
    mtm: MainThreadMarker,
    snapshot: &MenuSnapshot,
    on_press: OnPress,
) -> Vec<Retained<UIMenu>> {
    snapshot
        .menus
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.title.is_empty())
        .map(|(i, m)| {
            let identifier = NSString::from_str(&format!("{IDENTIFIER}.{i}"));
            UIMenu::menuWithTitle_image_identifier_options_children(
                &NSString::from_str(&m.title),
                None,
                Some(&identifier),
                UIMenuOptions::empty(),
                &NSArray::from_retained_slice(&elements(mtm, &m.children, &on_press)),
                mtm,
            )
        })
        .collect()
}

/// A menu's items: those before the first separator directly, each later run in an inline group
/// (UIKit draws a line between groups; it has no separator item).
fn elements(mtm: MainThreadMarker, items: &[MenuItem], on_press: &OnPress) -> Vec<Retained<UIMenuElement>> {
    let mut runs: Vec<Vec<Retained<UIMenuElement>>> = vec![Vec::new()];
    for item in items {
        if item.separator {
            if runs.last().is_some_and(|r| !r.is_empty()) {
                runs.push(Vec::new());
            }
            continue;
        }
        if item.title.is_empty() {
            continue;
        }
        if let Some(run) = runs.last_mut() {
            run.push(element(mtm, item, on_press));
        }
    }
    let mut out = Vec::new();
    for (i, run) in runs.into_iter().enumerate() {
        if run.is_empty() {
            continue;
        }
        if i == 0 {
            out.extend(run);
        } else {
            let group = UIMenu::menuWithTitle_image_identifier_options_children(
                &NSString::from_str(""),
                None,
                None,
                UIMenuOptions::DisplayInline,
                &NSArray::from_retained_slice(&run),
                mtm,
            );
            out.push(Retained::into_super(group));
        }
    }
    out
}

fn element(mtm: MainThreadMarker, item: &MenuItem, on_press: &OnPress) -> Retained<UIMenuElement> {
    let title = NSString::from_str(&item.title);
    if !item.children.is_empty() {
        let menu = UIMenu::menuWithTitle_children(
            &title,
            &NSArray::from_retained_slice(&elements(mtm, &item.children, on_press)),
            mtm,
        );
        return Retained::into_super(menu);
    }
    let press = Rc::clone(on_press);
    let id = item.id;
    let handler = RcBlock::new(move |_action: NonNull<UIAction>| press(id));
    // SAFETY: the handler block matches `UIActionHandler`; UIKit copies it.
    let action = unsafe {
        UIAction::actionWithTitle_image_identifier_handler(&title, None, None, RcBlock::as_ptr(&handler), mtm)
    };
    if let Some(key) = &item.key {
        action.setSubtitle(Some(&NSString::from_str(&key.label())));
    }
    if !item.enabled {
        action.setAttributes(UIMenuElementAttributes::Disabled);
    }
    action.setState(if item.checked { UIMenuElementState::On } else { UIMenuElementState::Off });
    Retained::into_super(action)
}

/// The iPad menu bar's remote menus.
pub struct MenuBar {
    mtm: MainThreadMarker,
    on_press: OnPress,
    current: Rc<RefCell<Option<MenuSnapshot>>>,
    installed: RefCell<bool>,
}

impl std::fmt::Debug for MenuBar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuBar").field("installed", &self.installed).finish_non_exhaustive()
    }
}

impl MenuBar {
    /// A menu bar that reports choices to `on_press`.
    pub fn new(mtm: MainThreadMarker, on_press: OnPress) -> Self {
        Self { mtm, on_press, current: Rc::default(), installed: RefCell::new(false) }
    }

    /// Takes over the main menu system's build handler (once).
    pub fn install(&self) {
        if self.installed.replace(true) {
            return;
        }
        let current = Rc::clone(&self.current);
        let on_press = Rc::clone(&self.on_press);
        let mtm = self.mtm;
        let handler = RcBlock::new(move |builder: NonNull<ProtocolObject<dyn UIMenuBuilder>>| {
            // SAFETY: UIKit passes a live builder for the duration of the call.
            let builder = unsafe { builder.as_ref() };
            if let Some(snapshot) = current.borrow().as_ref() {
                rebuild(mtm, builder, snapshot, &on_press);
            }
        });
        let configuration = UIMainMenuSystemConfiguration::new(self.mtm);
        UIMainMenuSystem::sharedSystem(self.mtm)
            .setBuildConfiguration_buildHandler(&configuration, Some(&handler));
    }

    /// Shows `snapshot` (or Drift's own menus for `None`).
    pub fn set(&self, snapshot: Option<MenuSnapshot>) {
        *self.current.borrow_mut() = snapshot;
        if *self.installed.borrow() {
            UIMainMenuSystem::sharedSystem(self.mtm).setNeedsRebuild();
        }
    }

    /// The titles of the remote menus shown.
    pub fn menu_titles(&self) -> Vec<String> {
        self.current
            .borrow()
            .as_ref()
            .map(|s| {
                build_menus(self.mtm, s, Rc::clone(&self.on_press))
                    .iter()
                    .map(|m| m.title().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Reports item `id` as chosen, as the menu bar does (UIKit offers no way to fire a
    /// `UIAction` from code).
    #[doc(hidden)]
    pub fn choose_for_tests(&self, id: u32) {
        let pressable =
            self.current.borrow().as_ref().and_then(|s| s.find(id)).is_some_and(MenuItem::is_pressable);
        if pressable {
            (self.on_press)(id);
        }
    }
}

/// Replaces Drift's standard menus with the remote ones, after Drift's app menu.
fn rebuild(
    mtm: MainThreadMarker,
    builder: &ProtocolObject<dyn UIMenuBuilder>,
    snapshot: &MenuSnapshot,
    on_press: &OnPress,
) {
    // SAFETY: immutable constants exported by UIKit.
    let standard = unsafe { [UIMenuFile, UIMenuEdit, UIMenuFormat, UIMenuView, UIMenuWindow, UIMenuHelp] };
    for identifier in standard {
        builder.removeMenuForIdentifier(identifier);
    }
    // SAFETY: as above.
    let mut after: Retained<NSString> = unsafe { UIMenuApplication }.retain();
    tracing::debug!(app = %snapshot.app, menus = snapshot.menus.len(), "building the remote menus");
    for menu in build_menus(mtm, snapshot, Rc::clone(on_press)) {
        builder.insertSiblingMenu_afterMenuForIdentifier(&menu, &after);
        after = menu.identifier();
    }
}
