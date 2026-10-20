//! EXT-3 Red: the remote Mac app's menus in the iPad's menu bar (iPadOS 26 `UIMainMenuSystem`).
//! A snapshot becomes one `UIMenu` per remote menu: items keep their titles, checkmarks,
//! enabled state and submenus, separators become inline groups, shortcuts show as subtitles,
//! and choosing an item reports its id.
//!
//! Runs on the main thread of an iOS simulator process (`support/harness.rs`).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[cfg(target_os = "ios")]
#[path = "support/harness.rs"]
mod harness;

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() -> std::process::ExitCode {
    tests::run()
}

#[cfg(target_os = "ios")]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use drift_core::menus::{KeyEquivalent, MenuItem, MenuSnapshot, Modifiers};
    use drift_ios::menu_bar::{MenuBar, build_menus};
    use objc2::MainThreadMarker;
    use objc2_ui_kit::{UIAction, UIMenu, UIMenuElementAttributes, UIMenuElementState, UIMenuOptions};

    use super::harness;

    pub fn run() -> std::process::ExitCode {
        harness::run(&[
            ("one_menu_per_remote_menu", one_menu_per_remote_menu),
            ("items_keep_state_shortcuts_and_submenus", items_keep_state_shortcuts_and_submenus),
            ("choosing_an_item_reports_its_id", choosing_an_item_reports_its_id),
            ("the_menu_bar_installs_and_rebuilds", the_menu_bar_installs_and_rebuilds),
        ])
    }

    fn mtm() -> MainThreadMarker {
        MainThreadMarker::new().expect("main thread")
    }

    fn item(id: u32, title: &str) -> MenuItem {
        MenuItem {
            id,
            title: title.into(),
            enabled: true,
            checked: false,
            separator: false,
            key: None,
            children: vec![],
        }
    }

    fn snapshot() -> MenuSnapshot {
        let mut save = item(3, "Save");
        save.key = Some(KeyEquivalent { key: "s".into(), modifiers: Modifiers::COMMAND });
        let sep = MenuItem { separator: true, enabled: false, ..item(4, "") };
        let mut revert = item(5, "Revert");
        revert.enabled = false;
        let mut recent = item(6, "Open Recent");
        recent.children = vec![item(7, "notes.txt")];
        let mut file = item(2, "File");
        file.children = vec![save, sep, revert, recent];
        let mut ruler = item(9, "Show Ruler");
        ruler.checked = true;
        let mut format = item(8, "Format");
        format.children = vec![ruler];
        MenuSnapshot { revision: 1, app: "TextEdit".into(), menus: vec![file, format] }
    }

    fn one_menu_per_remote_menu() {
        let menus = build_menus(mtm(), &snapshot(), Rc::new(|_| {}));
        let titles: Vec<String> = menus.iter().map(|m| m.title().to_string()).collect();
        assert_eq!(titles, ["File", "Format"]);
    }

    fn items_keep_state_shortcuts_and_submenus() {
        let menus = build_menus(mtm(), &snapshot(), Rc::new(|_| {}));
        let file = menus[0].children().to_vec();
        // Save, then an inline group for what follows the separator.
        let save = file[0].downcast_ref::<UIAction>().expect("Save is an action");
        assert_eq!(save.title().to_string(), "Save");
        assert_eq!(save.subtitle().map(|s| s.to_string()).as_deref(), Some("⌘S"));
        let group = file[1].downcast_ref::<UIMenu>().expect("an inline group after the separator");
        assert!(group.options().contains(UIMenuOptions::DisplayInline));
        let after = group.children().to_vec();
        let revert = after[0].downcast_ref::<UIAction>().unwrap();
        assert!(revert.attributes().contains(UIMenuElementAttributes::Disabled));
        let recent = after[1].downcast_ref::<UIMenu>().expect("a submenu");
        assert_eq!(recent.children().len(), 1);
        let format = menus[1].children().to_vec();
        let ruler = format[0].downcast_ref::<UIAction>().unwrap();
        assert_eq!(ruler.state(), UIMenuElementState::On);
    }

    fn choosing_an_item_reports_its_id() {
        let picked: Rc<RefCell<Vec<u32>>> = Rc::default();
        let p = picked.clone();
        let bar = MenuBar::new(mtm(), Rc::new(move |id| p.borrow_mut().push(id)));
        bar.set(Some(snapshot()));
        bar.choose_for_tests(3);
        assert_eq!(*picked.borrow(), vec![3]);
    }

    fn the_menu_bar_installs_and_rebuilds() {
        let bar = MenuBar::new(mtm(), Rc::new(|_| {}));
        bar.install();
        bar.set(Some(snapshot()));
        assert_eq!(bar.menu_titles(), vec!["File".to_owned(), "Format".to_owned()]);
        bar.set(None);
        assert!(bar.menu_titles().is_empty());
    }
}
