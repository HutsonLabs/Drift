//! # drift-apple
//!
//! Platform services shared by Drift's macOS (`drift-macos`, AppKit) and iOS (`drift-ios`,
//! UIKit) layers (ADR `IOS-0`, decision 8; task IOS-3). Nothing here touches AppKit or UIKit.
//!
//! | Module | Area | Task |
//! |---|---|---|
//! | [`audio_out`] | remote sound: `AVAudioEngine` player with the 200 ms latency cap | HOST-11 |
//! | [`cursor`] | pure fast-path pointer decode + cache, shared by `NSCursor` and the iPad pointer | M2-5, IOS-5 |
//! | [`keychain`] | Keychain password adapter (login keychain on macOS, data-protection keychain on iOS) | M3-2, IOS-3 |
//! | [`network`] | `NWPathMonitor` and the merged reconnect-trigger feed | M7-2, IOS-3 |
//! | [`lnp`] | Local Network Privacy (errno 65) detection | M1-1, M9-3, IOS-3 |
//!
//! Every `unsafe` block has a `// SAFETY:` comment.
#![cfg(target_vendor = "apple")]

pub mod audio_out;
pub mod cursor;
pub mod keychain;
pub mod lnp;
pub mod network;

pub use cursor::{CursorImage, CursorShape, PointerDecoder, PointerError, PointerEvent};
pub use keychain::{Keychain, KeychainError};
pub use network::{PathMonitor, PathStatus, TriggerFeed};
