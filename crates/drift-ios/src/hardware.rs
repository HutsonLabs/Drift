//! Hardware keyboard and mouse through GameController (task **IOS-6**).
//!
//! `GCKeyboard` reports every key as a HID usage, modifiers included, without going through
//! UIKit's text system; `GCMouse` reports raw deltas, which are the only pointer motion iPadOS
//! delivers while the pointer is locked. Both go to the *active* session view — the one whose
//! live picture is in front ([`set_active`]) — and nowhere else. Mouse deltas, buttons and
//! scroll are used only while the view's scene holds the pointer lock; otherwise UIKit's hover,
//! pointer touches and scroll recognizer on the view already carry them
//! ([`crate::view::RemoteView`]).
//!
//! The handlers run on GameController's handler queue, which is the main queue by default.

use std::cell::RefCell;
use std::ffi::c_float;
use std::ptr::NonNull;

use block2::RcBlock;
use drift_core::MouseButton;
use drift_input::ScrollDelta;
use objc2::MainThreadMarker;
use objc2::rc::{Retained, Weak};
use objc2::runtime::Bool;
use objc2_game_controller::{
    GCControllerButtonInput, GCControllerDirectionPad, GCKeyCode, GCKeyboard,
    GCKeyboardDidConnectNotification, GCKeyboardInput, GCMouse, GCMouseDidConnectNotification, GCMouseInput,
};

use crate::lifecycle::AppObserver;
use crate::view::RemoteView;

thread_local! {
    /// The session view whose picture is in front.
    static ACTIVE: RefCell<Option<Weak<RemoteView>>> = const { RefCell::new(None) };
}

/// Makes `view` the receiver of hardware input (or none). The previous view releases its held
/// keys and buttons, so nothing stays pressed on a session that went to the back.
pub fn set_active(view: Option<&RemoteView>) {
    let previous = ACTIVE.with(|a| a.replace(view.map(Weak::from)));
    if let Some(previous) = previous.and_then(|w| w.load())
        && view.is_none_or(|v| !std::ptr::eq(v, &*previous))
    {
        previous.release_all();
    }
}

/// The active view, if it is still alive.
pub fn active() -> Option<Retained<RemoteView>> {
    ACTIVE.with(|a| a.borrow().as_ref().and_then(Weak::load))
}

/// Whether the view's scene currently holds the pointer lock.
pub fn pointer_locked(view: &RemoteView) -> bool {
    view.window()
        .and_then(|w| w.windowScene())
        .and_then(|s| s.pointerLockState())
        .is_some_and(|state| state.isLocked())
}

/// Keeps the GameController hooks alive; dropping it stops listening for new devices.
#[derive(Debug)]
pub struct HardwareInput {
    _observers: Vec<AppObserver>,
}

impl HardwareInput {
    /// Hooks the keyboard and every mouse connected now and later.
    pub fn start(_mtm: MainThreadMarker) -> Self {
        hook_keyboard();
        hook_mice();
        // SAFETY: immutable constants exported by GameController.
        let (keyboard, mouse) = unsafe { (GCKeyboardDidConnectNotification, GCMouseDidConnectNotification) };
        Self {
            _observers: vec![
                AppObserver::observe(keyboard, hook_keyboard),
                AppObserver::observe(mouse, hook_mice),
            ],
        }
    }
}

fn hook_keyboard() {
    // SAFETY: class method returning the shared keyboard (or nil without one).
    let Some(input) = (unsafe { GCKeyboard::coalescedKeyboard() }).and_then(|k| unsafe { k.keyboardInput() })
    else {
        return;
    };
    let handler = RcBlock::new(
        |_input: NonNull<GCKeyboardInput>,
         _key: NonNull<GCControllerButtonInput>,
         code: GCKeyCode,
         pressed: Bool| {
            if let (Some(view), Ok(usage)) = (active(), u16::try_from(code)) {
                view.hid_key(usage, pressed.as_bool());
            }
        },
    );
    // SAFETY: GameController copies the block; it captures nothing.
    unsafe { input.setKeyChangedHandler(RcBlock::as_ptr(&handler)) };
}

fn hook_mice() {
    // SAFETY: class method returning the connected mice.
    for mouse in unsafe { GCMouse::mice() }.iter() {
        // SAFETY: plain getter.
        if let Some(input) = unsafe { mouse.mouseInput() } {
            hook_mouse(&input);
        }
    }
}

/// Runs `f` on the active view while its pointer is locked.
fn with_locked(f: impl FnOnce(&RemoteView)) {
    if let Some(view) = active()
        && pointer_locked(&view)
    {
        f(&view);
    }
}

fn hook_mouse(input: &GCMouseInput) {
    // GameController's y axis points up; view points grow downwards.
    let moved = RcBlock::new(|_input: NonNull<GCMouseInput>, dx: c_float, dy: c_float| {
        with_locked(|v| v.pointer_delta(f64::from(dx), -f64::from(dy)));
    });
    // SAFETY: GameController copies the block; it captures nothing.
    unsafe { input.setMouseMovedHandler(RcBlock::as_ptr(&moved)) };
    // SAFETY: plain getters.
    let buttons = unsafe {
        [
            (Some(input.leftButton()), MouseButton::Left),
            (input.rightButton(), MouseButton::Right),
            (input.middleButton(), MouseButton::Middle),
        ]
    };
    for (button, which) in buttons {
        let Some(button) = button else { continue };
        let pressed =
            RcBlock::new(move |_b: NonNull<GCControllerButtonInput>, _value: c_float, down: Bool| {
                with_locked(|v| v.pointer_button(which, down.as_bool()));
            });
        // SAFETY: GameController copies the block; it captures only a `Copy` enum.
        unsafe { button.setPressedChangedHandler(RcBlock::as_ptr(&pressed)) };
    }
    let scrolled = RcBlock::new(|_pad: NonNull<GCControllerDirectionPad>, x: c_float, y: c_float| {
        with_locked(|v| v.pointer_scroll(ScrollDelta::Precise { dx: f64::from(x), dy: f64::from(y) }, false));
    });
    // SAFETY: plain getter; GameController copies the block, which captures nothing.
    unsafe { input.scroll().setValueChangedHandler(RcBlock::as_ptr(&scrolled)) };
}
