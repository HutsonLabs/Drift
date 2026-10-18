use super::*;
use std::collections::BTreeSet;

const N: fn(u8) -> Option<Scancode> = |c| Some(Scancode::new(c));
const E: fn(u8) -> Option<Scancode> = |c| Some(Scancode::ext(c));

/// Expected ANSI mapping (CmdAs::Super) for every `kVK_*` constant.
fn ansi_table() -> Vec<(u16, Option<Scancode>)> {
    use kvk::*;
    vec![
        (ANSI_A, N(0x1E)),
        (ANSI_S, N(0x1F)),
        (ANSI_D, N(0x20)),
        (ANSI_F, N(0x21)),
        (ANSI_H, N(0x23)),
        (ANSI_G, N(0x22)),
        (ANSI_Z, N(0x2C)),
        (ANSI_X, N(0x2D)),
        (ANSI_C, N(0x2E)),
        (ANSI_V, N(0x2F)),
        (ANSI_B, N(0x30)),
        (ANSI_Q, N(0x10)),
        (ANSI_W, N(0x11)),
        (ANSI_E, N(0x12)),
        (ANSI_R, N(0x13)),
        (ANSI_Y, N(0x15)),
        (ANSI_T, N(0x14)),
        (ANSI_1, N(0x02)),
        (ANSI_2, N(0x03)),
        (ANSI_3, N(0x04)),
        (ANSI_4, N(0x05)),
        (ANSI_6, N(0x07)),
        (ANSI_5, N(0x06)),
        (ANSI_EQUAL, N(0x0D)),
        (ANSI_9, N(0x0A)),
        (ANSI_7, N(0x08)),
        (ANSI_MINUS, N(0x0C)),
        (ANSI_8, N(0x09)),
        (ANSI_0, N(0x0B)),
        (ANSI_RIGHT_BRACKET, N(0x1B)),
        (ANSI_O, N(0x18)),
        (ANSI_U, N(0x16)),
        (ANSI_LEFT_BRACKET, N(0x1A)),
        (ANSI_I, N(0x17)),
        (ANSI_P, N(0x19)),
        (ANSI_L, N(0x26)),
        (ANSI_J, N(0x24)),
        (ANSI_QUOTE, N(0x28)),
        (ANSI_K, N(0x25)),
        (ANSI_SEMICOLON, N(0x27)),
        (ANSI_BACKSLASH, N(0x2B)),
        (ANSI_COMMA, N(0x33)),
        (ANSI_SLASH, N(0x35)),
        (ANSI_N, N(0x31)),
        (ANSI_M, N(0x32)),
        (ANSI_PERIOD, N(0x34)),
        (ANSI_GRAVE, N(0x29)),
        (ANSI_KEYPAD_DECIMAL, N(0x53)),
        (ANSI_KEYPAD_MULTIPLY, N(0x37)),
        (ANSI_KEYPAD_PLUS, N(0x4E)),
        (ANSI_KEYPAD_CLEAR, None),
        (ANSI_KEYPAD_DIVIDE, E(0x35)),
        (ANSI_KEYPAD_ENTER, E(0x1C)),
        (ANSI_KEYPAD_MINUS, N(0x4A)),
        (ANSI_KEYPAD_EQUALS, N(0x59)),
        (ANSI_KEYPAD_0, N(0x52)),
        (ANSI_KEYPAD_1, N(0x4F)),
        (ANSI_KEYPAD_2, N(0x50)),
        (ANSI_KEYPAD_3, N(0x51)),
        (ANSI_KEYPAD_4, N(0x4B)),
        (ANSI_KEYPAD_5, N(0x4C)),
        (ANSI_KEYPAD_6, N(0x4D)),
        (ANSI_KEYPAD_7, N(0x47)),
        (ANSI_KEYPAD_8, N(0x48)),
        (ANSI_KEYPAD_9, N(0x49)),
        (RETURN, N(0x1C)),
        (TAB, N(0x0F)),
        (SPACE, N(0x39)),
        (DELETE, N(0x0E)),
        (ESCAPE, N(0x01)),
        (RIGHT_COMMAND, E(0x5C)),
        (COMMAND, E(0x5B)),
        (SHIFT, N(0x2A)),
        (CAPS_LOCK, N(0x3A)),
        (OPTION, N(0x38)),
        (CONTROL, N(0x1D)),
        (RIGHT_SHIFT, N(0x36)),
        (RIGHT_OPTION, E(0x38)),
        (RIGHT_CONTROL, E(0x1D)),
        (FUNCTION, None),
        (F17, N(0x68)),
        (VOLUME_UP, E(0x30)),
        (VOLUME_DOWN, E(0x2E)),
        (MUTE, E(0x20)),
        (F18, N(0x69)),
        (F19, N(0x6A)),
        (F20, N(0x6B)),
        (F5, N(0x3F)),
        (F6, N(0x40)),
        (F7, N(0x41)),
        (F3, N(0x3D)),
        (F8, N(0x42)),
        (F9, N(0x43)),
        (F11, N(0x57)),
        (F13, N(0x64)),
        (F16, N(0x67)),
        (F14, N(0x65)),
        (F10, N(0x44)),
        (CONTEXTUAL_MENU, E(0x5D)),
        (F12, N(0x58)),
        (F15, N(0x66)),
        (HELP, E(0x52)),
        (HOME, E(0x47)),
        (PAGE_UP, E(0x49)),
        (FORWARD_DELETE, E(0x53)),
        (F4, N(0x3E)),
        (END, E(0x4F)),
        (F2, N(0x3C)),
        (PAGE_DOWN, E(0x51)),
        (F1, N(0x3B)),
        (LEFT_ARROW, E(0x4B)),
        (RIGHT_ARROW, E(0x4D)),
        (DOWN_ARROW, E(0x50)),
        (UP_ARROW, E(0x48)),
        (ISO_SECTION, N(0x56)),
        (JIS_YEN, N(0x7D)),
        (JIS_UNDERSCORE, N(0x73)),
        (JIS_KEYPAD_COMMA, N(0x7E)),
        (JIS_EISU, N(0x71)),
        (JIS_KANA, N(0x72)),
    ]
}

