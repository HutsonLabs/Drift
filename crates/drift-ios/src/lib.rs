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
//! | [`switcher`] | the switcher button and its menu of windows | IOS-11 |
//! | [`view`] | `RemoteView`: `UIView` + `CAMetalLayer` under the `WKWebView` | IOS-4 |
//! | [`webview`] | the page's `WKWebView`: picture below it, frames, main-queue dispatch | IOS-5 |
//! | [`qr`] | the pairing QR scanner (AVFoundation camera, full-screen) | IOS-9 |
//! | [`paste`] | `UIPasteControl` over the picture: the explicit, prompt-free paste | IOS-10 |
//! | [`hardware`] | `GCKeyboard` keys and `GCMouse` deltas (under pointer lock) to the front session | IOS-6 |
//! | [`pointer_lock`] | `prefersPointerLocked` on tao's root view controller, per instance | IOS-6 |
//! | [`pacing`] | `CADisplayLink` asking for up to 120 Hz while a session is visible | IOS-4 |
//!
//! UIKit exists only on iOS, so the crate is empty on every other target. Every `unsafe` block
//! has a `// SAFETY:` comment.
#![cfg(target_os = "ios")]

pub mod hardware;
pub mod lifecycle;
pub mod pacing;
pub mod paste;
pub mod pointer_lock;
pub mod qr;
pub mod settings;
pub mod switcher;
pub mod view;
pub mod webview;

pub use drift_apple::{Keychain, KeychainError, PathMonitor, PathStatus, TriggerFeed};
pub use lifecycle::{AppObserver, ReconnectTriggers};
pub use pacing::FramePacing;
pub use view::{RemoteView, RemoteViewHandler};
