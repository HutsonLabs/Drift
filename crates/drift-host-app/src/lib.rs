//! # drift-host-app
//!
//! Drift Host's menu-bar app (task **HOST-9**, ADR `HOST-9-menu-bar`): it runs the host
//! (`drift-host`), shows its status and the paired devices, checks the permissions the host
//! needs and explains them in plain words, and can start at login. A separate bundle from Drift
//! (`com.hutsonlabs.drift.host`), Developer ID only, not sandboxed (ADR `IOS-0` decision 9).
//!
//! | Module | What |
//! |---|---|
//! | [`model`] | pure view model: status line, menu, icon state, permission guidance |
//! | `status_item` | the `NSStatusItem` and `NSMenu` (humble object) |
//! | `permissions` | Screen Recording / Accessibility checks and prompts, Settings links |
//! | `login` | Start at Login through `SMAppService` |

#[cfg(target_os = "macos")]
pub mod login;
pub mod model;
#[cfg(target_os = "macos")]
pub mod permissions;
#[cfg(target_os = "macos")]
pub mod status_item;

/// The app's bundle identifier.
pub const BUNDLE_ID: &str = "com.hutsonlabs.drift.host";

/// The executable inside `Drift Host.app/Contents/MacOS`.
pub const EXECUTABLE: &str = "drift-host-app";
