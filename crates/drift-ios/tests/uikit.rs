//! IOS-4: the session's `RemoteView` on iOS — a `UIView` whose layer is the `CAMetalLayer`
//! `drift-render` draws into, placed under the page's `WKWebView` (ADR M7-3 layering), with a
//! drawable that tracks the view's bounds and the screen scale, and ProMotion frame pacing.
//!
//! Runs on the main thread of an iOS simulator process (`support/harness.rs`):
//! `xcrun simctl spawn <device> target/aarch64-apple-ios-sim/debug/deps/uikit-<hash>`.
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

    use drift_core::{Size, ViewGeometry};
    use drift_ios::pacing::FramePacing;
    use drift_ios::view::{RemoteView, RemoteViewHandler, insert_below};
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_ui_kit::UIView;

    use super::harness;

    pub fn run() -> std::process::ExitCode {
        harness::run(&[
            ("remote_view_layer_is_a_metal_layer", remote_view_layer_is_a_metal_layer),
            ("drawable_size_tracks_bounds_and_scale", drawable_size_tracks_bounds_and_scale),
            ("geometry_changes_reach_the_handler_once", geometry_changes_reach_the_handler_once),
            ("insert_below_puts_the_view_under_the_page", insert_below_puts_the_view_under_the_page),
            ("frame_pacing_asks_for_up_to_120_hz", frame_pacing_asks_for_up_to_120_hz),
        ])
    }

    fn mtm() -> MainThreadMarker {
        MainThreadMarker::new().expect("tests run on the main thread")
    }

    fn rect(w: f64, h: f64) -> CGRect {
        CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: w, height: h } }
    }

    #[derive(Default)]
    struct Recorder {
        geometry: RefCell<Vec<ViewGeometry>>,
    }

    impl RemoteViewHandler for Recorder {
        fn geometry_changed(&self, geometry: ViewGeometry) {
            self.geometry.borrow_mut().push(geometry);
        }
    }

    fn remote_view_layer_is_a_metal_layer() {
        let view = RemoteView::new(mtm(), rect(100.0, 50.0));
        let layer = view.layer();
        assert_eq!(layer.class().name().to_str().unwrap(), "CAMetalLayer");
        assert!(std::ptr::eq(
            Retained::as_ptr(&layer).cast::<u8>(),
            (view.metal_layer() as *const objc2_quartz_core::CAMetalLayer).cast::<u8>()
        ));
        assert!(view.metal_layer().isOpaque());
    }

    fn drawable_size_tracks_bounds_and_scale() {
        let view = RemoteView::new(mtm(), rect(200.0, 100.0));
        view.set_scale_for_tests(3.0);
        view.sync_drawable_size();
        let size = view.metal_layer().drawableSize();
        assert_eq!((size.width, size.height), (600.0, 300.0));
        assert_eq!(view.metal_layer().contentsScale(), 3.0);

        view.setFrame(rect(320.5, 200.25));
        view.sync_drawable_size();
        let size = view.metal_layer().drawableSize();
        assert_eq!((size.width, size.height), (962.0, 601.0), "rounded to whole pixels");
        assert_eq!(view.geometry(), ViewGeometry { points: Size::new(320.5, 200.25), backing_scale: 3.0 });
    }

    fn geometry_changes_reach_the_handler_once() {
        let view = RemoteView::new(mtm(), rect(10.0, 10.0));
        let recorder = Rc::new(Recorder::default());
        view.set_handler(recorder.clone());
        view.set_scale_for_tests(2.0);
        view.setFrame(rect(300.0, 200.0));
        view.sync_drawable_size();
        view.sync_drawable_size();
        let seen = recorder.geometry.borrow();
        assert_eq!(seen.last(), Some(&ViewGeometry { points: Size::new(300.0, 200.0), backing_scale: 2.0 }));
        assert_eq!(seen.iter().filter(|g| g.points == Size::new(300.0, 200.0)).count(), 1, "{seen:?}");
    }

    fn insert_below_puts_the_view_under_the_page() {
        let mtm = mtm();
        let root = UIView::initWithFrame(UIView::alloc(mtm), rect(400.0, 300.0));
        let page = UIView::initWithFrame(UIView::alloc(mtm), rect(400.0, 300.0));
        root.addSubview(&page);
        let view = RemoteView::new(mtm, rect(1.0, 1.0));
        insert_below(&page, &view).expect("page has a superview");
        let subviews = root.subviews();
        assert_eq!(subviews.len(), 2);
        assert!(std::ptr::eq(
            Retained::as_ptr(&subviews.objectAtIndex(0)).cast::<u8>(),
            Retained::as_ptr(&view).cast()
        ));
        assert!(std::ptr::eq(
            Retained::as_ptr(&subviews.objectAtIndex(1)).cast::<u8>(),
            Retained::as_ptr(&page).cast()
        ));
        let frame = view.frame();
        assert_eq!((frame.size.width, frame.size.height), (400.0, 300.0), "fills the page's frame");
        assert!(!page.isOpaque(), "the page must let the picture show through");

        let orphan = UIView::initWithFrame(UIView::alloc(mtm), rect(1.0, 1.0));
        assert!(insert_below(&orphan, &RemoteView::new(mtm, rect(1.0, 1.0))).is_err());
    }

    fn frame_pacing_asks_for_up_to_120_hz() {
        let pacing = FramePacing::new(mtm());
        let range = pacing.preferred_range();
        assert_eq!(range.maximum, 120.0);
        assert_eq!(range.preferred, 120.0);
        assert!(range.minimum <= 60.0);
        assert!(pacing.is_paused(), "starts paused until a session is visible");
        pacing.set_active(true);
        assert!(!pacing.is_paused());
        pacing.set_active(false);
        assert!(pacing.is_paused());
    }
}
