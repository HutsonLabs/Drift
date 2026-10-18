//! `RemoteView` on iOS (task **IOS-4**; ADR `IOS-0` decision 8, ADR `M7-3` layering).
//!
//! A `UIView` subclass whose backing layer is the `CAMetalLayer` that `drift-render` draws into
//! on the session render thread. It sits under the page's `WKWebView` in the same superview
//! ([`insert_below`]), so the overlays and the HUD stay web code, as on macOS. The view:
//!
//! * keeps `contentsScale`/`drawableSize` equal to bounds × the screen's display scale in
//!   `layoutSubviews` and `didMoveToWindow`, and reports the [`ViewGeometry`] to its handler
//!   (Display Control, M4-2);
//! * turns input into [`InputEvent`]s through `drift-input` (IOS-6): hardware keys arrive as HID
//!   usages ([`RemoteView::hid_key`], fed by [`crate::hardware`]); the iPad pointer and Apple
//!   Pencil hover move the remote pointer (`UIHoverGestureRecognizer`); pointer clicks arrive as
//!   indirect-pointer touches, with the secondary button as a right click; wheel and trackpad
//!   scrolling come from a scroll-only `UIPanGestureRecognizer`; under pointer lock
//!   [`crate::hardware`] feeds `GCMouse` deltas, buttons and scroll instead;
//! * turns fingers and the Pencil into pointer input in trackpad or direct mode, with two-finger
//!   scroll and right click and, on iPhone, a local pinch zoom applied as the view's transform
//!   (IOS-7, `drift_input::touch`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use drift_core::{
    CmdAs, DesktopSize, InputEvent, MomentumPhase, MouseButton, Point, ScrollPhase, Size, TouchMode,
    ViewGeometry,
};
use drift_input::{
    HidKeyboard, Pointer, ScaleMode, ScrollConfig, ScrollDelta, TouchInputMode, TouchOutput, Touches, Zoom,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_core_foundation::{CGAffineTransform, CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSSet};
use objc2_quartz_core::CAMetalLayer;
use objc2_ui_kit::{
    NSObjectUIAccessibility as _, UIColor, UIEvent, UIEventButtonMask, UIGestureRecognizer,
    UIGestureRecognizerState, UIHoverGestureRecognizer, UIPanGestureRecognizer, UIPointerInteraction,
    UIPressesEvent, UIScrollTypeMask, UITouch, UITouchType, UITraitEnvironment as _, UIView,
    UIViewAutoresizing,
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
    /// Pointer position, buttons and scroll remainder (`drift-input`).
    pointer: RefCell<Pointer>,
    /// Held hardware keys.
    keyboard: RefCell<HidKeyboard>,
    /// The pointer press in progress used the secondary button.
    pressed_secondary: Cell<bool>,
    /// Finger gestures (IOS-7).
    touches: RefCell<Touches>,
    /// Local pinch zoom (iPhone).
    zoom: RefCell<Zoom>,
    /// The on-screen trackpad below the picture (iPhone portrait, IOS-8); always trackpad mode.
    pad_touches: RefCell<Touches>,
    /// A strip at the bottom edge whose touches reveal the landscape controls (IOS-8).
    reveal: RefCell<Option<RevealStrip>>,
    /// The finger that hit the reveal strip; the rest of its gesture is ignored.
    revealing: Cell<Option<u64>>,
}

/// A bottom strip's height in points and what touching it does.
type RevealStrip = (f64, Box<dyn Fn()>);

/// Where an indirect-pointer touch is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TouchPhase {
    Began,
    Moved,
    Ended,
    Cancelled,
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

        #[unsafe(method(canBecomeFirstResponder))]
        fn can_become_first_responder(&self) -> bool {
            true
        }

        /// iPad pointer and Apple Pencil hover: absolute moves.
        #[unsafe(method(driftHover:))]
        fn drift_hover(&self, recognizer: &UIGestureRecognizer) {
            let p = recognizer.locationInView(Some(self));
            self.pointer_hover(Point::new(p.x, p.y));
        }

        /// Wheel and two-finger trackpad scrolling (scroll events only, no touches).
        #[unsafe(method(driftScroll:))]
        fn drift_scroll(&self, recognizer: &UIPanGestureRecognizer) {
            let t = recognizer.translationInView(Some(self));
            recognizer.setTranslation_inView(CGPoint { x: 0.0, y: 0.0 }, Some(self));
            // EXT-2: the recognizer's state is the scroll's phase for a Drift Host.
            let phase = match recognizer.state() {
                UIGestureRecognizerState::Began => ScrollPhase::Began,
                UIGestureRecognizerState::Ended => ScrollPhase::Ended,
                UIGestureRecognizerState::Cancelled | UIGestureRecognizerState::Failed => ScrollPhase::Cancelled,
                _ => ScrollPhase::Changed,
            };
            self.pointer_precise_scroll(t.x, t.y, phase);
        }

        #[unsafe(method(touchesBegan:withEvent:))]
        fn touches_began(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            if !self.touches(touches, event, TouchPhase::Began) {
                // SAFETY: forwarding to the superclass, as UIKit requires for unhandled touches.
                let () = unsafe { msg_send![super(self), touchesBegan: touches, withEvent: event] };
            }
        }

        #[unsafe(method(touchesMoved:withEvent:))]
        fn touches_moved(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            if !self.touches(touches, event, TouchPhase::Moved) {
                // SAFETY: as above.
                let () = unsafe { msg_send![super(self), touchesMoved: touches, withEvent: event] };
            }
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn touches_ended(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            if !self.touches(touches, event, TouchPhase::Ended) {
                // SAFETY: as above.
                let () = unsafe { msg_send![super(self), touchesEnded: touches, withEvent: event] };
            }
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn touches_cancelled(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            if !self.touches(touches, event, TouchPhase::Cancelled) {
                // SAFETY: as above.
                let () = unsafe { msg_send![super(self), touchesCancelled: touches, withEvent: event] };
            }
        }

        /// Hardware keys reach the session through `GCKeyboard` ([`crate::hardware`]); the
        /// presses UIKit also delivers are swallowed so they neither reach the page nor beep.
        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, _presses: &NSSet<AnyObject>, _event: Option<&UIPressesEvent>) {}

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(&self, _presses: &NSSet<AnyObject>, _event: Option<&UIPressesEvent>) {}

        #[unsafe(method(pressesCancelled:withEvent:))]
        fn presses_cancelled(&self, _presses: &NSSet<AnyObject>, _event: Option<&UIPressesEvent>) {}
    }
);

