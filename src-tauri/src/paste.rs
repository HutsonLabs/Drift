//! The explicit paste on iOS (task **IOS-10**, pure).
//!
//! iOS never lets Drift read the pasteboard on its own, so local → remote clipboard sync only
//! happens when the user pastes into the session (the `UIPasteControl` over the picture,
//! `drift_ios::paste`). One paste does what the user meant: it becomes that session's local
//! clipboard (the actor advertises it to the remote, M5-2) and is then pasted on the remote with
//! Ctrl+V. The remote applies the format list before the key, because both travel in order on
//! the same connection.

use drift_clipboard::ClipboardContents;
use drift_core::{ClipboardPrefs, InputEvent};
use drift_rdp::SessionCommand;

/// Set-1 scancodes of Left Ctrl and V.
const CTRL: u8 = 0x1D;
const V: u8 = 0x2F;

/// The commands for one paste of `contents` into a session whose clipboard preference is
/// `prefs`; nothing when the paste is empty or the clipboard is off.
pub fn paste_commands(contents: ClipboardContents, prefs: ClipboardPrefs) -> Vec<SessionCommand> {
    if prefs == ClipboardPrefs::Off || contents.is_empty() {
        return Vec::new();
    }
    let key = |scancode, down| SessionCommand::Input(InputEvent::Key { scancode, extended: false, down });
    vec![
        SessionCommand::ClipboardLocalChanged(contents),
        key(CTRL, true),
        key(V, true),
        key(V, false),
        key(CTRL, false),
    ]
}
