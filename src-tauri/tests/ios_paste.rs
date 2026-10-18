//! IOS-10 Red: an explicit paste on iOS becomes the local clipboard of that one session, then
//! a Ctrl+V on the remote, so the user's paste lands where they asked (`drift_app::paste`).
#![allow(clippy::unwrap_used)]

use drift_app::paste::paste_commands;
use drift_clipboard::{ClipboardContents, ClipboardItem};
use drift_core::{ClipboardPrefs, InputEvent};
use drift_rdp::SessionCommand;

fn text(t: &str) -> ClipboardContents {
    ClipboardContents { items: vec![ClipboardItem::Text(t.into())] }
}

const CTRL: u8 = 0x1D;
const V: u8 = 0x2F;

fn key(scancode: u8, down: bool) -> SessionCommand {
    SessionCommand::Input(InputEvent::Key { scancode, extended: false, down })
}

#[test]
fn paste_sends_the_contents_then_ctrl_v() {
    let cmds = paste_commands(text("fake"), ClipboardPrefs::Text);
    assert_eq!(
        cmds,
        vec![
            SessionCommand::ClipboardLocalChanged(text("fake")),
            key(CTRL, true),
            key(V, true),
            key(V, false),
            key(CTRL, false),
        ]
    );
}

#[test]
fn nothing_when_the_paste_is_empty_or_the_clipboard_is_off() {
    assert!(paste_commands(ClipboardContents::empty(), ClipboardPrefs::TextAndImages).is_empty());
    assert!(paste_commands(text("fake"), ClipboardPrefs::Off).is_empty());
}
