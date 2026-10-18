//! # drift-input
//!
//! Pure input translation from macOS events to [`drift_core::InputEvent`]s. No AppKit
//! types appear here: `drift-macos` extracts plain values (key codes, modifier flags,
//! deltas, points) and calls into this crate.
//!
//! | Module | Task |
//! |---|---|
//! | [`keymap`] | M2-1: `kVK_*` → set-1 scancode (+extended) for ANSI/ISO/JIS |
//! | [`modifiers`] | M2-1: `modifierFlags` → held left/right modifier keys |
//! | [`keyboard`] | M2-1/M2-2: stateful translator (diffing, `CmdAs`, focus loss, Caps Lock, text) |
//! | [`unicode`] | M2-2: "Type using Mac layout" routing |
//! | [`scroll`] | M2-3: precise-delta → fractional wheel-unit accumulator |
//! | [`coords`] | M2-3: `view_to_desktop` (1:1, fit/letterbox, Retina) |
//! | [`shortcuts`] | M2-4: `performKeyEquivalent:` allow-list |
//! | [`hid`] | IOS-6: HID keyboard usages (iPad `GCKeyboard`) → set-1 scancodes, held keys |
//! | [`pointer`] | IOS-6: absolute and relative (pointer lock, trackpad) pointer with buttons and scroll |

pub mod coords;
pub mod hid;
pub mod keyboard;
pub mod keymap;
pub mod modifiers;
pub mod pointer;
pub mod scroll;
pub mod shortcuts;
pub mod unicode;

pub use coords::{ScaleMode, Viewport, view_to_desktop};
pub use hid::{HidKeyboard, scancode_for_hid};
pub use keyboard::{KeyDown, Keyboard};
pub use keymap::{KeyboardType, Scancode};
pub use modifiers::ModifierFlags;
pub use pointer::Pointer;
pub use scroll::{ScrollAccumulator, ScrollConfig, ScrollDelta};
pub use shortcuts::{MenuShortcut, menu_shortcut};
pub use unicode::KeyRoute;
