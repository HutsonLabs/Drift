//! # drift-inject
//!
//! Drift Host's input injection (task **HOST-6**; ADR `HOST-6-input-injection`): the inverse of
//! the client's input translation.
//!
//! | Module | What | |
//! |---|---|---|
//! | [`plan`] | RDP fast-path input → [`plan::InjectEvent`]s: keys back to `kVK_*`, pointer to display points, clicks, wheel | pure |
//! | [`gesture`] | EXT-2: precise scrolls with phases; swipes and pinches as Mission Control, Spaces and zoom shortcuts | pure |
//! | `cg` | builds and posts `CGEvent`s at the HID tap; Accessibility check | macOS, humble |

pub mod gesture;
pub mod plan;

#[cfg(target_os = "macos")]
pub mod cg;