#[test]
fn all_kvk_lists_every_constant_once() {
    let codes: BTreeSet<u16> = ALL_KVK.iter().map(|(_, c)| *c).collect();
    assert_eq!(codes.len(), ALL_KVK.len(), "duplicate kVK code");
    let names: BTreeSet<&str> = ALL_KVK.iter().map(|(n, _)| *n).collect();
    assert_eq!(names.len(), ALL_KVK.len(), "duplicate kVK name");
    // HIToolbox Events.h (macOS 27 SDK) defines exactly 120 kVK_* key codes (incl. kVK_ContextualMenu).
    assert_eq!(ALL_KVK.len(), 120);
    let table: BTreeSet<u16> = ansi_table().iter().map(|(c, _)| *c).collect();
    assert_eq!(codes, table, "expected table must cover exactly ALL_KVK");
}

#[test]
fn ansi_table_over_every_kvk_constant() {
    for (code, expected) in ansi_table() {
        let name = ALL_KVK.iter().find(|(_, c)| *c == code).map_or("?", |(n, _)| *n);
        assert_eq!(scancode_for(code, KeyboardType::Ansi, CmdAs::Super), expected, "{name} (0x{code:02X})");
    }
}

#[test]
fn iso_swaps_section_and_grave_back_to_physical_positions() {
    for (code, expected) in ansi_table() {
        let expected = match code {
            // key left of `1` (reported as kVK_ISO_Section on ISO) → PC `~` position
            kvk::ISO_SECTION => N(0x29),
            // key right of left Shift (reported as kVK_ANSI_Grave on ISO) → PC 102nd key
            kvk::ANSI_GRAVE => N(0x56),
            _ => expected,
        };
        assert_eq!(scancode_for(code, KeyboardType::Iso, CmdAs::Super), expected, "0x{code:02X}");
    }
}

#[test]
fn jis_matches_ansi_table() {
    for (code, expected) in ansi_table() {
        assert_eq!(scancode_for(code, KeyboardType::Jis, CmdAs::Super), expected, "0x{code:02X}");
    }
}

#[test]
fn cmd_as_ctrl_maps_command_keys_to_control() {
    assert_eq!(scancode_for(kvk::COMMAND, KeyboardType::Ansi, CmdAs::Ctrl), N(0x1D));
    assert_eq!(scancode_for(kvk::RIGHT_COMMAND, KeyboardType::Ansi, CmdAs::Ctrl), E(0x1D));
    // everything else is unaffected
    assert_eq!(scancode_for(kvk::ANSI_A, KeyboardType::Ansi, CmdAs::Ctrl), N(0x1E));
    assert_eq!(scancode_for(kvk::CONTROL, KeyboardType::Ansi, CmdAs::Ctrl), N(0x1D));
}

#[test]
fn unknown_codes_are_unmapped() {
    for code in [0x0C0_u16, 0x7F, 0xFFFF, 0x34, 0x42, 0x44, 0x46, 0x4D, 0x6C, 0x70] {
        assert_eq!(scancode_for(code, KeyboardType::Ansi, CmdAs::Super), None, "0x{code:02X}");
    }
}

