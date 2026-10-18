//! Start at login through `SMAppService.mainAppService` (task **HOST-9**). Humble object.
//!
//! The service only exists for an app bundle; run as a bare binary (tests, `cargo run`) it
//! reports [`LoginItem::Unavailable`].

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::NSError;

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

/// The login item's state (`SMAppService.Status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItem {
    /// Not registered.
    Off,
    /// Registered and enabled.
    On,
    /// Registered, waiting for the user in System Settings › General › Login Items.
    NeedsApproval,
    /// No service for this process (not running from a bundle).
    Unavailable,
}

fn service() -> Option<Retained<AnyObject>> {
    let class = AnyClass::get(c"SMAppService")?;
    // SAFETY: `+mainAppService` takes no arguments and returns the shared service object.
    unsafe { msg_send![class, mainAppService] }
}

/// The current state.
pub fn status() -> LoginItem {
    let Some(service) = service() else { return LoginItem::Unavailable };
    // SAFETY: `status` is an `NSInteger` property of SMAppService.
    let raw: isize = unsafe { msg_send![&*service, status] };
    match raw {
        0 => LoginItem::Off,
        1 => LoginItem::On,
        2 => LoginItem::NeedsApproval,
        _ => LoginItem::Unavailable,
    }
}

/// Registers (`on`) or unregisters the app as a login item.
pub fn set(on: bool) -> Result<(), String> {
    let service = service().ok_or("Start at Login needs the Drift Host app bundle")?;
    let mut error: *mut NSError = std::ptr::null_mut();
    // SAFETY: both methods take an `NSError **` out-parameter and return BOOL.
    let ok: bool = unsafe {
        if on {
            msg_send![&*service, registerAndReturnError: &mut error]
        } else {
            msg_send![&*service, unregisterAndReturnError: &mut error]
        }
    };
    if ok {
        return Ok(());
    }
    // SAFETY: on failure the method stores an autoreleased NSError or leaves null.
    let message = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
    Err(message.unwrap_or_else(|| "macOS refused the login item".to_owned()))
}
