//! Pointer lock for the session view (task **IOS-6**; IOS-1 spike notes).
//!
//! iPadOS locks the pointer for a scene whose root view controller answers YES to
//! `prefersPointerLocked`. The root view controller is tao's `TaoUIViewController`, a class tao
//! builds at runtime that does not override the property, so Drift adds the method to that class
//! once and answers per instance: only a view controller Drift asked to lock says YES. iPadOS
//! grants the lock only to a full-screen, foreground scene, applies it a moment later, and drops
//! it when the scene leaves the foreground.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::CStr;

use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{MainThreadMarker, msg_send, sel};

thread_local! {
    /// View controllers (by address) that should lock the pointer.
    static LOCKED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    /// Classes that already got the method.
    static PATCHED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
}

/// The added `-prefersPointerLocked`.
extern "C-unwind" fn prefers_pointer_locked(this: *mut AnyObject, _cmd: Sel) -> Bool {
    Bool::new(LOCKED.with(|l| l.borrow().contains(&(this as usize))))
}

/// Locks (or unlocks) the pointer for view controller `vc` and asks UIKit to re-read it.
/// Returns `false` when the class already implements `prefersPointerLocked` itself and could
/// not be patched (its own answer then stands).
pub fn set_locked(_mtm: MainThreadMarker, vc: &AnyObject, locked: bool) -> bool {
    let class: &AnyClass = vc.class();
    if !patch(class) {
        return false;
    }
    let key = std::ptr::from_ref(vc) as usize;
    let changed = LOCKED.with(|l| {
        let mut l = l.borrow_mut();
        if locked { l.insert(key) } else { l.remove(&key) }
    });
    if changed {
        // SAFETY: a UIViewController method (iOS 14+) with no arguments, on the main thread.
        let () = unsafe { msg_send![vc, setNeedsUpdateOfPrefersPointerLocked] };
    }
    true
}

/// Adds `prefersPointerLocked` to `class` once.
fn patch(class: &AnyClass) -> bool {
    let key = std::ptr::from_ref(class) as usize;
    if PATCHED.with(|p| p.borrow().contains(&key)) {
        return true;
    }
    let types: &CStr = c"B@:";
    // SAFETY: `prefers_pointer_locked` matches the BOOL-returning, argument-less selector
    // (encoding `B@:` on arm64); `class_addMethod` copies the types string. It refuses (returns
    // NO) when the class itself already defines the method, which leaves that class untouched.
    let added = unsafe {
        let imp: Imp = std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool, Imp>(
            prefers_pointer_locked,
        );
        objc2::ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            sel!(prefersPointerLocked),
            imp,
            types.as_ptr(),
        )
    };
    if added.as_bool() {
        PATCHED.with(|p| p.borrow_mut().insert(key));
    }
    added.as_bool()
}

/// Locks (or unlocks) the pointer for the scene showing `view`, through its window's root view
/// controller. `false` when the view is not in a window yet or the class could not be patched.
pub fn set_locked_for_view(mtm: MainThreadMarker, view: &objc2_ui_kit::UIView, locked: bool) -> bool {
    let Some(vc) = view.window().and_then(|w| w.rootViewController()) else { return false };
    set_locked(mtm, &vc, locked)
}
