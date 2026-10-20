//! The front session's menus in the iPad's menu bar (task **UI-kinds**): Drift's own GNOME
//! menus ([`crate::gnome_menu`]), whose items send GNOME's shortcuts or disconnect. iPhones have
//! no menu bar, so nothing is installed there.

use std::cell::RefCell;
use std::rc::Rc;

use drift_ios::menu_bar::MenuBar;
use drift_rdp::SessionCommand;
use objc2::MainThreadMarker;
use tauri::{AppHandle, Manager as _, Runtime};

use crate::commands::AppState;
use crate::gnome_menu::{self, GnomeAction};

#[derive(Default)]
struct State {
    bar: Option<MenuBar>,
    /// The session window whose menus are shown.
    shown: Option<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Session window `label` is gone or lost its session.
pub(super) fn forget<R: Runtime>(app: &AppHandle<R>, _label: &str) {
    if let Some(mtm) = MainThreadMarker::new() {
        refresh(app, mtm);
    }
}

/// Shows Drift's GNOME menus for the front session window.
pub(super) fn refresh<R: Runtime>(app: &AppHandle<R>, mtm: MainThreadMarker) {
    if drift_ios::view::is_phone(mtm) {
        return;
    }
    let front = super::session_labels()
        .into_iter()
        .filter(|label| {
            super::with_platform(label, |plat| drift_ios::webview::is_front(&plat.web)) == Some(true)
        })
        .find(|label| app.state::<AppState>().sessions.profile(label).is_some());
    let wanted = front;
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
        tracing::info!(shown = ?wanted, "menus in the menu bar");
        bar.set(wanted.as_ref().map(|_| gnome_menu::snapshot()));
        s.shown = wanted;
    });
}

/// The user chose item `id` in the menu bar.
fn press<R: Runtime>(app: &AppHandle<R>, id: u32) {
    let sessions = &app.state::<AppState>().sessions;
    let Some(label) = STATE.with_borrow(|s| s.shown.clone()) else { return };
    match gnome_menu::action(id) {
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
    }
}
