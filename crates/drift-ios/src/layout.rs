//! Size-change notifications for the window's root view (task **IOS-8**).
//!
//! The iPhone layout depends on the container's size and orientation, and nothing Drift owns is
//! told when tao's root view rotates or resizes. A [`ResizeWatcher`] is an invisible, full-size,
//! touch-transparent subview whose `layoutSubviews` reports each new size once.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_foundation::CGRect;
use objc2_ui_kit::{UIColor, UIView, UIViewAutoresizing};

/// Instance variables of [`ResizeWatcher`].
pub struct Ivars {
    on_resize: RefCell<Box<dyn Fn(f64, f64)>>,
    last: Cell<Option<(f64, f64)>>,
}

define_class!(
    /// An invisible view that reports its size whenever it changes.
    #[unsafe(super(UIView))]
    #[thread_kind = MainThreadOnly]
    #[name = "DriftResizeWatcher"]
    #[ivars = Ivars]
    pub struct ResizeWatcher;

    impl ResizeWatcher {
        #[unsafe(method(layoutSubviews))]
        fn layout_subviews(&self) {
            // SAFETY: calling the superclass implementation, as UIKit requires.
            let () = unsafe { msg_send![super(self), layoutSubviews] };
            let size = self.bounds().size;
            let now = (size.width, size.height);
            if self.ivars().last.replace(Some(now)) != Some(now) {
                (self.ivars().on_resize.borrow())(now.0, now.1);
            }
        }
    }
);

impl ResizeWatcher {
    /// A watcher with `frame` that calls `on_resize(width, height)` for every new size.
    pub fn new(mtm: MainThreadMarker, frame: CGRect, on_resize: Box<dyn Fn(f64, f64)>) -> Retained<Self> {
        let this =
            Self::alloc(mtm).set_ivars(Ivars { on_resize: RefCell::new(on_resize), last: Cell::new(None) });
        // SAFETY: `initWithFrame:` is UIView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setUserInteractionEnabled(false);
        this.setOpaque(false);
        this.setBackgroundColor(Some(&UIColor::clearColor()));
        this.setAutoresizingMask(UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight);
        this
    }

    /// Adds the watcher at the back of `container`, filling it.
    pub fn install(&self, container: &UIView) {
        self.setFrame(container.bounds());
        container.insertSubview_atIndex(self, 0);
    }
}

/// Asks iOS to turn the scene showing `view` to landscape (`true`) or portrait. Used by
/// simulator smoke runs, which have no hardware to rotate; iOS may refuse, which is logged.
pub fn request_orientation(view: &UIView, landscape: bool) {
    use objc2::AllocAnyThread as _;
    use objc2_ui_kit::{UIInterfaceOrientationMask, UIWindowSceneGeometryPreferencesIOS};

    let Some(scene) = view.window().and_then(|w| w.windowScene()) else { return };
    let mask = if landscape {
        UIInterfaceOrientationMask::LandscapeRight
    } else {
        UIInterfaceOrientationMask::Portrait
    };
    let prefs = UIWindowSceneGeometryPreferencesIOS::initWithInterfaceOrientations(
        UIWindowSceneGeometryPreferencesIOS::alloc(),
        mask,
    );
    let handler = block2::RcBlock::new(|error: std::ptr::NonNull<objc2_foundation::NSError>| {
        // SAFETY: UIKit passes a valid error for the duration of the call.
        tracing::warn!(error = %unsafe { error.as_ref() }.localizedDescription(), "orientation request refused");
    });
    scene.requestGeometryUpdateWithPreferences_errorHandler(&prefs, Some(&handler));
}