impl RemoteView {
    /// Creates the view with `frame` (points).
    pub fn new(mtm: MainThreadMarker, frame: CGRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            handler: RefCell::new(None),
            geometry: Cell::new(ViewGeometry { points: Size::new(0.0, 0.0), backing_scale: 1.0 }),
            scale_override: Cell::new(None),
            pointer: RefCell::new(Pointer::new(ScrollConfig::default())),
            keyboard: RefCell::new(HidKeyboard::new(CmdAs::default())),
            pressed_secondary: Cell::new(false),
            touches: RefCell::new(Touches::new(TouchInputMode::Direct, false)),
            zoom: RefCell::new(Zoom::default()),
            pad_touches: RefCell::new(Touches::new(TouchInputMode::Trackpad, false)),
            reveal: RefCell::new(None),
            revealing: Cell::new(None),
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
        // Two-finger scroll, right click and pinch need every finger (IOS-7).
        this.setMultipleTouchEnabled(true);
        this.install_recognizers(mtm);
        this.sync_drawable_size();
        this
    }

    fn install_recognizers(&self, mtm: MainThreadMarker) {
        // SAFETY: the view is the target and outlives its own recognizers; `driftHover:` and
        // `driftScroll:` take the recognizer, as UIKit sends it.
        let hover = unsafe {
            UIHoverGestureRecognizer::initWithTarget_action(
                UIHoverGestureRecognizer::alloc(mtm),
                Some(self),
                Some(sel!(driftHover:)),
            )
        };
        self.addGestureRecognizer(&hover);
        // SAFETY: as above.
        let scroll = unsafe {
            UIPanGestureRecognizer::initWithTarget_action(
                UIPanGestureRecognizer::alloc(mtm),
                Some(self),
                Some(sel!(driftScroll:)),
            )
        };
        scroll.setAllowedScrollTypesMask(UIScrollTypeMask::All);
        // No touch types: this recognizer sees scroll events only, never fingers.
        scroll.setAllowedTouchTypes(&NSArray::new());
        self.addGestureRecognizer(&scroll);
        let pointer = UIPointerInteraction::initWithDelegate(UIPointerInteraction::alloc(mtm), None);
        self.addInteraction(ProtocolObject::from_ref(&*pointer));
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
            self.ivars().pointer.borrow_mut().set_view_geometry(geometry);
            self.ivars().zoom.borrow_mut().set_view(geometry.points);
            self.apply_zoom();
            let handler = self.ivars().handler.borrow().clone();
            if let Some(h) = handler {
                h.geometry_changed(geometry);
            }
        }
    }

    /// Attaches the remote desktop (pointer mapping; the picture is fitted, as on macOS).
    pub fn set_desktop(&self, desktop: DesktopSize) {
        self.ivars().pointer.borrow_mut().set_desktop(desktop, ScaleMode::Fit);
    }

    /// Detaches the desktop (disconnected): pointer input is dropped.
    pub fn clear_desktop(&self) {
        self.ivars().pointer.borrow_mut().clear_desktop();
    }

    /// Where Command goes (the profile's keyboard preference).
    pub fn set_cmd_as(&self, cmd_as: CmdAs) {
        self.ivars().keyboard.borrow_mut().set_cmd_as(cmd_as);
    }

    /// The viewport the pointer maps through, if a desktop is attached.
    pub fn viewport(&self) -> Option<drift_input::Viewport> {
        self.ivars().pointer.borrow().viewport()
    }

    /// A hardware key, as a HID usage.
    pub fn hid_key(&self, usage: u16, down: bool) {
        let events = self.ivars().keyboard.borrow_mut().key(usage, down);
        self.send_input(&events);
    }

    /// An absolute pointer position in view points (hover, pointer clicks).
    pub fn pointer_hover(&self, point: Point<f64>) {
        let events = self.ivars().pointer.borrow_mut().move_to(point);
        self.send_input(&events);
    }

    /// A relative pointer move in view points (pointer lock).
    pub fn pointer_delta(&self, dx: f64, dy: f64) {
        let events = self.ivars().pointer.borrow_mut().move_by(dx, dy);
        self.send_input(&events);
    }

    /// A pointer button where the pointer is.
    pub fn pointer_button(&self, button: MouseButton, down: bool) {
        let events = self.ivars().pointer.borrow_mut().button(button, down);
        self.send_input(&events);
    }

    /// Wheel or trackpad scrolling.
    pub fn pointer_scroll(&self, delta: ScrollDelta, began: bool) {
        let events = self.ivars().pointer.borrow_mut().scroll(delta, began);
        self.send_input(&events);
    }

    /// A trackpad scroll with its phase (EXT-2): one precise scroll event carrying its wheel
    /// fallback.
    pub fn pointer_precise_scroll(&self, dx: f64, dy: f64, phase: ScrollPhase) {
        let event = self.ivars().pointer.borrow_mut().precise_scroll(dx, dy, phase, MomentumPhase::None);
        self.send_input(&[event]);
    }

    /// Releases held keys and buttons (focus loss, background, another session in front).
    pub fn release_all(&self) {
        let mut events = self.ivars().keyboard.borrow_mut().release_all();
        events.extend(self.ivars().pointer.borrow_mut().release_all());
        self.send_input(&events);
    }

    /// Sets the touch mode for this session; `phone` resolves [`TouchMode::Auto`] and enables
    /// the local pinch zoom (iPhone).
    pub fn set_touch(&self, mode: TouchMode, phone: bool) {
        let mode = match mode.resolve(phone) {
            TouchMode::Trackpad => TouchInputMode::Trackpad,
            _ => TouchInputMode::Direct,
        };
        let mut touches = self.ivars().touches.borrow_mut();
        touches.set_mode(mode);
        touches.set_zoom(phone);
    }

    /// The current touch mode.
    pub fn touch_mode(&self) -> TouchMode {
        match self.ivars().touches.borrow().mode() {
            TouchInputMode::Trackpad => TouchMode::Trackpad,
            TouchInputMode::Direct => TouchMode::Direct,
        }
    }

    /// Makes touches that start in the bottom `strip` points call `reveal` instead of reaching
    /// the remote (iPhone landscape, IOS-8); `None` turns it off.
    pub fn set_reveal_strip(&self, strip: Option<f64>, reveal: Box<dyn Fn()>) {
        *self.ivars().reveal.borrow_mut() = strip.map(|s| (s, reveal));
    }

    /// A finger (or the Pencil) touched down at `at` (screen points: the superview's space).
    pub fn finger_began(&self, id: u64, at: Point<f64>, t: f64) {
        let height = self.bounds().size.height;
        let hit = self.ivars().reveal.borrow().as_ref().is_some_and(|(strip, _)| at.y >= height - strip);
        if hit {
            self.ivars().revealing.set(Some(id));
            if let Some((_, reveal)) = self.ivars().reveal.borrow().as_ref() {
                reveal();
            }
            return;
        }
        let zoom = *self.ivars().zoom.borrow();
        let out =
            self.ivars().touches.borrow_mut().began(&mut self.ivars().pointer.borrow_mut(), &zoom, id, at, t);
        self.apply_touch(out);
    }

    /// A finger moved.
    pub fn finger_moved(&self, id: u64, at: Point<f64>, t: f64) {
        if self.ivars().revealing.get() == Some(id) {
            return;
        }
        let zoom = *self.ivars().zoom.borrow();
        let out =
            self.ivars().touches.borrow_mut().moved(&mut self.ivars().pointer.borrow_mut(), &zoom, id, at, t);
        self.apply_touch(out);
    }

    /// A finger lifted.
    pub fn finger_ended(&self, id: u64, at: Point<f64>, t: f64) {
        if self.ivars().revealing.get() == Some(id) {
            self.ivars().revealing.set(None);
            return;
        }
        let zoom = *self.ivars().zoom.borrow();
        let out =
            self.ivars().touches.borrow_mut().ended(&mut self.ivars().pointer.borrow_mut(), &zoom, id, at, t);
        self.apply_touch(out);
    }

    /// UIKit cancelled a finger.
    pub fn finger_cancelled(&self, id: u64) {
        if self.ivars().revealing.get() == Some(id) {
            self.ivars().revealing.set(None);
            return;
        }
        let out = self.ivars().touches.borrow_mut().cancelled(&mut self.ivars().pointer.borrow_mut(), id);
        self.apply_touch(out);
    }

    /// A finger on the on-screen trackpad went down (the pad's own points; only deltas matter).
    pub fn pad_began(&self, id: u64, at: Point<f64>, t: f64) {
        let out = self.ivars().pad_touches.borrow_mut().began(
            &mut self.ivars().pointer.borrow_mut(),
            &Zoom::default(),
            id,
            at,
            t,
        );
        self.send_input(&out.events);
    }

    /// A finger on the trackpad moved.
    pub fn pad_moved(&self, id: u64, at: Point<f64>, t: f64) {
        let out = self.ivars().pad_touches.borrow_mut().moved(
            &mut self.ivars().pointer.borrow_mut(),
            &Zoom::default(),
            id,
            at,
            t,
        );
        self.send_input(&out.events);
    }

    /// A finger on the trackpad lifted.
    pub fn pad_ended(&self, id: u64, at: Point<f64>, t: f64) {
        let out = self.ivars().pad_touches.borrow_mut().ended(
            &mut self.ivars().pointer.borrow_mut(),
            &Zoom::default(),
            id,
            at,
            t,
        );
        self.send_input(&out.events);
    }

    /// UIKit cancelled a finger on the trackpad.
    pub fn pad_cancelled(&self, id: u64) {
        let out = self.ivars().pad_touches.borrow_mut().cancelled(&mut self.ivars().pointer.borrow_mut(), id);
        self.send_input(&out.events);
    }

    fn apply_touch(&self, out: TouchOutput) {
        self.send_input(&out.events);
        if let Some(change) = out.zoom {
            self.ivars().zoom.borrow_mut().apply(change);
            self.apply_zoom();
        }
    }

    /// Back to 1× (the session's desktop changed, or the user asked).
    pub fn reset_zoom(&self) {
        self.ivars().zoom.borrow_mut().reset();
        self.apply_zoom();
    }

    fn apply_zoom(&self) {
        let (a, tx, ty) = self.ivars().zoom.borrow().transform();
        self.setTransform(CGAffineTransform { a, b: 0.0, c: 0.0, d: a, tx, ty });
    }

    /// Touches on the picture. Indirect-pointer touches (iPad pointer clicks) press and drag
    /// where the pointer is, with the secondary button as a right click; fingers and the Pencil
    /// go through the touch state machine in screen points. Returns `false` when nothing was
    /// handled, so UIKit gets the touches.
    fn touches(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>, phase: TouchPhase) -> bool {
        let Some(touch) = touches.iter().find(|t| t.r#type() == UITouchType::IndirectPointer) else {
            return self.finger_touches(touches, phase);
        };
        let p = touch.locationInView(Some(self));
        self.pointer_hover(Point::new(p.x, p.y));
        match phase {
            TouchPhase::Began => {
                let secondary = event.is_some_and(|e| e.buttonMask().contains(UIEventButtonMask::Secondary));
                self.ivars().pressed_secondary.set(secondary);
                self.pointer_button(if secondary { MouseButton::Right } else { MouseButton::Left }, true);
            }
            TouchPhase::Moved => {}
            TouchPhase::Ended => {
                let right = self.ivars().pressed_secondary.replace(false);
                self.pointer_button(if right { MouseButton::Right } else { MouseButton::Left }, false);
            }
            TouchPhase::Cancelled => {
                let right = self.ivars().pressed_secondary.replace(false);
                self.pointer_button(if right { MouseButton::Right } else { MouseButton::Left }, false);
            }
        }
        true
    }

    fn finger_touches(&self, touches: &NSSet<UITouch>, phase: TouchPhase) -> bool {
        let screen = self.superview();
        let mut handled = false;
        for touch in touches.iter() {
            if !matches!(touch.r#type(), UITouchType::Direct | UITouchType::Pencil) {
                continue;
            }
            handled = true;
            // The touch object lives for the whole gesture, so its address identifies the finger.
            let id = Retained::as_ptr(&touch) as usize as u64;
            let p = touch.locationInView(screen.as_deref());
            let (at, t) = (Point::new(p.x, p.y), touch.timestamp());
            match phase {
                TouchPhase::Began => self.finger_began(id, at, t),
                TouchPhase::Moved => self.finger_moved(id, at, t),
                TouchPhase::Ended => self.finger_ended(id, at, t),
                TouchPhase::Cancelled => self.finger_cancelled(id),
            }
        }
        handled
    }

    /// Forwards input events to the handler (the IOS-6/IOS-7 event sources call this).
    pub fn send_input(&self, events: &[InputEvent]) {
        let handler = self.ivars().handler.borrow().clone();
        if let Some(h) = handler {
            h.input(events);
        }
    }
}

