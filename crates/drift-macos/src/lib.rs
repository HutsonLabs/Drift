//! # drift-macos
//!
//! Humble-object FFI glue for AppKit and system services (plan §2):
//!
//! | Module | Area | Task |
//! |---|---|---|
//! | [`view`] | `RemoteView`: layer-hosting NSView + CAMetalLayer, input capture, IME | M1-6, M2-2, M2-4 |
//! | [`input`] | pure per-view input state behind `RemoteView` (keys, pointer, scroll, key equivalents) | M2-2, M2-4 |
//! | [`webview`] | insert the RemoteView below the WKWebView; focus switching | M1-6 |
//! | [`cursor`] | pure fast-path pointer decode + cache; [`cursor_ns`]: `NSCursor` | M2-5 |
//! | [`keychain`] | Keychain password adapter (re-exported from `drift-apple`) | M3-2, IOS-3 |
//! | [`window`] | occlusion / key-window observer, main-queue dispatch, no tabbing | M6-3, UI-tabs, UI-windows |
//! | [`dock`] | the Dock menu (`applicationDockMenu:` on the app delegate) | UI-windows |
//! | [`alert`] | close and quit confirmations (`NSAlert` sheet / app-modal) | UI-windows |
//! | [`network`] | `NSWorkspace` wake + `drift-apple`'s `NWPathMonitor` → `TriggerMerger` | M7-2 |
//! | [`lnp`] | System Settings link; errno 65 detection from `drift-apple` | M1-1, M9-3 |
//! | [`keyboard_type`] | ANSI/ISO/JIS detection of the local keyboard | M2-1 |
//!
//! Decision logic lives in pure code (`drift-input`, `drift-core`, and the pure modules
//! [`input`], [`cursor`]); the AppKit-facing code only extracts plain
//! values and applies results. Every `unsafe` block has a `// SAFETY:` comment.
//!
//! AppKit exists only on macOS, so the crate is empty on every other target (IOS-2).
#![cfg(target_os = "macos")]

pub mod alert;
pub mod cursor;
pub mod cursor_ns;
pub mod dock;
pub mod input;
pub mod keyboard_type;
pub use drift_apple::keychain;
pub mod lnp;
pub mod network;
pub mod view;
pub mod webview;
pub mod window;

#[cfg(feature = "tauri")]
pub mod tauri_glue;

pub use cursor::{CursorImage, CursorShape, PointerDecoder, PointerError, PointerEvent};
pub use input::{InputController, KeyEquivalent};
pub use keychain::{Keychain, KeychainError};
pub use network::{PathMonitor, PathStatus, ReconnectTriggers, TriggerFeed, WakeObserver};
pub use view::{RemoteView, RemoteViewHandler};
pub use window::{WindowEvent, WindowObserver, dispatch_main};
