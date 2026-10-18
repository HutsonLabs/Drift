//! USB HID keyboard usages → set-1 scancodes (task **IOS-6**).
//!
//! iPadOS reports hardware keys as HID keyboard-page usages (`GCKeyboard`'s `GCKeyCode`,
//! `UIKey.keyCode`). They are positional, like `kVK_*` and PC scancodes, so the mapping is a
//! fixed table. It agrees with the macOS table ([`crate::keymap`]) on every key both know, which
//! the tests check key by key, so a key means the same on an iPad and a Mac.
//!
//! Apple ISO keyboards swap two usages in hardware (the key left of `1` reports Non-US `\`
//! `0x64`, the key right of left Shift reports Grave `0x35`); macOS undoes that from the keyboard
//! type, which iPadOS does not expose. This table stays the HID standard (ADR `IOS-6`).

use std::collections::BTreeSet;

use drift_core::{CmdAs, InputEvent};

use crate::keymap::Scancode;

/// Maps a HID keyboard-page usage to its set-1 scancode. The GUI (Command) keys follow
/// `cmd_as`, as on macOS. `None` for usages with no PC key (Pause's E1 sequence, Power, the
/// reserved and error usages).
pub fn scancode_for_hid(usage: u16, cmd_as: CmdAs) -> Option<Scancode> {
    let n = Scancode::new;
    let e = Scancode::ext;
    Some(match usage {
        // Letters a..z, in HID order, and the digit row 1..0.
        0x04..=0x1D => n(LETTERS[usize::from(usage - 0x04)]),
        0x1E..=0x26 => n(0x02 + u8::try_from(usage - 0x1E).ok()?),
        0x27 => n(0x0B),
        0x28 => n(0x1C),
        0x29 => n(0x01),
        0x2A => n(0x0E),
        0x2B => n(0x0F),
        0x2C => n(0x39),
        0x2D => n(0x0C),
        0x2E => n(0x0D),
        0x2F => n(0x1A),
        0x30 => n(0x1B),
        0x31 => n(0x2B),
        // Non-US `#` sits where US keyboards have backslash.
        0x32 => n(0x2B),
        0x33 => n(0x27),
        0x34 => n(0x28),
        0x35 => n(0x29),
        0x36 => n(0x33),
        0x37 => n(0x34),
        0x38 => n(0x35),
        0x39 => n(0x3A),
        // F1..F10, F11, F12.
        0x3A..=0x43 => n(0x3B + u8::try_from(usage - 0x3A).ok()?),
        0x44 => n(0x57),
        0x45 => n(0x58),
        0x46 => e(0x37),
        0x47 => n(0x46),
        0x49 => e(0x52),
        0x4A => e(0x47),
        0x4B => e(0x49),
        0x4C => e(0x53),
        0x4D => e(0x4F),
        0x4E => e(0x51),
        0x4F => e(0x4D),
        0x50 => e(0x4B),
        0x51 => e(0x50),
        0x52 => e(0x48),
        0x53 => n(0x45),
        0x54 => e(0x35),
        0x55 => n(0x37),
        0x56 => n(0x4A),
        0x57 => n(0x4E),
        0x58 => e(0x1C),
        0x59 => n(0x4F),
        0x5A => n(0x50),
        0x5B => n(0x51),
        0x5C => n(0x4B),
        0x5D => n(0x4C),
        0x5E => n(0x4D),
        0x5F => n(0x47),
        0x60 => n(0x48),
        0x61 => n(0x49),
        0x62 => n(0x52),
        0x63 => n(0x53),
        // Non-US `\` (the ISO 102nd key).
        0x64 => n(0x56),
        0x65 => e(0x5D),
        0x67 => n(0x59),
        // F13..F20 (0x64..0x6B), F21..F23 (0x6C..0x6E), F24 (0x76).
        0x68..=0x70 => n(0x64 + u8::try_from(usage - 0x68).ok()?),
        0x71 => n(0x6D),
        0x72 => n(0x6E),
        0x73 => n(0x76),
        0x7F => e(0x20),
        0x80 => e(0x30),
        0x81 => e(0x2E),
        0x85 => n(0x7E),
        0x87 => n(0x73),
        0x88 => n(0x70),
        0x89 => n(0x7D),
        0x8A => n(0x79),
        0x8B => n(0x7B),
        0x90 => n(0x72),
        0x91 => n(0x71),
        0xE0 => n(0x1D),
        0xE1 => n(0x2A),
        0xE2 => n(0x38),
        0xE3 => match cmd_as {
            CmdAs::Super => e(0x5B),
            CmdAs::Ctrl => n(0x1D),
        },
        0xE4 => e(0x1D),
        0xE5 => n(0x36),
        0xE6 => e(0x38),
        0xE7 => match cmd_as {
            CmdAs::Super => e(0x5C),
            CmdAs::Ctrl => e(0x1D),
        },
        _ => return None,
    })
}

/// Set-1 make codes of HID usages `0x04` (a) to `0x1D` (z).
const LETTERS: [u8; 26] = [
    0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10,
    0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
];

/// Held-key state for a HID keyboard: every press is sent once, every release only for a held
/// key, and [`HidKeyboard::release_all`] lifts whatever is still down (focus loss, background).
#[derive(Debug, Clone, Default)]
pub struct HidKeyboard {
    cmd_as: CmdAs,
    held: BTreeSet<Scancode>,
}

impl HidKeyboard {
    /// A keyboard sending Command as `cmd_as`.
    pub fn new(cmd_as: CmdAs) -> Self {
        Self { cmd_as, held: BTreeSet::new() }
    }

    /// Changes where Command goes (profile edited while connected).
    pub fn set_cmd_as(&mut self, cmd_as: CmdAs) {
        self.cmd_as = cmd_as;
    }

    /// One key change from the hardware keyboard.
    pub fn key(&mut self, usage: u16, down: bool) -> Vec<InputEvent> {
        let Some(code) = scancode_for_hid(usage, self.cmd_as) else { return Vec::new() };
        let changed = if down { self.held.insert(code) } else { self.held.remove(&code) };
        if changed { vec![code.event(down)] } else { Vec::new() }
    }

    /// Releases every held key, in scancode order.
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.held).into_iter().map(|c| c.event(false)).collect()
    }
}

#[cfg(test)]
#[path = "tests/hid.rs"]
mod tests;
