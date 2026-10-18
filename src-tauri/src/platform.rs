//! Thin platform glue used by commands (humble object; no logic).

use crate::profiles::CommandError;

/// Opens `url` with Launch Services (`x-apple.systempreferences:` URLs open System Settings).
#[cfg(target_os = "macos")]
pub fn open_url(url: &str) -> Result<(), CommandError> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    let Some(nsurl) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return Err(CommandError::Platform { message: format!("invalid URL {url}") });
    };
    if NSWorkspace::sharedWorkspace().openURL(&nsurl) {
        Ok(())
    } else {
        Err(CommandError::Platform { message: format!("macOS could not open {url}") })
    }
}

/// Opens the place where the user grants Local Network access: System Settings › Privacy &
/// Security › Local Network on macOS, the app's own page in Settings on iOS (ADR IOS-3).
pub fn open_local_network_settings() -> Result<(), CommandError> {
    #[cfg(target_os = "macos")]
    {
        open_url(drift_core::messages::LOCAL_NETWORK_SETTINGS_URL)
    }
    #[cfg(target_os = "ios")]
    {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return Err(CommandError::Platform {
                message: "Settings can only be opened from the main thread".into(),
            });
        };
        drift_ios::settings::open_app_settings(mtm)
            .map_err(|e| CommandError::Platform { message: e.to_string() })
    }
}
