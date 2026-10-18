//! The app's page in Settings (IOS-3).
//!
//! iOS has no deep link to Privacy › Local Network; the Local Network switch for Drift sits on
//! the app's own page in Settings, which `UIApplicationOpenSettingsURLString` opens.

use objc2::MainThreadMarker;
use objc2_foundation::{NSDictionary, NSString, NSURL};
use objc2_ui_kit::{UIApplication, UIApplicationOpenSettingsURLString};

/// The URL of the app's page in Settings (`app-settings:`).
pub fn app_settings_url() -> String {
    // SAFETY: an immutable constant exported by UIKit.
    unsafe { UIApplicationOpenSettingsURLString }.to_string()
}

/// Opening Settings failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("iOS could not open {0}")]
pub struct OpenSettingsError(pub String);

/// Opens the app's page in Settings. Must run on the main thread.
pub fn open_app_settings(mtm: MainThreadMarker) -> Result<(), OpenSettingsError> {
    let url = app_settings_url();
    let nsurl = NSURL::URLWithString(&NSString::from_str(&url)).ok_or(OpenSettingsError(url))?;
    let app = UIApplication::sharedApplication(mtm);
    // SAFETY: a valid URL, an empty options dictionary of the declared key type, no completion.
    unsafe { app.openURL_options_completionHandler(&nsurl, &NSDictionary::new(), None) };
    Ok(())
}
