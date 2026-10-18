//! `RemoteView` on iOS (task **IOS-4**; ADR `IOS-0` decision 8, ADR `M7-3` layering).
//!
//! A `UIView` subclass whose backing layer is the `CAMetalLayer` that `drift-render` draws into
//! on the session render thread. It sits under the page's `WKWebView` in the same superview
//! ([`insert_below`]), so the overlays and the HUD stay web code, as on macOS. The view:
//!
//! * keeps `contentsScale`/`drawableSize` equal to bounds × the screen's display scale in
//!   `layoutSubviews` and `didMoveToWindow`, and reports the [`ViewGeometry`] to its handler
//!   (Display Control, M4-2);
//! * hands input to its handler; the event sources (keyboard, pointer, touch) arrive with IOS-6
//!   and IOS-7.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use drift_core::{InputEvent, Size, ViewGeometry};
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_foundation::{CGRect, CGSize};
use objc2_quartz_core::CAMetalLayer;
use objc2_ui_kit::{
    NSObjectUIAccessibility as _, UIColor, UITraitEnvironment as _, UIView, UIViewAutoresizing,
};

/// What VoiceOver announces for the picture until the session names it (M9-4).
pub const DEFAULT_ACCESSIBILITY_LABEL: &str = "Remote desktop";

/// Receives what a [`RemoteView`] produces. Called on the main thread; implementations should
/// only forward (e.g. `SessionHandle::send`, which never blocks).
pub trait RemoteViewHandler {
    /// Input events to send, in order.
    fn input(&self, events: &[InputEvent]) {
        let _ = events;
    }
    /// The view's size or display scale changed (drives Display Control, M4-2).
    fn geometry_changed(&self, geometry: ViewGeometry) {
        let _ = geometry;
    }
    /// The view gained or lost focus.
    fn focus_changed(&self, focused: bool) {
        let _ = focused;
    }
}

/// Instance variables of [`RemoteView`].
pub struct Ivars {
    handler: RefCell<Option<Rc<dyn RemoteViewHandler>>>,
    geometry: Cell<ViewGeometry>,
    /// A fixed display scale (tests run without a window or screen).
    scale_override: Cell<Option<f64>>,
}

define_class!(
    /// The session's picture: a `UIView` backed by a `CAMetalLayer`.
    #[unsafe(super(UIView))]
    #[thread_kind = MainThreadOnly]
    #[name = "DriftRemoteView"]
    #[ivars = Ivars]
    pub struct RemoteView;

    impl RemoteView {
        #[unsafe(method(layerClass))]
        fn layer_class() -> &'static AnyClass {
            CAMetalLayer::class()
        }

        #[unsafe(method(layoutSubviews))]
        fn layout_subviews(&self) {
            // SAFETY: calling the superclass implementation, as UIKit requires.
            let () = unsafe { msg_send![super(self), layoutSubviews] };
            self.sync_drawable_size();
        }

        #[unsafe(method(didMoveToWindow))]
        fn did_move_to_window(&self) {
            // SAFETY: calling the superclass implementation, as UIKit requires.
            let () = unsafe { msg_send![super(self), didMoveToWindow] };
            self.sync_drawable_size();
        }
    }
);

impl RemoteView {
    /// Creates the view with `frame` (points).
    pub fn new(mtm: MainThreadMarker, frame: CGRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            handler: RefCell::new(None),
            geometry: Cell::new(ViewGeometry { points: Size::new(0.0, 0.0), backing_scale: 1.0 }),
            scale_override: Cell::new(None),
        });
        // SAFETY: `initWithFrame:` is UIView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.metal_layer().setOpaque(true);
        this.setOpaque(true);
        this.setAutoresizingMask(UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight);
        this.setIsAccessibilityElement(true, mtm);
        this.setAccessibilityLabel(
            Some(&objc2_foundation::NSString::from_str(DEFAULT_ACCESSIBILITY_LABEL)),
            mtm,
        );
        this.sync_drawable_size();
        this
    }

    /// The Metal layer (hand it to `drift_render::LayerTarget`).
    pub fn metal_layer(&self) -> &CAMetalLayer {
        let layer = self.layer();
        // SAFETY: `+layerClass` returns `CAMetalLayer`, so UIKit created the backing layer as
        // one; the view owns it for its whole lifetime, so the reference cannot outlive it.
        unsafe { &*Retained::as_ptr(&layer).cast::<CAMetalLayer>() }
    }

    /// Sets the receiver of input, geometry and focus notifications.
    pub fn set_handler(&self, handler: Rc<dyn RemoteViewHandler>) {
        *self.ivars().handler.borrow_mut() = Some(handler);
    }

    /// Sets what VoiceOver announces for the picture (M9-4).
    pub fn set_accessibility_label(&self, label: &str) {
        self.setAccessibilityLabel(Some(&objc2_foundation::NSString::from_str(label)), self.mtm());
    }

    /// The view's current geometry.
    pub fn geometry(&self) -> ViewGeometry {
        self.ivars().geometry.get()
    }

    /// Fixes the display scale (tests have no window or screen).
    #[doc(hidden)]
    pub fn set_scale_for_tests(&self, scale: f64) {
        self.ivars().scale_override.set(Some(scale));
    }

    /// The pixels-per-point of the screen the view is on.
    fn display_scale(&self) -> f64 {
        if let Some(scale) = self.ivars().scale_override.get() {
            return scale;
        }
        // SAFETY: a plain CGFloat getter on the window's current trait collection.
        let from_window = self.window().map(|w| unsafe { w.traitCollection().displayScale() });
        match from_window {
            Some(scale) if scale.is_finite() && scale > 0.0 => scale,
            _ => self.metal_layer().contentsScale(),
        }
    }

    /// Recomputes `contentsScale` and `drawableSize` from the bounds and the display scale, and
    /// reports a changed geometry to the handler.
    pub fn sync_drawable_size(&self) {
        let scale = self.display_scale();
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let size = self.bounds().size;
        let layer = self.metal_layer();
        self.setContentScaleFactor(scale);
        layer.setContentsScale(scale);
        layer.setDrawableSize(CGSize {
            width: (size.width * scale).round().max(1.0),
            height: (size.height * scale).round().max(1.0),
        });
        let geometry = ViewGeometry { points: Size::new(size.width, size.height), backing_scale: scale };
        if geometry != self.ivars().geometry.replace(geometry) {
            let handler = self.ivars().handler.borrow().clone();
            if let Some(h) = handler {
                h.geometry_changed(geometry);
            }
        }
    }

    /// Forwards input events to the handler (the IOS-6/IOS-7 event sources call this).
    pub fn send_input(&self, events: &[InputEvent]) {
        let handler = self.ivars().handler.borrow().clone();
        if let Some(h) = handler {
            h.input(events);
        }
    }
}

/// The page view had no superview to insert the picture into.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the page view is not in a view hierarchy")]
pub struct AttachError;

/// Inserts `view` directly below `reference` (the page's `WKWebView`) in the same superview,
/// with the same frame and autoresizing, and makes the page transparent so the picture shows
/// through it (ADR M7-3).
pub fn insert_below(reference: &UIView, view: &UIView) -> Result<(), AttachError> {
    let superview = reference.superview().ok_or(AttachError)?;
    view.setFrame(reference.frame());
    view.setAutoresizingMask(UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight);
    superview.insertSubview_belowSubview(view, reference);
    reference.setOpaque(false);
    reference.setBackgroundColor(Some(&UIColor::clearColor()));
    Ok(())
}
