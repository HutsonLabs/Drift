//! Thin AppKit glue used by commands (humble object; no logic).

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

use crate::profiles::CommandError;

/// Opens `url` with Launch Services (`x-apple.systempreferences:` URLs open System Settings).
pub fn open_url(url: &str) -> Result<(), CommandError> {
    let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return Err(CommandError::Platform { message: format!("invalid URL {url}") });
    };
    if NSWorkspace::sharedWorkspace().openURL(&nsurl) {
        Ok(())
    } else {
        Err(CommandError::Platform { message: format!("macOS could not open {url}") })
    }
}
