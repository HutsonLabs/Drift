//! USB HID keyboard usages → set-1 scancodes (task **IOS-6**).
//!
//! Stub (Red).

use drift_core::{CmdAs, InputEvent};

use crate::keymap::Scancode;

/// Maps a HID keyboard-page usage to its set-1 scancode.
pub fn scancode_for_hid(_usage: u16, _cmd_as: CmdAs) -> Option<Scancode> {
    None
}

/// Held-key state for a HID keyboard.
#[derive(Debug, Clone, Default)]
pub struct HidKeyboard {
    _cmd_as: CmdAs,
}

impl HidKeyboard {
    /// A keyboard sending Command as `cmd_as`.
    pub fn new(cmd_as: CmdAs) -> Self {
        Self { _cmd_as: cmd_as }
    }

    /// One key change.
    pub fn key(&mut self, _usage: u16, _down: bool) -> Vec<InputEvent> {
        Vec::new()
    }

    /// Releases every held key.
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        Vec::new()
    }
}

#[cfg(test)]
#[path = "tests/hid.rs"]
mod tests;
