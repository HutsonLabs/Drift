//! The switcher button over a session window (task **IOS-11**).
//!
//! A `UIButton` in the top-leading corner whose menu (shown on tap) lists the app's windows:
//! Connections and every session, built by `drift_app::switcher`. On iPhone this is the only way
//! between session windows, which stack in the app's one scene; on iPad it complements the
//! system's scene switching. The menu is rebuilt whenever the set of sessions changes.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSString};
use objc2_ui_kit::{
    NSObjectUIAccessibility as _, UIAction, UIButton, UIButtonType, UIColor, UIImage, UIMenu, UIMenuElement,
    UIMenuElementState, UIView, UIViewAutoresizing,
};

/// One menu entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    /// What a choice reports back (a window label).
    pub id: String,
    /// The entry's title.
    pub title: String,
    /// A second line (the connection's status).
    pub subtitle: Option<String>,
    /// Shown with a checkmark (the current window).
    pub checked: bool,
}

/// Side of the button, in points, and its inset from the window's top-leading corner (below
/// the status bar).
const SIDE: f64 = 44.0;
const INSET_X: f64 = 12.0;
const INSET_Y: f64 = 12.0;

/// The switcher button and its menu.
pub struct SwitcherButton {
    button: Retained<UIButton>,
    on_pick: Rc<dyn Fn(String)>,
    ids: RefCell<Vec<String>>,
    mtm: MainThreadMarker,
}

impl std::fmt::Debug for SwitcherButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwitcherButton").field("ids", &self.ids.borrow()).finish_non_exhaustive()
    }
}

impl SwitcherButton {
    /// Adds the button on top of `parent`'s subviews; `on_pick` gets the chosen entry's id.
    pub fn install(mtm: MainThreadMarker, parent: &UIView, on_pick: impl Fn(String) + 'static) -> Self {
        let button = UIButton::buttonWithType(UIButtonType::System, mtm);
        if let Some(image) = UIImage::systemImageNamed(&NSString::from_str("rectangle.stack")) {
            button.setImage_forState(Some(&image), objc2_ui_kit::UIControlState::Normal);
        }
        button.setBackgroundColor(Some(&UIColor::colorWithWhite_alpha(0.0, 0.35)));
        button.layer().setCornerRadius(SIDE / 2.0);
        // SAFETY: a plain colour setter on a live view.
        unsafe { button.setTintColor(Some(&UIColor::whiteColor())) };
        button.setShowsMenuAsPrimaryAction(true);
        button.setAutoresizingMask(
            UIViewAutoresizing::FlexibleRightMargin | UIViewAutoresizing::FlexibleBottomMargin,
        );
        button.setAccessibilityLabel(Some(&NSString::from_str("Switch window")), mtm);
        parent.addSubview(&button);
        let this = Self { button, on_pick: Rc::new(on_pick), ids: RefCell::new(Vec::new()), mtm };
        this.place(parent.safeAreaInsets().top, parent.safeAreaInsets().left);
        this
    }

    /// Puts the button `top`/`left` points inside the window's safe area.
    pub fn place(&self, top: f64, left: f64) {
        self.button.setFrame(CGRect {
            origin: CGPoint { x: left + INSET_X, y: top + INSET_Y },
            size: CGSize { width: SIDE, height: SIDE },
        });
    }

    /// Replaces the menu's entries.
    pub fn set_entries(&self, entries: &[MenuEntry]) {
        *self.ids.borrow_mut() = entries.iter().map(|e| e.id.clone()).collect();
        let actions: Vec<Retained<UIMenuElement>> = entries
            .iter()
            .map(|entry| {
                let pick = Rc::clone(&self.on_pick);
                let id = entry.id.clone();
                let handler = RcBlock::new(move |_action: NonNull<UIAction>| pick(id.clone()));
                // SAFETY: the handler block matches `UIActionHandler`; UIKit copies it.
                let action = unsafe {
                    UIAction::actionWithTitle_image_identifier_handler(
                        &NSString::from_str(&entry.title),
                        None,
                        None,
                        RcBlock::as_ptr(&handler),
                        self.mtm,
                    )
                };
                if let Some(subtitle) = &entry.subtitle {
                    action.setSubtitle(Some(&NSString::from_str(subtitle)));
                }
                action.setState(if entry.checked { UIMenuElementState::On } else { UIMenuElementState::Off });
                Retained::into_super(action)
            })
            .collect();
        let menu = UIMenu::menuWithChildren(&NSArray::from_retained_slice(&actions), self.mtm);
        self.button.setMenu(Some(&menu));
    }

    /// Shows or hides the button.
    pub fn set_visible(&self, visible: bool) {
        self.button.setHidden(!visible);
    }

    /// Keeps the button above views added after it (the paste button, the picture).
    pub fn bring_to_front(&self) {
        if let Some(parent) = self.button.superview() {
            parent.bringSubviewToFront(&self.button);
        }
    }

    /// The menu's titles, in order.
    pub fn titles(&self) -> Vec<String> {
        self.elements().iter().map(|e| e.title().to_string()).collect()
    }

    /// Which entries are checked, in order.
    pub fn checked(&self) -> Vec<bool> {
        self.elements()
            .iter()
            .map(|e| e.downcast_ref::<UIAction>().is_some_and(|a| a.state() == UIMenuElementState::On))
            .collect()
    }

    /// Reports entry `index` as chosen, as tapping it does (UIKit offers no way to fire a
    /// `UIAction` from code).
    #[doc(hidden)]
    pub fn choose_for_tests(&self, index: usize) {
        let id = self.ids.borrow().get(index).cloned();
        if let Some(id) = id {
            (self.on_pick)(id);
        }
    }

    fn elements(&self) -> Vec<Retained<UIMenuElement>> {
        self.button.menu().map(|m| m.children().to_vec()).unwrap_or_default()
    }
}
