//! The front session's menus in the iPad's menu bar (task **EXT-3**; ADR
//! `EXT-3-menu-mirroring`; task **UI-kinds**).
//!
//! Every session that mirrors its host's menu bar reports it here ([`apply_menu`]); the iPad
//! shows the one of the session window in front. Choosing an item goes back to that session as
//! [`SessionCommand::MenuPress`]. A GNOME computer has no menus to mirror, so its window shows
//! Drift's own ([`crate::gnome_menu`]): their items send GNOME's shortcuts or disconnect.
//! iPhones have no menu bar, so nothing is installed there.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use drift_core::HostKind;
use drift_ext::menus::MenuSnapshot;
use drift_ios::menu_bar::MenuBar;
use drift_rdp::SessionCommand;
use objc2::MainThreadMarker;
use tauri::{AppHandle, Manager as _, Runtime};

use crate::commands::AppState;
use crate::gnome_menu::{self, GnomeAction};

/// What the menu bar shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shown {
    /// Window `label`'s mirrored host menu, at `revision`.
    Remote { label: String, revision: u32 },
    /// Drift's menus for GNOME window `label`.
    Gnome { label: String },
}

#[derive(Default)]
struct State {
    bar: Option<MenuBar>,
    menus: HashMap<String, Arc<MenuSnapshot>>,
    /// What is shown.
    shown: Option<Shown>,
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

/// Shows the menu of the front session window: its host's, or Drift's for GNOME.
pub(super) fn refresh<R: Runtime>(app: &AppHandle<R>, mtm: MainThreadMarker) {
    if drift_ios::view::is_phone(mtm) {
        return;
    }
    let front = super::session_labels()
        .into_iter()
        .filter(|label| {
            super::with_platform(label, |plat| drift_ios::webview::is_front(&plat.web)) == Some(true)
        })
        .find_map(|label| {
            if let Some(menu) = STATE.with_borrow(|s| s.menus.get(&label).cloned()) {
                return Some((Shown::Remote { label, revision: menu.revision }, menu));
            }
            let gnome =
                app.state::<AppState>().sessions.profile(&label).is_some_and(|p| p.kind() == HostKind::Gnome);
            gnome.then(|| (Shown::Gnome { label }, Arc::new(gnome_menu::snapshot())))
        });
    let wanted = front.as_ref().map(|(shown, _)| shown.clone());
    let changed = STATE.with_borrow(|s| s.shown != wanted);
    if !changed {
        return;
    }
    let app = app.clone();
    STATE.with_borrow_mut(|s| {
        let bar = s.bar.get_or_insert_with(|| {
            let bar = MenuBar::new(mtm, Rc::new(move |id| press(&app, id)));
            bar.install();
            bar
        });
        tracing::info!(
            shown = ?wanted,
            app = front.as_ref().map(|(_, m)| m.app.as_str()),
            menus = front.as_ref().map_or(0, |(_, m)| m.menus.len()),
            "menus in the menu bar"
        );
        bar.set(front.as_ref().map(|(_, m)| (**m).clone()));
        s.shown = wanted;
    });
}

/// The user chose item `id` in the menu bar.
fn press<R: Runtime>(app: &AppHandle<R>, id: u32) {
    let sessions = &app.state::<AppState>().sessions;
    match STATE.with_borrow(|s| s.shown.clone()) {
        Some(Shown::Remote { label, .. }) => {
            if let Err(e) = sessions.send(&label, SessionCommand::MenuPress(id)) {
                tracing::warn!(%label, error = %e, "could not choose a remote menu item");
            }
        }
        Some(Shown::Gnome { label }) => match gnome_menu::action(id) {
            Some(GnomeAction::Keys(keys)) => {
                let names: Vec<String> = keys.iter().map(|k| (*k).to_owned()).collect();
                for event in crate::phone::named_chord_events(&names).unwrap_or_default() {
                    if let Err(e) = sessions.send(&label, SessionCommand::Input(event)) {
                        tracing::warn!(%label, error = %e, "could not send a GNOME shortcut");
                        break;
                    }
                }
            }
            Some(GnomeAction::Disconnect) => super::close_session_window(app, &label),
            None => {}
        },
        None => {}
    }
}