/// Whether the app runs on an iPhone-sized device (trackpad by default, local zoom; IOS-7).
pub fn is_phone(mtm: MainThreadMarker) -> bool {
    objc2_ui_kit::UIDevice::currentDevice(mtm).userInterfaceIdiom()
        == objc2_ui_kit::UIUserInterfaceIdiom::Phone
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

/// Instance variables of [`TrackpadView`].
pub struct PadIvars {
    target: objc2::rc::Weak<RemoteView>,
}

define_class!(
    /// The on-screen trackpad under the picture on an iPhone in portrait (IOS-8). Its fingers
    /// drive the picture's pointer as a trackpad, whatever the picture's own touch mode is.
    #[unsafe(super(UIView))]
    #[thread_kind = MainThreadOnly]
    #[name = "DriftTrackpadView"]
    #[ivars = PadIvars]
    pub struct TrackpadView;

    impl TrackpadView {
        #[unsafe(method(touchesBegan:withEvent:))]
        fn touches_began(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.pad_touches(touches, TouchPhase::Began);
        }

        #[unsafe(method(touchesMoved:withEvent:))]
        fn touches_moved(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.pad_touches(touches, TouchPhase::Moved);
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn touches_ended(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.pad_touches(touches, TouchPhase::Ended);
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn touches_cancelled(&self, touches: &NSSet<UITouch>, _event: Option<&UIEvent>) {
            self.pad_touches(touches, TouchPhase::Cancelled);
        }
    }
);

impl TrackpadView {
    /// A trackpad with `frame` driving `target`'s pointer.
    pub fn new(mtm: MainThreadMarker, frame: CGRect, target: &RemoteView) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PadIvars { target: objc2::rc::Weak::from(target) });
        // SAFETY: `initWithFrame:` is UIView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setMultipleTouchEnabled(true);
        this.setBackgroundColor(Some(&UIColor::secondarySystemBackgroundColor()));
        this.setIsAccessibilityElement(true, mtm);
        this.setAccessibilityLabel(Some(&objc2_foundation::NSString::from_str("Trackpad")), mtm);
        this
    }

    fn pad_touches(&self, touches: &NSSet<UITouch>, phase: TouchPhase) {
        for touch in touches.iter() {
            let id = Retained::as_ptr(&touch) as usize as u64;
            let p = touch.locationInView(Some(self));
            let (at, t) = (Point::new(p.x, p.y), touch.timestamp());
            match phase {
                TouchPhase::Began => self.finger_began(id, at, t),
                TouchPhase::Moved => self.finger_moved(id, at, t),
                TouchPhase::Ended => self.finger_ended(id, at, t),
                TouchPhase::Cancelled => {
                    if let Some(target) = self.ivars().target.load() {
                        target.pad_cancelled(id);
                    }
                }
            }
        }
    }

    /// A finger went down at `at` (the pad's points).
    pub fn finger_began(&self, id: u64, at: Point<f64>, t: f64) {
        if let Some(target) = self.ivars().target.load() {
            target.pad_began(id, at, t);
        }
    }

    /// A finger moved.
    pub fn finger_moved(&self, id: u64, at: Point<f64>, t: f64) {
        if let Some(target) = self.ivars().target.load() {
            target.pad_moved(id, at, t);
        }
    }

    /// A finger lifted.
    pub fn finger_ended(&self, id: u64, at: Point<f64>, t: f64) {
        if let Some(target) = self.ivars().target.load() {
            target.pad_ended(id, at, t);
        }
    }
}
