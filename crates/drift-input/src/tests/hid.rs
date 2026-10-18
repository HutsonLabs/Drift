use super::*;
use crate::keymap::{KeyboardType, kvk, scancode_for};
use drift_core::CmdAs;

/// HID usage ↔ `kVK_*` for every key both tables know (Apple's own correspondence). The HID
/// table must agree with the M2-1 table on all of them, so a key means the same on iPad and Mac.
fn hid_to_kvk() -> Vec<(u16, u16)> {
    use kvk::*;
    let letters = [
        ANSI_A, ANSI_B, ANSI_C, ANSI_D, ANSI_E, ANSI_F, ANSI_G, ANSI_H, ANSI_I, ANSI_J, ANSI_K, ANSI_L,
        ANSI_M, ANSI_N, ANSI_O, ANSI_P, ANSI_Q, ANSI_R, ANSI_S, ANSI_T, ANSI_U, ANSI_V, ANSI_W, ANSI_X,
        ANSI_Y, ANSI_Z,
    ];
    let digits = [ANSI_1, ANSI_2, ANSI_3, ANSI_4, ANSI_5, ANSI_6, ANSI_7, ANSI_8, ANSI_9, ANSI_0];
    let mut v: Vec<(u16, u16)> = letters.iter().enumerate().map(|(i, k)| (0x04 + i as u16, *k)).collect();
    v.extend(digits.iter().enumerate().map(|(i, k)| (0x1E + i as u16, *k)));
    v.extend([
        (0x28, RETURN),
        (0x29, ESCAPE),
        (0x2A, DELETE),
        (0x2B, TAB),
        (0x2C, SPACE),
        (0x2D, ANSI_MINUS),
        (0x2E, ANSI_EQUAL),
        (0x2F, ANSI_LEFT_BRACKET),
        (0x30, ANSI_RIGHT_BRACKET),
        (0x31, ANSI_BACKSLASH),
        (0x33, ANSI_SEMICOLON),
        (0x34, ANSI_QUOTE),
        (0x35, ANSI_GRAVE),
        (0x36, ANSI_COMMA),
        (0x37, ANSI_PERIOD),
        (0x38, ANSI_SLASH),
        (0x39, CAPS_LOCK),
        (0x3A, F1),
        (0x3B, F2),
        (0x3C, F3),
        (0x3D, F4),
        (0x3E, F5),
        (0x3F, F6),
        (0x40, F7),
        (0x41, F8),
        (0x42, F9),
        (0x43, F10),
        (0x44, F11),
        (0x45, F12),
        (0x49, HELP),
        (0x4A, HOME),
        (0x4B, PAGE_UP),
        (0x4C, FORWARD_DELETE),
        (0x4D, END),
        (0x4E, PAGE_DOWN),
        (0x4F, RIGHT_ARROW),
        (0x50, LEFT_ARROW),
        (0x51, DOWN_ARROW),
        (0x52, UP_ARROW),
        (0x54, ANSI_KEYPAD_DIVIDE),
        (0x55, ANSI_KEYPAD_MULTIPLY),
        (0x56, ANSI_KEYPAD_MINUS),
        (0x57, ANSI_KEYPAD_PLUS),
        (0x58, ANSI_KEYPAD_ENTER),
        (0x59, ANSI_KEYPAD_1),
        (0x5A, ANSI_KEYPAD_2),
        (0x5B, ANSI_KEYPAD_3),
        (0x5C, ANSI_KEYPAD_4),
        (0x5D, ANSI_KEYPAD_5),
        (0x5E, ANSI_KEYPAD_6),
        (0x5F, ANSI_KEYPAD_7),
        (0x60, ANSI_KEYPAD_8),
        (0x61, ANSI_KEYPAD_9),
        (0x62, ANSI_KEYPAD_0),
        (0x63, ANSI_KEYPAD_DECIMAL),
        (0x65, CONTEXTUAL_MENU),
        (0x67, ANSI_KEYPAD_EQUALS),
        (0x68, F13),
        (0x69, F14),
        (0x6A, F15),
        (0x6B, F16),
        (0x6C, F17),
        (0x6D, F18),
        (0x6E, F19),
        (0x6F, F20),
        (0x7F, MUTE),
        (0x80, VOLUME_UP),
        (0x81, VOLUME_DOWN),
        (0x85, JIS_KEYPAD_COMMA),
        (0x87, JIS_UNDERSCORE),
        (0x89, JIS_YEN),
        (0x90, JIS_KANA),
        (0x91, JIS_EISU),
        (0xE0, CONTROL),
        (0xE1, SHIFT),
        (0xE2, OPTION),
        (0xE3, COMMAND),
        (0xE4, RIGHT_CONTROL),
        (0xE5, RIGHT_SHIFT),
        (0xE6, RIGHT_OPTION),
        (0xE7, RIGHT_COMMAND),
    ]);
    v
}

