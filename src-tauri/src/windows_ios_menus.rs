//! The front session's remote menus in the iPad's menu bar (task **EXT-3**; ADR
//! `EXT-3-menu-mirroring`).
//!
//! Every session that mirrors its host's menu bar reports it here ([`apply_menu`]); the iPad
//! shows the one of the session window in front. Choosing an item goes back to that session as
//! [`SessionCommand::MenuPress`]. iPhones have no menu bar, so nothing is installed there.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use drift_ext::menus::MenuSnapshot;
use drift_ios::menu_bar::MenuBar;
use drift_rdp::SessionCommand;
use objc2::MainThreadMarker;
use tauri::{AppHandle, Manager as _, Runtime};

use crate::commands::AppState;

#[derive(Default)]
struct State {
    bar: Option<MenuBar>,
    menus: HashMap<String, Arc<MenuSnapshot>>,
    /// The window whose menu is shown, and its revision.
    shown: Option<(String, u32)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Session window `label`'s host menu changed (`None`: it has none). Main thread.
pub(super) fn apply_menu<R: Runtime>(
    app: &AppHandle<R>,
    mtm: MainThreadMarker,
    label: &str,
    menu: Option<Arc<MenuSnapshot>>,
) {
    STATE.with_borrow_mut(|s| {
        match menu {
            Some(m) => s.menus.insert(label.to_owned(), m),
            None => s.menus.remove(label),
        };
    });
    refresh(app, mtm);
}

/// Session window `label` is gone or lost its session.
pub(super) fn forget<R: Runtime>(app: &AppHandle<R>, label: &str) {
    STATE.with_borrow_mut(|s| s.menus.remove(label));
    if let Some(mtm) = MainThreadMarker::new() {
        refresh(app, mtm);
    }
}

/// Shows the menu of the front session window that has one.
pub(super) fn refresh<R: Runtime>(app: &AppHandle<R>, mtm: MainThreadMarker) {
    if drift_ios::view::is_phone(mtm) {
        return;
    }
    let front = STATE.with_borrow(|s| {
        s.menus
            .iter()
            .filter(|(label, _)| {
                super::with_platform(label, |plat| drift_ios::webview::is_front(&plat.web)) == Some(true)
            })
            .map(|(label, menu)| (label.clone(), Arc::clone(menu)))
            .min_by(|a, b| a.0.cmp(&b.0))
    });
    let wanted = front.as_ref().map(|(label, m)| (label.clone(), m.revision));
    let changed = STATE.with_borrow(|s| s.shown != wanted);
    if !changed {
        return;
    }
    let app = app.clone();
    STATE.with_borrow_mut(|s| {
        let bar = s.bar.get_or_insert_with(|| {
            let bar = MenuBar::new(
                mtm,
                Rc::new(move |id| {
                    let Some((label, _)) = STATE.with_borrow(|s| s.shown.clone()) else { return };
                    if let Err(e) =
                        app.state::<AppState>().sessions.send(&label, SessionCommand::MenuPress(id))
                    {
                        tracing::warn!(%label, error = %e, "could not choose a remote menu item");
                    }
                }),
            );
            bar.install();
            bar
        });
        bar.set(front.as_ref().map(|(_, m)| (**m).clone()));
        s.shown = wanted;
    });
}
