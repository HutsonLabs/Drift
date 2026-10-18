//! Reads and requests the permissions Drift Host needs (task **HOST-9**). Humble object; the
//! wording and order live in [`crate::model`].

use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString};
use objc2_foundation::{NSString, NSURL};

use crate::model::{Grant, Permissions};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: Option<&CFDictionary>) -> bool;
    static kAXTrustedCheckOptionPrompt: &'static CFString;
}

/// Screen Recording and Accessibility as macOS reports them now. Local Network has no query, so
/// it is `Unknown` until a device has reached the host (`reached`).
pub fn current(reached_from_network: bool) -> Permissions {
    Permissions {
        screen_recording: grant(objc2_core_graphics::CGPreflightScreenCaptureAccess()),
        // SAFETY: a plain query without arguments.
        accessibility: grant(unsafe { AXIsProcessTrusted() }),
        local_network: if reached_from_network { Grant::Granted } else { Grant::Unknown },
    }
}

fn grant(granted: bool) -> Grant {
    if granted { Grant::Granted } else { Grant::Missing }
}

/// Asks macOS to show its Screen Recording prompt (once per app; afterwards only Settings helps).
pub fn request_screen_recording() -> bool {
    objc2_core_graphics::CGRequestScreenCaptureAccess()
}

/// Asks macOS to show its Accessibility prompt, which also adds Drift Host to the list in
/// Settings.
pub fn request_accessibility() -> bool {
    // SAFETY: an immutable constant exported by ApplicationServices.
    let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
    let options: CFRetained<CFDictionary<CFString, CFBoolean>> =
        CFDictionary::from_slices(&[key], &[CFBoolean::new(true)]);
    // SAFETY: a dictionary with the documented key and a CFBoolean value.
    unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) }
}

/// Opens a System Settings pane (`x-apple.systempreferences:` URL).
pub fn open_settings(url: &str) -> bool {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else { return false };
    objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url)
}
