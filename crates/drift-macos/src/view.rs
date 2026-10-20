//! `RemoteView`: the session's NSView (tasks **M1-6**, **M2-2**, **M2-4**, **M2-5**; plan §1.8).
//!
//! A layer-hosting `NSView` subclass (`define_class!`) whose layer is a `CAMetalLayer` rendered
//! by `drift-render` on the session render thread. The view:
//!
//! * keeps `contentsScale`/`drawableSize` equal to bounds × backing scale in `setFrameSize:` and
//!   `viewDidChangeBackingProperties`, and reports the [`ViewGeometry`] to its handler;
//! * is flipped (top-left origin) and accepts first responder / first mouse;
//! * translates `keyDown:`/`keyUp:`/`flagsChanged:`, mouse buttons, moves, drags and precise
//!   `scrollWheel:` through the pure [`InputController`] into [`InputEvent`]s;
//! * claims every Command/Control combo in `performKeyEquivalent:` except Drift's allow-list
//!   (M2-4), so Cmd+C or Ctrl+Tab reach the remote instead of the Edit menu or AppKit;
//! * implements `NSTextInputClient` so that, in "Type using Mac layout" mode, dead keys and
//!   IMEs compose locally and the committed text is sent as Unicode events (M2-2);
//! * shows the remote pointer as its cursor (M2-5).
//!
//! All logic is in [`InputController`]; this file only moves values between AppKit and it.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use drift_core::{DesktopSize, InputEvent, KeyboardPrefs, Point, Size, ViewGeometry};
use drift_input::{KeyDown, ModifierFlags, ScaleMode, ScrollConfig, ScrollDelta};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, Sel};
use objc2::{AnyThread as _, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibilityImageRole, NSApplication, NSAutoresizingMaskOptions, NSCursor, NSEvent, NSEventMask,
    NSEventModifierFlags, NSEventPhase, NSEventType, NSTextInputClient, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSWindowDidBecomeKeyNotification, NSWindowDidResignKeyNotification,
};
use objc2_core_foundation::CGSize;
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound, NSNotification, NSNotificationCenter,
    NSPoint, NSRange, NSRect, NSSize, NSString, NSUInteger,
};
use objc2_quartz_core::CAMetalLayer;

use crate::cursor::CursorShape;
use crate::cursor_ns;
use crate::input::{InputController, KeyEquivalent, mouse_button};
use crate::keyboard_type;

/// Receives what a [`RemoteView`] produces. Called on the main thread; implementations should
/// only forward (e.g. `SessionHandle::send`, which never blocks).
pub trait RemoteViewHandler {
    /// Input events to send, in order.
    fn input(&self, events: &[InputEvent]);
    /// The view's size or backing scale changed (drives Display Control, M4-2).
    fn geometry_changed(&self, geometry: ViewGeometry) {
        let _ = geometry;
    }
    /// The view gained or lost key focus.
    fn focus_changed(&self, focused: bool) {
        let _ = focused;
    }
}

/// Instance variables of [`RemoteView`].
pub struct Ivars {
    layer: Retained<CAMetalLayer>,
    input: RefCell<InputController>,
    handler: RefCell<Option<Rc<dyn RemoteViewHandler>>>,
    cursor: RefCell<Retained<NSCursor>>,
    /// IME composition in progress (`setMarkedText:`), as UTF-16 length.
    marked_len: Cell<usize>,
    /// The key event currently inside `interpretKeyEvents:` (`kVK`, raw modifier flags), for
    /// the `doCommandBySelector:` scancode fallback.
    interpreting: Cell<Option<(u16, u64)>>,
    geometry: Cell<ViewGeometry>,
    focused: Cell<bool>,
    /// What VoiceOver announces for the picture (M9-4); the app keeps it in step with the
    /// session through [`RemoteView::set_accessibility_label`].
    accessibility_label: RefCell<Retained<NSString>>,
}

/// VoiceOver's role description for the live picture.
pub const ROLE_DESCRIPTION: &str = "remote desktop";

/// VoiceOver's help text for the live picture.
pub const ACCESSIBILITY_HELP: &str =
    "Keyboard, pointer and scroll input go to the remote computer while this picture has focus.";

/// The label shown before a session names itself.
pub const DEFAULT_ACCESSIBILITY_LABEL: &str = "Remote desktop";