#[test]
fn hid_table_agrees_with_the_mac_table_for_every_shared_key() {
    for cmd_as in [CmdAs::Super, CmdAs::Ctrl] {
        for (usage, code) in hid_to_kvk() {
            assert_eq!(
                scancode_for_hid(usage, cmd_as),
                scancode_for(code, KeyboardType::Ansi, cmd_as),
                "HID usage {usage:#04x} (kVK {code:#04x}, {cmd_as:?})"
            );
        }
    }
}

#[test]
fn pc_keys_the_mac_table_lacks() {
    let n = Scancode::new;
    let e = Scancode::ext;
    assert_eq!(scancode_for_hid(0x32, CmdAs::Super), Some(n(0x2B)), "Non-US # sits where backslash is");
    assert_eq!(scancode_for_hid(0x64, CmdAs::Super), Some(n(0x56)), "Non-US \\ (ISO 102nd key)");
    assert_eq!(scancode_for_hid(0x46, CmdAs::Super), Some(e(0x37)), "Print Screen");
    assert_eq!(scancode_for_hid(0x47, CmdAs::Super), Some(n(0x46)), "Scroll Lock");
    assert_eq!(scancode_for_hid(0x53, CmdAs::Super), Some(n(0x45)), "Num Lock");
    assert_eq!(scancode_for_hid(0x88, CmdAs::Super), Some(n(0x70)), "International2 (katakana/hiragana)");
    assert_eq!(scancode_for_hid(0x8A, CmdAs::Super), Some(n(0x79)), "International4 (henkan)");
    assert_eq!(scancode_for_hid(0x8B, CmdAs::Super), Some(n(0x7B)), "International5 (muhenkan)");
}

#[test]
fn reserved_and_unmappable_usages_are_none() {
    for usage in [0x00, 0x01, 0x02, 0x03, 0x48, 0x66, 0xA5, 0xDF, 0xE8, 0xFFFF] {
        assert_eq!(scancode_for_hid(usage, CmdAs::Super), None, "usage {usage:#x}");
    }
}

#[test]
fn keyboard_sends_press_and_release_once() {
    let mut kb = HidKeyboard::new(CmdAs::Super);
    let a = Scancode::new(0x1E);
    assert_eq!(kb.key(0x04, true), vec![a.event(true)]);
    assert_eq!(kb.key(0x04, true), vec![], "a repeated press is not sent again");
    assert_eq!(kb.key(0x04, false), vec![a.event(false)]);
    assert_eq!(kb.key(0x04, false), vec![], "a release of a key not held is dropped");
    assert_eq!(kb.key(0x01, true), vec![], "unmapped usages are dropped");
}

#[test]
fn command_follows_cmd_as() {
    let mut kb = HidKeyboard::new(CmdAs::Ctrl);
    assert_eq!(kb.key(0xE3, true), vec![Scancode::new(0x1D).event(true)]);
    assert_eq!(kb.key(0xE7, true), vec![Scancode::ext(0x1D).event(true)]);
}

#[test]
fn release_all_lifts_every_held_key_in_order() {
    let mut kb = HidKeyboard::new(CmdAs::Super);
    kb.key(0xE1, true);
    kb.key(0x04, true);
    let released = kb.release_all();
    assert_eq!(released, vec![Scancode::new(0x1E).event(false), Scancode::new(0x2A).event(false)]);
    assert_eq!(kb.release_all(), vec![]);
}

/// HOST-6: a key typed on an iPad reaches a Mac host as the same key: HID usage → scancode →
/// `kVK_*` agrees with Apple's HID ↔ `kVK_*` correspondence.
#[test]
fn hid_to_scancode_to_kvk_matches_apple() {
    for (usage, kvk) in hid_to_kvk() {
        let sc = scancode_for_hid(usage, CmdAs::Super).unwrap_or_else(|| panic!("usage {usage:#x}"));
        assert_eq!(crate::keymap::kvk_for(sc, KeyboardType::Ansi), Some(kvk), "usage {usage:#x}");
    }
}
