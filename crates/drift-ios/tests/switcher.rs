//! IOS-11 Red: the switcher button over a session window. A `UIButton` in the top-leading corner
//! whose menu lists the switcher entries; the current one is checked, and choosing an entry
//! calls back with its id.
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

    use drift_ios::switcher::{MenuEntry, SwitcherButton};
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_ui_kit::UIView;

    use super::harness;

    pub fn run() -> std::process::ExitCode {
        harness::run(&[
            ("button_sits_in_the_top_leading_corner", button_sits_in_the_top_leading_corner),
            ("menu_lists_the_entries_and_checks_the_current", menu_lists_the_entries_and_checks_the_current),
            ("choosing_an_entry_calls_back_with_its_id", choosing_an_entry_calls_back_with_its_id),
        ])
    }

    fn mtm() -> MainThreadMarker {
        MainThreadMarker::new().expect("main thread")
    }

    fn root() -> objc2::rc::Retained<UIView> {
        let rect =
            CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 300.0 } };
        UIView::initWithFrame(UIView::alloc(mtm()), rect)
    }

    fn entries() -> Vec<MenuEntry> {
        vec![
            MenuEntry {
                id: "connections".into(),
                title: "Connections".into(),
                subtitle: None,
                checked: false,
            },
            MenuEntry {
                id: "session-0".into(),
                title: "Homelab".into(),
                subtitle: Some("Live".into()),
                checked: true,
            },
        ]
    }

    fn button_sits_in_the_top_leading_corner() {
        let root = root();
        let button = SwitcherButton::install(mtm(), &root, |_| {});
        let view = root.subviews().lastObject().unwrap();
        assert_eq!(view.class().name().to_str().unwrap(), "UIButton");
        let frame = view.frame();
        assert!(frame.origin.x < 40.0 && frame.origin.y < 80.0, "{frame:?}");
        assert!(frame.size.width >= 44.0 && frame.size.height >= 44.0, "a 44-point touch target");
        button.set_visible(false);
        assert!(view.isHidden());
    }

    fn menu_lists_the_entries_and_checks_the_current() {
        let root = root();
        let button = SwitcherButton::install(mtm(), &root, |_| {});
        button.set_entries(&entries());
        assert_eq!(button.titles(), vec!["Connections".to_owned(), "Homelab".to_owned()]);
        assert_eq!(button.checked(), vec![false, true]);
    }

    fn choosing_an_entry_calls_back_with_its_id() {
        let root = root();
        let picked: Rc<RefCell<Vec<String>>> = Rc::default();
        let p = picked.clone();
        let button = SwitcherButton::install(mtm(), &root, move |id| p.borrow_mut().push(id));
        button.set_entries(&entries());
        button.choose_for_tests(1);
        assert_eq!(*picked.borrow(), vec!["session-0".to_owned()]);
    }
}
