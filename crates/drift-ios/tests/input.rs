//! IOS-6: hardware keyboard, pointer, pointer lock and Pencil hover on the iOS `RemoteView`.
//!
//! The UIKit and GameController callbacks are humble glue; these smoke tests drive the view's
//! entry points with the plain values those callbacks extract and check what reaches the
//! session, plus the pieces UIKit must see (recognizers, pointer interaction, pointer lock).
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

    use drift_core::{DesktopSize, InputEvent, MouseButton, Point, ViewGeometry};
    use drift_input::ScrollDelta;
    use drift_ios::pointer_lock;
    use drift_ios::view::{RemoteView, RemoteViewHandler};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, ClassBuilder};
    use objc2::{ClassType, MainThreadMarker, msg_send};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_ui_kit::UIViewController;

    use super::harness;

    pub fn run() -> std::process::ExitCode {
        harness::run(&[
            ("hover_maps_through_the_viewport", hover_maps_through_the_viewport),
            ("hid_keys_reach_the_session", hid_keys_reach_the_session),
            (
                "locked_deltas_buttons_and_scroll_reach_the_session",
                locked_deltas_buttons_and_scroll_reach_the_session,
            ),
            ("focus_loss_releases_keys_and_buttons", focus_loss_releases_keys_and_buttons),
            (
                "view_installs_hover_scroll_and_pointer_interaction",
                view_installs_hover_scroll_and_pointer_interaction,
            ),
            ("pointer_lock_is_per_view_controller", pointer_lock_is_per_view_controller),
            ("direct_touch_clicks_on_the_picture", direct_touch_clicks_on_the_picture),
            ("trackpad_touch_moves_relatively", trackpad_touch_moves_relatively),
            ("pinch_transforms_the_view_on_phones", pinch_transforms_the_view_on_phones),
        ])
    }

    fn mtm() -> MainThreadMarker {
        MainThreadMarker::new().expect("main thread")
    }

    fn rect(w: f64, h: f64) -> CGRect {
        CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: w, height: h } }
    }

    #[derive(Default)]
    struct Recorder {
        events: RefCell<Vec<InputEvent>>,
    }

    impl RemoteViewHandler for Recorder {
        fn input(&self, events: &[InputEvent]) {
            self.events.borrow_mut().extend_from_slice(events);
        }
        fn geometry_changed(&self, _geometry: ViewGeometry) {}
    }

    /// 1000×500 points showing a 2000×1000 desktop (0.5 points per pixel).
    fn attached() -> (Retained<RemoteView>, Rc<Recorder>) {
        let view = RemoteView::new(mtm(), rect(1000.0, 500.0));
        view.set_scale_for_tests(2.0);
        view.sync_drawable_size();
        let recorder = Rc::new(Recorder::default());
        view.set_handler(recorder.clone());
        view.set_desktop(DesktopSize { width: 2000, height: 1000 });
        (view, recorder)
    }

    fn hover_maps_through_the_viewport() {
        let (view, rec) = attached();
        view.pointer_hover(Point::new(100.0, 50.0));
        assert_eq!(*rec.events.borrow(), vec![InputEvent::MouseMove { x: 200, y: 100 }]);
        view.clear_desktop();
        view.pointer_hover(Point::new(10.0, 10.0));
        assert_eq!(rec.events.borrow().len(), 1, "no desktop, no pointer");
    }

    fn hid_keys_reach_the_session() {
        let (view, rec) = attached();
        view.hid_key(0x04, true);
        view.hid_key(0x04, false);
        assert_eq!(
            *rec.events.borrow(),
            vec![
                InputEvent::Key { scancode: 0x1E, extended: false, down: true },
                InputEvent::Key { scancode: 0x1E, extended: false, down: false },
            ]
        );
    }

    fn locked_deltas_buttons_and_scroll_reach_the_session() {
        let (view, rec) = attached();
        view.pointer_hover(Point::new(0.0, 0.0));
        view.pointer_delta(10.0, 5.0);
        view.pointer_button(MouseButton::Left, true);
        view.pointer_button(MouseButton::Left, false);
        view.pointer_scroll(ScrollDelta::Lines { dx: 0.0, dy: 1.0 }, true);
        assert_eq!(
            *rec.events.borrow(),
            vec![
                InputEvent::MouseMove { x: 0, y: 0 },
                InputEvent::MouseMove { x: 20, y: 10 },
                InputEvent::MouseButton { button: MouseButton::Left, down: true, x: 20, y: 10 },
                InputEvent::MouseButton { button: MouseButton::Left, down: false, x: 20, y: 10 },
                InputEvent::Wheel { horizontal: false, units: 120 },
            ]
        );
    }

    fn focus_loss_releases_keys_and_buttons() {
        let (view, rec) = attached();
        view.pointer_hover(Point::new(5.0, 5.0));
        view.hid_key(0xE1, true);
        view.pointer_button(MouseButton::Right, true);
        rec.events.borrow_mut().clear();
        view.release_all();
        assert_eq!(
            *rec.events.borrow(),
            vec![
                InputEvent::Key { scancode: 0x2A, extended: false, down: false },
                InputEvent::MouseButton { button: MouseButton::Right, down: false, x: 10, y: 10 },
            ]
        );
    }

    fn view_installs_hover_scroll_and_pointer_interaction() {
        let view = RemoteView::new(mtm(), rect(10.0, 10.0));
        let names: Vec<String> = view
            .gestureRecognizers()
            .map(|a| a.iter().map(|g| g.class().name().to_str().unwrap().to_owned()).collect())
            .unwrap_or_default();
        assert!(names.iter().any(|n| n == "UIHoverGestureRecognizer"), "{names:?}");
        assert!(names.iter().any(|n| n == "UIPanGestureRecognizer"), "{names:?}");
        let interactions: Vec<String> = view
            .interactions()
            .iter()
            .map(|i| AsRef::<AnyObject>::as_ref(&*i).class().name().to_str().unwrap().to_owned())
            .collect();
        assert!(interactions.iter().any(|n| n == "UIPointerInteraction"), "{interactions:?}");
    }

    fn pointer_lock_is_per_view_controller() {
        // A runtime-declared subclass that does not override `prefersPointerLocked`, like tao's
        // `TaoUIViewController`.
        let name = c"DriftTestTaoLikeVC";
        let class = ClassBuilder::new(name, UIViewController::class()).map(|b| b.register()).unwrap();
        let make = || -> Retained<AnyObject> {
            // SAFETY: plain alloc/init of a UIViewController subclass on the main thread.
            unsafe {
                let alloc: *mut AnyObject = msg_send![class, alloc];
                let obj: *mut AnyObject = msg_send![alloc, init];
                Retained::from_raw(obj).unwrap()
            }
        };
        let (a, b) = (make(), make());
        let locked = |vc: &AnyObject| -> bool {
            // SAFETY: `prefersPointerLocked` is a BOOL getter on every UIViewController.
            let v: Bool = unsafe { msg_send![vc, prefersPointerLocked] };
            v.as_bool()
        };
        assert!(!locked(&a));
        pointer_lock::set_locked(mtm(), &a, true);
        assert!(locked(&a));
        assert!(!locked(&b), "only the asked-for view controller locks");
        pointer_lock::set_locked(mtm(), &a, false);
        assert!(!locked(&a));
    }

    fn direct_touch_clicks_on_the_picture() {
        let (view, rec) = attached();
        view.set_touch(drift_core::TouchMode::Direct, false);
        view.finger_began(1, Point::new(100.0, 50.0), 0.0);
        view.finger_ended(1, Point::new(100.0, 50.0), 0.1);
        assert_eq!(
            *rec.events.borrow(),
            vec![
                InputEvent::MouseMove { x: 200, y: 100 },
                InputEvent::MouseButton { button: MouseButton::Left, down: true, x: 200, y: 100 },
                InputEvent::MouseButton { button: MouseButton::Left, down: false, x: 200, y: 100 },
            ]
        );
    }

    fn trackpad_touch_moves_relatively() {
        let (view, rec) = attached();
        view.set_touch(drift_core::TouchMode::Trackpad, false);
        view.pointer_hover(Point::new(0.0, 0.0));
        view.finger_began(1, Point::new(500.0, 250.0), 0.0);
        view.finger_moved(1, Point::new(520.0, 250.0), 0.05);
        assert_eq!(rec.events.borrow().last(), Some(&InputEvent::MouseMove { x: 60, y: 0 }));
    }

    fn pinch_transforms_the_view_on_phones() {
        let (view, _rec) = attached();
        view.set_touch(drift_core::TouchMode::Direct, true);
        view.finger_began(1, Point::new(400.0, 250.0), 0.0);
        view.finger_began(2, Point::new(600.0, 250.0), 0.0);
        view.finger_moved(1, Point::new(300.0, 250.0), 0.05);
        view.finger_moved(2, Point::new(700.0, 250.0), 0.05);
        let t = view.transform();
        assert!(t.a > 1.5, "zoomed in: {t:?}");
        view.reset_zoom();
        assert_eq!(view.transform().a, 1.0);
    }
}