#[test]
fn keyboard_type_from_layout_type() {
    assert_eq!(KeyboardType::from_layout_type(KeyboardType::LAYOUT_ANSI), KeyboardType::Ansi);
    assert_eq!(KeyboardType::from_layout_type(KeyboardType::LAYOUT_ISO), KeyboardType::Iso);
    assert_eq!(KeyboardType::from_layout_type(KeyboardType::LAYOUT_JIS), KeyboardType::Jis);
    assert_eq!(KeyboardType::from_layout_type(0), KeyboardType::Ansi);
    assert_eq!(KeyboardType::LAYOUT_JIS, 0x4A49_5320);
}

#[test]
fn modifier_and_text_key_classification() {
    use kvk::*;
    for m in [
        SHIFT,
        RIGHT_SHIFT,
        CONTROL,
        RIGHT_CONTROL,
        OPTION,
        RIGHT_OPTION,
        COMMAND,
        RIGHT_COMMAND,
        CAPS_LOCK,
        FUNCTION,
    ] {
        assert!(is_modifier(m), "0x{m:02X}");
        assert!(!is_text_key(m), "0x{m:02X}");
    }
    for t in [
        ANSI_A,
        ANSI_Z,
        ANSI_1,
        ANSI_GRAVE,
        ANSI_SLASH,
        SPACE,
        ISO_SECTION,
        JIS_YEN,
        JIS_UNDERSCORE,
        ANSI_KEYPAD_5,
    ] {
        assert!(is_text_key(t), "0x{t:02X}");
        assert!(!is_modifier(t), "0x{t:02X}");
    }
    for k in [
        RETURN,
        TAB,
        DELETE,
        ESCAPE,
        F1,
        LEFT_ARROW,
        HOME,
        FORWARD_DELETE,
        ANSI_KEYPAD_ENTER,
        JIS_EISU,
        JIS_KANA,
    ] {
        assert!(!is_text_key(k), "0x{k:02X}");
        assert!(!is_modifier(k), "0x{k:02X}");
    }
}

#[test]
fn scancode_event() {
    assert_eq!(
        Scancode::ext(0x5B).event(true),
        InputEvent::Key { scancode: 0x5B, extended: true, down: true }
    );
    assert_eq!(
        Scancode::new(0x1E).event(false),
        InputEvent::Key { scancode: 0x1E, extended: false, down: false }
    );
}

// ---- HOST-6: the inverse table (set-1 scancode → kVK_*) for the Mac host --------------------

/// Every `kVK_*` that maps to a scancode maps back to itself, for each keyboard type: both
/// directions come from the same table, so they cannot drift apart.
#[test]
fn kvk_for_round_trips_every_mapped_key() {
    for keyboard in [KeyboardType::Ansi, KeyboardType::Iso, KeyboardType::Jis] {
        for &(name, code) in ALL_KVK {
            let Some(sc) = scancode_for(code, keyboard, CmdAs::Super) else { continue };
            assert_eq!(kvk_for(sc, keyboard), Some(code), "{name} on {keyboard:?} via {sc:?}");
        }
    }
}

#[test]
fn kvk_for_maps_windows_keys_to_command_and_keeps_control() {
    assert_eq!(kvk_for(Scancode::ext(0x5B), KeyboardType::Ansi), Some(kvk::COMMAND));
    assert_eq!(kvk_for(Scancode::ext(0x5C), KeyboardType::Ansi), Some(kvk::RIGHT_COMMAND));
    assert_eq!(kvk_for(Scancode::new(0x1D), KeyboardType::Ansi), Some(kvk::CONTROL));
    assert_eq!(kvk_for(Scancode::ext(0x1D), KeyboardType::Ansi), Some(kvk::RIGHT_CONTROL));
}

#[test]
fn kvk_for_undoes_the_iso_swap_on_iso_hosts() {
    // The key left of `1` (scancode 0x29) is kVK_ISO_Section on an ISO Mac, kVK_ANSI_Grave on ANSI.
    assert_eq!(kvk_for(Scancode::new(0x29), KeyboardType::Iso), Some(kvk::ISO_SECTION));
    assert_eq!(kvk_for(Scancode::new(0x56), KeyboardType::Iso), Some(kvk::ANSI_GRAVE));
    assert_eq!(kvk_for(Scancode::new(0x29), KeyboardType::Ansi), Some(kvk::ANSI_GRAVE));
    assert_eq!(kvk_for(Scancode::new(0x56), KeyboardType::Ansi), Some(kvk::ISO_SECTION));
}

#[test]
fn kvk_for_has_no_answer_for_pc_only_keys() {
    // Num Lock, Print Screen, Scroll Lock, Pause/Break (E1 sequence never reaches here) and
    // unknown codes have no Mac key.
    for sc in [
        Scancode::new(0x45),
        Scancode::ext(0x37),
        Scancode::new(0x46),
        Scancode::new(0x00),
        Scancode::ext(0x7F),
    ] {
        assert_eq!(kvk_for(sc, KeyboardType::Ansi), None, "{sc:?}");
    }
}
