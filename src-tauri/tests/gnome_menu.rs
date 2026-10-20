//! UI-kinds Red (mockup-targets H): on iPad a GNOME session gets Drift's own menus in the menu
//! bar — Session, Keys and Activities (with Show Apps) — so the bar is not empty. A Mac keeps
//! mirroring its own menus (EXT-3).
#![allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures

use std::collections::HashSet;

use drift_app::gnome_menu::{GnomeAction, action, snapshot};
use drift_core::menus::MenuItem;

fn all(items: &[MenuItem]) -> Vec<&MenuItem> {
    items.iter().flat_map(|i| std::iter::once(i).chain(all(&i.children))).collect()
}

fn item<'a>(menus: &'a [MenuItem], title: &str) -> &'a MenuItem {
    all(menus).into_iter().find(|i| i.title == title).unwrap_or_else(|| panic!("no item {title}"))
}

#[test]
fn session_keys_and_activities_menus() {
    let menu = snapshot();
    let titles: Vec<&str> = menu.menus.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(titles, ["Session", "Keys", "Activities"]);
    assert_eq!(menu.app, "GNOME");
}

#[test]
fn every_item_does_something_and_ids_are_unique() {
    let menu = snapshot();
    let items = all(&menu.menus);
    let ids: HashSet<u32> = items.iter().map(|i| i.id).collect();
    assert_eq!(ids.len(), items.len(), "unique ids");
    for i in items.iter().filter(|i| i.is_pressable()) {
        assert!(action(i.id).is_some(), "{} has an action", i.title);
    }
    assert_eq!(action(0xdead), None);
}

#[test]
fn the_items_send_gnomes_shortcuts() {
    let menu = snapshot();
    let chord = |title: &str| action(item(&menu.menus, title).id);
    assert_eq!(chord("Activities Overview"), Some(GnomeAction::Keys(&["super"])));
    assert_eq!(chord("Show Apps"), Some(GnomeAction::Keys(&["super", "a"])));
    assert_eq!(chord("Super"), Some(GnomeAction::Keys(&["super"])));
    assert_eq!(chord("Ctrl+Alt+Delete"), Some(GnomeAction::Keys(&["control", "alt", "delete"])));
    assert_eq!(chord("Switch Windows"), Some(GnomeAction::Keys(&["alt", "tab"])));
    assert_eq!(chord("Lock Screen"), Some(GnomeAction::Keys(&["super", "l"])));
    assert_eq!(chord("Disconnect"), Some(GnomeAction::Disconnect));
    for keys in all(&menu.menus).iter().filter_map(|i| match action(i.id) {
        Some(GnomeAction::Keys(k)) => Some(k),
        _ => None,
    }) {
        let names: Vec<String> = keys.iter().map(|k| (*k).to_owned()).collect();
        assert!(drift_app::phone::named_chord_events(&names).is_some(), "{keys:?} are known keys");
    }
}