define_class!(
    /// Layer-hosting `NSView` with a `CAMetalLayer`, capturing keyboard, pointer and scroll
    /// input for one remote session.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "DriftRemoteView"]
    #[ivars = Ivars]
    pub struct RemoteView;

    impl RemoteView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            // SAFETY: calling the superclass implementation of the overridden method.
            let ok: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            if ok {
                self.focus_gained();
            }
            ok
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            // SAFETY: calling the superclass implementation of the overridden method.
            let ok: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            if ok {
                self.focus_lost();
            }
            ok
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: calling the superclass implementation of the overridden method.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.sync_drawable_size();
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn view_did_change_backing_properties(&self) {
            // SAFETY: calling the superclass implementation of the overridden method.
            let _: () = unsafe { msg_send![super(self), viewDidChangeBackingProperties] };
            self.sync_drawable_size();
        }

        #[unsafe(method(viewWillMoveToWindow:))]
        fn view_will_move_to_window(&self, _window: Option<&AnyObject>) {
            // SAFETY: removing ourselves as an observer is always valid.
            unsafe { NSNotificationCenter::defaultCenter().removeObserver(self) };
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            self.observe_window_key_state();
            self.sync_drawable_size();
        }

        #[unsafe(method(driftWindowDidResignKey:))]
        fn window_did_resign_key(&self, _note: &NSNotification) {
            if self.is_first_responder() {
                self.focus_lost();
            }
        }

        #[unsafe(method(driftWindowDidBecomeKey:))]
        fn window_did_become_key(&self, _note: &NSNotification) {
            if self.is_first_responder() {
                self.focus_gained();
            }
        }

        // --- keyboard ----------------------------------------------------------------------

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.handle_key_down(event);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            self.handle_key_up(event);
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            let events = self.ivars().input.borrow_mut().flags_changed(event.keyCode(), flags(event.modifierFlags()));
            self.emit(&events);
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            self.handle_key_equivalent(event)
        }

        // --- pointer -----------------------------------------------------------------------

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.handle_move(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.handle_move(event);
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            self.handle_move(event);
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            self.handle_move(event);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.handle_button(event, true);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.handle_button(event, false);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.handle_button(event, true);
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            self.handle_button(event, false);
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            self.handle_button(event, true);
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            self.handle_button(event, false);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let (dx, dy) = (event.scrollingDeltaX(), event.scrollingDeltaY());
            let delta = if event.hasPreciseScrollingDeltas() {
                ScrollDelta::Precise { dx, dy }
            } else {
                ScrollDelta::Lines { dx, dy }
            };
            if event.hasPreciseScrollingDeltas() {
                // EXT-2: precise scrolls carry their phase; GNOME gets the wheel units.
                let phase = crate::input::scroll_phase(event.phase().0 as u64);
                let momentum = crate::input::momentum_phase(event.momentumPhase().0 as u64);
                let scroll = self.ivars().input.borrow_mut().precise_scroll(dx, dy, phase, momentum);
                self.emit(&[scroll]);
                return;
            }
            let began = event.phase().contains(NSEventPhase::Began);
            let events = self.ivars().input.borrow_mut().scroll(delta, began);
            self.emit(&events);
        }

        // --- cursor ------------------------------------------------------------------------

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let cursor = self.ivars().cursor.borrow().clone();
            self.addCursorRect_cursor(self.visibleRect(), &cursor);
        }

        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, _event: &NSEvent) {
            self.ivars().cursor.borrow().set();
        }

        // --- accessibility (M9-4) ----------------------------------------------------------
        //
        // Without these a layer-hosting NSView reads as an unnamed "group" and VoiceOver users
        // cannot tell what the window is showing. The picture is one element with an image
        // role; its contents live on the remote computer, so there is nothing to walk into.

        #[unsafe(method(isAccessibilityElement))]
        fn is_accessibility_element(&self) -> bool {
            true
        }

        #[unsafe(method_id(accessibilityRole))]
        fn accessibility_role(&self) -> Option<Retained<NSString>> {
            // SAFETY: reading an AppKit string constant.
            Some(unsafe { NSAccessibilityImageRole }.to_owned())
        }

        #[unsafe(method_id(accessibilityRoleDescription))]
        fn accessibility_role_description(&self) -> Option<Retained<NSString>> {
            Some(NSString::from_str(ROLE_DESCRIPTION))
        }

        #[unsafe(method_id(accessibilityLabel))]
        fn accessibility_label(&self) -> Option<Retained<NSString>> {
            Some(self.ivars().accessibility_label.borrow().clone())
        }

        #[unsafe(method_id(accessibilityHelp))]
        fn accessibility_help(&self) -> Option<Retained<NSString>> {
            Some(NSString::from_str(ACCESSIBILITY_HELP))
        }

        #[unsafe(method_id(accessibilityChildren))]
        fn accessibility_children(&self) -> Option<Retained<NSArray>> {
            // The picture's contents live on the remote computer: nothing to walk into.
            None
        }
    }

    unsafe impl NSObjectProtocol for RemoteView {}

    unsafe impl NSTextInputClient for RemoteView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text_replacement_range(&self, string: &AnyObject, _replacement: NSRange) {
            self.ivars().marked_len.set(0);
            let text = plain_string(string);
            let events = self.ivars().input.borrow_mut().insert_text(&text);
            self.emit(&events);
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command_by_selector(&self, _selector: Sel) {
            // A non-text key inside `interpretKeyEvents:` (Return, arrows, Delete …): send it
            // as a scancode. Outside a key event there is nothing to send.
            if let Some((kvk, raw)) = self.ivars().interpreting.get() {
                let events = self.ivars().input.borrow_mut().key_down_scancode(kvk, ModifierFlags(raw));
                self.emit(&events);
            }
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, string: &AnyObject, _selected: NSRange, _replacement: NSRange) {
            let len = plain_string(string).encode_utf16().count();
            self.ivars().marked_len.set(len);
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            self.ivars().marked_len.set(0);
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            NSRange::new(NSNotFound as NSUInteger, 0)
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            match self.ivars().marked_len.get() {
                0 => NSRange::new(NSNotFound as NSUInteger, 0),
                n => NSRange::new(0, n),
            }
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            self.ivars().marked_len.get() > 0
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring(&self, _range: NSRange, _actual: *mut NSRange) -> Option<Retained<NSAttributedString>> {
            None
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes_for_marked_text(&self) -> Retained<NSArray<NSAttributedStringKey>> {
            NSArray::new()
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect_for_character_range(&self, _range: NSRange, _actual: *mut NSRange) -> NSRect {
            // Place IME candidate windows at the bottom-left of the view, in screen coordinates.
            let local = NSRect::new(NSPoint::new(0.0, self.bounds().size.height), NSSize::new(1.0, 1.0));
            let in_window = self.convertRect_toView(local, None);
            self.window().map(|w| w.convertRectToScreen(in_window)).unwrap_or(in_window)
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index_for_point(&self, _point: NSPoint) -> NSUInteger {
            NSNotFound as NSUInteger
        }
    }
);

/// `NSEventModifierFlags` → the raw flags `drift-input` works with.
fn flags(f: NSEventModifierFlags) -> ModifierFlags {
    ModifierFlags(f.0 as u64)
}

/// The text of an `NSString` or `NSAttributedString` argument.
fn plain_string(obj: &AnyObject) -> String {
    if let Some(s) = obj.downcast_ref::<NSString>() {
        s.to_string()
    } else if let Some(a) = obj.downcast_ref::<NSAttributedString>() {
        a.string().to_string()
    } else {
        String::new()
    }
}

impl RemoteView {
    /// Creates the view with `frame` (points) for a profile's keyboard preferences.
    pub fn new(mtm: MainThreadMarker, frame: NSRect, prefs: KeyboardPrefs) -> Retained<Self> {
        let layer = CAMetalLayer::new();
        let input = InputController::new(prefs, keyboard_type::detect(), ScrollConfig::default());
        let this = Self::alloc(mtm).set_ivars(Ivars {
            layer: layer.clone(),
            input: RefCell::new(input),
            handler: RefCell::new(None),
            cursor: RefCell::new(NSCursor::arrowCursor()),
            marked_len: Cell::new(0),
            interpreting: Cell::new(None),
            geometry: Cell::new(ViewGeometry { points: Size::new(0.0, 0.0), backing_scale: 1.0 }),
            focused: Cell::new(false),
            accessibility_label: RefCell::new(NSString::from_str(DEFAULT_ACCESSIBILITY_LABEL)),
        });
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        // Layer-hosting view: set the layer first, then `wantsLayer` (plan §1.8).
        this.setLayer(Some(&layer));
        this.setWantsLayer(true);
        layer.setOpaque(true);
        this.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let options = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::CursorUpdate
            | NSTrackingAreaOptions::ActiveInKeyWindow
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: the owner (the view) outlives its own tracking area; no user info.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                options,
                Some(&this),
                None,
            )
        };
        this.addTrackingArea(&area);
        this.sync_drawable_size();
        this
    }

    /// The Metal layer (hand it to `drift_render::LayerTarget`).
    pub fn metal_layer(&self) -> &CAMetalLayer {
        &self.ivars().layer
    }

    /// Sets the receiver of input, geometry and focus notifications.
    pub fn set_handler(&self, handler: Rc<dyn RemoteViewHandler>) {
        *self.ivars().handler.borrow_mut() = Some(handler);
    }

    /// Changes the keyboard preferences.
    pub fn set_keyboard_prefs(&self, prefs: KeyboardPrefs) {
        self.ivars().input.borrow_mut().set_keyboard_prefs(prefs);
    }

    /// Changes the scroll conversion settings.
    pub fn set_scroll_config(&self, config: ScrollConfig) {
        let mut input = self.ivars().input.borrow_mut();
        input.set_scroll_config(config);
    }

    /// Attaches the remote desktop size and placement (pointer mapping).
    pub fn set_desktop(&self, desktop: DesktopSize, mode: ScaleMode) {
        self.ivars().input.borrow_mut().set_desktop(desktop, mode);
    }

    /// Sets what VoiceOver announces for the picture (M9-4), e.g.
    /// `"Homelab — remote desktop, 1280 by 800 pixels"`.
    pub fn set_accessibility_label(&self, label: &str) {
        *self.ivars().accessibility_label.borrow_mut() = NSString::from_str(label);
    }

    /// Detaches the desktop (pointer events are dropped).
    pub fn clear_desktop(&self) {
        self.ivars().input.borrow_mut().clear_desktop();
    }

    /// The view's current geometry.
    pub fn geometry(&self) -> ViewGeometry {
        self.ivars().geometry.get()
    }

    /// Recomputes `contentsScale` and `drawableSize` from the bounds and the window's backing
    /// scale factor, and reports a changed geometry to the handler.
    pub fn sync_drawable_size(&self) {
        let layer = &self.ivars().layer;
        let scale = self.window().map(|w| w.backingScaleFactor()).unwrap_or_else(|| layer.contentsScale());
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let size = self.bounds().size;
        layer.setContentsScale(scale);
        layer.setDrawableSize(CGSize {
            width: (size.width * scale).round(),
            height: (size.height * scale).round(),
        });
        let geometry = ViewGeometry { points: Size::new(size.width, size.height), backing_scale: scale };
        if geometry != self.ivars().geometry.replace(geometry) {
            self.ivars().input.borrow_mut().set_view_geometry(geometry);
            if let Some(h) = self.handler() {
                h.geometry_changed(geometry);
            }
        }
    }

    /// Shows a remote pointer shape (desktop scale `scale_percent`).
    pub fn set_cursor_shape(&self, shape: &CursorShape, scale_percent: u32) {
        let cursor = cursor_ns::cursor_for(self.mtm(), shape, scale_percent);
        *self.ivars().cursor.borrow_mut() = cursor.clone();
        if let Some(window) = self.window() {
            window.invalidateCursorRectsForView(self);
            let p = self.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
            let b = self.bounds();
            let inside = p.x >= 0.0 && p.y >= 0.0 && p.x < b.size.width && p.y < b.size.height;
            if inside && window.isKeyWindow() {
                cursor.set();
            }
        }
    }

    /// The cursor currently shown over the view.
    pub fn current_cursor(&self) -> Retained<NSCursor> {
        self.ivars().cursor.borrow().clone()
    }

    /// The "Send Ctrl+Alt+Del" menu item.
    pub fn send_ctrl_alt_del(&self) {
        let events = self.ivars().input.borrow_mut().ctrl_alt_del();
        self.emit(&events);
    }

    /// Releases every key and button the remote considers held (window hidden, disconnect).
    pub fn release_all(&self) {
        let events = self.ivars().input.borrow_mut().focus_lost();
        self.emit(&events);
    }

    // --- internals -----------------------------------------------------------------------

    fn handler(&self) -> Option<Rc<dyn RemoteViewHandler>> {
        self.ivars().handler.borrow().clone()
    }

    /// Sends events to the handler. No `RefCell` borrow is held here, so the handler may call
    /// back into the view.
    fn emit(&self, events: &[InputEvent]) {
        if events.is_empty() {
            return;
        }
        if let Some(h) = self.handler() {
            h.input(events);
        }
    }

    fn is_first_responder(&self) -> bool {
        self.window().and_then(|w| w.firstResponder()).is_some_and(|r| {
            std::ptr::eq(Retained::as_ptr(&r).cast::<()>(), (self as *const Self).cast::<()>())
        })
    }

    fn focus_gained(&self) {
        let events = self.ivars().input.borrow_mut().focus_gained(flags(NSEvent::modifierFlags_class()));
        self.emit(&events);
        if !self.ivars().focused.replace(true)
            && let Some(h) = self.handler()
        {
            h.focus_changed(true);
        }
    }

    fn focus_lost(&self) {
        self.ivars().marked_len.set(0);
        let events = self.ivars().input.borrow_mut().focus_lost();
        self.emit(&events);
        if self.ivars().focused.replace(false)
            && let Some(h) = self.handler()
        {
            h.focus_changed(false);
        }
    }

    fn observe_window_key_state(&self) {
        let Some(window) = self.window() else { return };
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: selector-based observers are weakly referenced by the notification center
        // (macOS ≥ 10.11) and removed in `viewWillMoveToWindow:`; both selectors are defined
        // above with the `(NSNotification*)` signature.
        unsafe {
            center.addObserver_selector_name_object(
                self,
                sel!(driftWindowDidResignKey:),
                Some(NSWindowDidResignKeyNotification),
                Some(&window),
            );
            center.addObserver_selector_name_object(
                self,
                sel!(driftWindowDidBecomeKey:),
                Some(NSWindowDidBecomeKeyNotification),
                Some(&window),
            );
        }
    }

    fn handle_key_down(&self, event: &NSEvent) {
        let kvk = event.keyCode();
        let f = flags(event.modifierFlags());
        let composing = self.ivars().marked_len.get() > 0;
        let routed = self.ivars().input.borrow_mut().key_down(kvk, f, event.isARepeat(), composing);
        match routed {
            KeyDown::Send(events) => self.emit(&events),
            KeyDown::InterpretText => {
                self.ivars().interpreting.set(Some((kvk, f.0)));
                self.interpretKeyEvents(&NSArray::from_slice(&[event]));
                self.ivars().interpreting.set(None);
            }
        }
    }

    fn handle_key_up(&self, event: &NSEvent) {
        let events = self.ivars().input.borrow_mut().key_up(event.keyCode());
        self.emit(&events);
    }

    fn handle_key_equivalent(&self, event: &NSEvent) -> bool {
        if event.r#type() != NSEventType::KeyDown {
            return false;
        }
        let chars = event.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default();
        let first_responder = self.is_first_responder();
        let decision = self.ivars().input.borrow_mut().key_equivalent(
            first_responder,
            event.keyCode(),
            &chars,
            flags(event.modifierFlags()),
            event.isARepeat(),
        );
        match decision {
            KeyEquivalent::Pass | KeyEquivalent::Menu(_) => false,
            KeyEquivalent::Claim(events) => {
                self.emit(&events);
                true
            }
        }
    }

    fn view_point(&self, event: &NSEvent) -> Point<f64> {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        Point::new(p.x, p.y)
    }

    fn handle_move(&self, event: &NSEvent) {
        let p = self.view_point(event);
        let events = self.ivars().input.borrow_mut().mouse_move(p);
        self.emit(&events);
    }

    fn handle_button(&self, event: &NSEvent, down: bool) {
        let Some(button) = mouse_button(event.buttonNumber() as i64) else { return };
        let p = self.view_point(event);
        let events = self.ivars().input.borrow_mut().mouse_button(button, down, p);
        self.emit(&events);
    }
}

/// Installs (once per process) a local key-up monitor that forwards key-ups AppKit swallows
/// while Command is held to the key window's `RemoteView`, so keys pressed in a claimed
/// Command combo are released on the remote.
pub fn install_key_up_monitor(mtm: MainThreadMarker) {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit passes a valid event for the duration of the handler.
        let ev = unsafe { event.as_ref() };
        if ev.modifierFlags().contains(NSEventModifierFlags::Command)
            && let Some(window) = NSApplication::sharedApplication(mtm).keyWindow()
            && let Some(responder) = window.firstResponder()
            && let Some(view) = responder.downcast_ref::<RemoteView>()
        {
            view.handle_key_up(ev);
        }
        event.as_ptr()
    });
    // SAFETY: the handler returns the event unchanged; the monitor lives for the process
    // (the returned token is intentionally leaked).
    let token = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyUp, &block) };
    std::mem::forget(token);
}
