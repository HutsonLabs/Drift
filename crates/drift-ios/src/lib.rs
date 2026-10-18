//! # drift-ios
//!
//! Humble-object FFI glue for UIKit (ADR `IOS-0`, decision 8), the iOS counterpart of
//! `drift-macos`. Shared Apple services (Keychain, `NWPathMonitor`, Local Network detection)
//! live in `drift-apple`.
//!
//! | Module | Area | Task |
//! |---|---|---|
//! | [`lifecycle`] | foreground observer and the reconnect triggers it feeds | IOS-3 |
//! | [`settings`] | the app's page in Settings (Local Network permission) | IOS-3 |
//! | [`view`] | `RemoteView`: `UIView` + `CAMetalLayer` under the `WKWebView` | IOS-4 |
//! | [`pacing`] | `CADisplayLink` asking for up to 120 Hz while a session is visible | IOS-4 |
//!
//! UIKit exists only on iOS, so the crate is empty on every other target. Every `unsafe` block
//! has a `// SAFETY:` comment.
#![cfg(target_os = "ios")]

pub mod lifecycle;
pub mod pacing;
pub mod settings;
pub mod view;

pub use drift_apple::{Keychain, KeychainError, PathMonitor, PathStatus, TriggerFeed};
pub use lifecycle::{ForegroundObserver, ReconnectTriggers};
pub use pacing::FramePacing;
pub use view::{RemoteView, RemoteViewHandler};
