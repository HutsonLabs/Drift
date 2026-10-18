//! Who is at the console (task **HOST-13**): `SCDynamicStoreCopyConsoleUser`. Humble object.
//!
//! The login window reports the user `loginwindow` (uid 0), or no user at all.

use std::ffi::{c_char, c_void};

use crate::broker::ConsoleState;

#[link(name = "SystemConfiguration", kind = "framework")]
unsafe extern "C" {
    fn SCDynamicStoreCopyConsoleUser(store: *const c_void, uid: *mut u32, gid: *mut u32) -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringGetCString(s: *const c_void, buf: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFRelease(cf: *const c_void);
}

const UTF8: u32 = 0x0800_0100;

/// The console's current state; [`ConsoleState::Unknown`] if it cannot be read.
pub fn console_state() -> ConsoleState {
    let (mut uid, mut gid) = (0_u32, 0_u32);
    // SAFETY: a null store means "a temporary session"; uid/gid are valid out-pointers.
    let name = unsafe { SCDynamicStoreCopyConsoleUser(std::ptr::null(), &mut uid, &mut gid) };
    if name.is_null() {
        // No console user: the login window (or very early boot).
        return ConsoleState::LoginWindow;
    }
    let mut buf = [0 as c_char; 256];
    // SAFETY: `name` is a CFString we own (+1); the buffer is 256 bytes.
    let ok = unsafe { CFStringGetCString(name, buf.as_mut_ptr(), buf.len() as isize, UTF8) } != 0;
    // SAFETY: releasing the +1 reference returned above.
    unsafe { CFRelease(name) };
    if !ok {
        return ConsoleState::Unknown;
    }
    // SAFETY: CFStringGetCString wrote a NUL-terminated string into `buf`.
    let name = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
    classify(&name, uid)
}

/// The state for a console user name and uid.
pub fn classify(name: &str, uid: u32) -> ConsoleState {
    if name.is_empty() || name == "loginwindow" || uid == 0 {
        ConsoleState::LoginWindow
    } else {
        ConsoleState::User { uid, name: name.to_owned() }
    }
}
