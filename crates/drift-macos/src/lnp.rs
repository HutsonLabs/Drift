//! Local Network Privacy on macOS: the System Settings link (plan §1.8, tasks M1-1 / M9-3).
//!
//! Detection is shared with iOS and lives in `drift-apple`; it is re-exported here.

pub use drift_apple::lnp::{EHOSTUNREACH, is_local_network_denied, local_network_settings_url};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

/// Opening System Settings failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("macOS could not open {0}")]
pub struct OpenSettingsError(pub String);

/// Opens System Settings › Privacy & Security › Local Network.
pub fn open_local_network_settings() -> Result<(), OpenSettingsError> {
    let url = local_network_settings_url();
    let err = || OpenSettingsError(url.into());
    let nsurl = NSURL::URLWithString(&NSString::from_str(url)).ok_or_else(err)?;
    if NSWorkspace::sharedWorkspace().openURL(&nsurl) { Ok(()) } else { Err(err()) }
}
