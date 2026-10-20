//! M2-2 / M2-4: the RemoteView's input brain — key-equivalent routing (allow-list goes to the
//! menu, everything else is claimed for the remote), text input, pointer mapping and focus.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_core::{CmdAs, InputEvent, KeyboardPrefs, MouseButton, Point, Size, ViewGeometry};
use drift_input::keymap::kvk;
use drift_input::{KeyDown, KeyboardType, MenuShortcut, ModifierFlags, ScaleMode, ScrollConfig, ScrollDelta};
use drift_macos::input::{InputController, KeyEquivalent, mouse_button};

const CMD_L: u64 = ModifierFlags::COMMAND | ModifierFlags::DEVICE_LEFT_COMMAND;
const CTRL_L: u64 = ModifierFlags::CONTROL | ModifierFlags::DEVICE_LEFT_CONTROL;
const SHIFT_L: u64 = ModifierFlags::SHIFT | ModifierFlags::DEVICE_LEFT_SHIFT;
const KVK_COMMAND: u16 = 0x37;
const KVK_CONTROL: u16 = 0x3B;
const KVK_TAB: u16 = 0x30;
const KVK_F5: u16 = 0x60;

fn key(scancode: u8, extended: bool, down: bool) -> InputEvent {
    InputEvent::Key { scancode, extended, down }
}

fn flags(bits: u64) -> ModifierFlags {
    ModifierFlags(bits)
}

fn controller() -> InputController {
    InputController::new(KeyboardPrefs::default(), KeyboardType::Ansi, ScrollConfig::default())
}

fn unicode_controller() -> InputController {
    let prefs = KeyboardPrefs { cmd_as: CmdAs::Super, type_with_mac_layout: true };
    InputController::new(prefs, KeyboardType::Ansi, ScrollConfig::default())
}

fn connected(view: (f64, f64), scale: f64, desktop: (u32, u32), mode: ScaleMode) -> InputController {
    let mut c = controller();
    c.set_view_geometry(ViewGeometry { points: Size::new(view.0, view.1), backing_scale: scale });
    c.set_desktop(Size::new(desktop.0, desktop.1), mode);
    c
}

// ---------------------------------------------------------------------------------------
// performKeyEquivalent: (M2-4)

#[test]
fn key_equivalents_pass_when_the_view_is_not_first_responder() {
    let mut c = controller();
    // e.g. the connect form's webview has focus: Cmd+C must reach the webview / Edit menu.
    assert_eq!(c.key_equivalent(false, kvk::ANSI_C, "c", flags(CMD_L), false), KeyEquivalent::Pass);
    assert_eq!(c.key_equivalent(false, kvk::ANSI_T, "t", flags(CMD_L), false), KeyEquivalent::Pass);
}

#[test]
fn key_equivalents_without_command_or_control_pass_to_key_down() {
    let mut c = controller();
    assert_eq!(c.key_equivalent(true, KVK_F5, "\u{F708}", flags(0), false), KeyEquivalent::Pass);
    assert_eq!(c.key_equivalent(true, kvk::ANSI_A, "a", flags(SHIFT_L), false), KeyEquivalent::Pass);
}

#[test]
fn allow_listed_combos_go_to_the_menu_and_never_reach_the_remote() {
    let cases = [
        (kvk::ANSI_N, "n", CMD_L, MenuShortcut::NewConnection),
        (kvk::ANSI_0, "0", CMD_L, MenuShortcut::ShowConnections),
        (kvk::ANSI_E, "e", CMD_L, MenuShortcut::EditConnection),
        (kvk::ANSI_D, "D", CMD_L | SHIFT_L, MenuShortcut::Disconnect),
        (kvk::ANSI_W, "w", CMD_L, MenuShortcut::CloseWindow),
        (kvk::ANSI_Q, "q", CMD_L, MenuShortcut::Quit),
        (kvk::ANSI_1, "1", CMD_L, MenuShortcut::SelectSession(1)),
        (kvk::ANSI_9, "9", CMD_L, MenuShortcut::SelectSession(9)),
        (kvk::ANSI_GRAVE, "`", CMD_L, MenuShortcut::CycleWindows),
    ];
    for (code, chars, bits, want) in cases {
        let mut c = controller();
        let mut wire = c.flags_changed(KVK_COMMAND, flags(CMD_L));
        if bits & ModifierFlags::SHIFT != 0 {
            wire.extend(c.flags_changed(0x38, flags(bits)));
        }
        assert_eq!(
            c.key_equivalent(true, code, chars, flags(bits), false),
            KeyEquivalent::Menu(want),
            "{chars}"
        );
        // AppKit swallows the key-up of a menu equivalent; releasing Command sends nothing.
        wire.extend(c.key_up(code));
        wire.extend(c.flags_changed(KVK_COMMAND, flags(0)));
        let commands: Vec<_> =
            wire.iter().filter(|e| matches!(e, InputEvent::Key { scancode: 0x5B, .. })).collect();
        assert!(commands.is_empty(), "{chars}: the remote saw Super: {wire:?}");
        assert!(
            !wire
                .iter()
                .any(|e| matches!(e, InputEvent::Key { down: true, scancode, .. } if *scancode != 0x2A)),
            "{chars}: {wire:?}"
        );
    }
}

#[test]
fn other_command_combos_are_claimed_and_sent_as_scancodes() {
    let mut c = controller();
    assert_eq!(c.flags_changed(KVK_COMMAND, flags(CMD_L)), vec![], "Command is deferred");
    // Cmd+K: Super (0x5B ext) then K (0x25).
    assert_eq!(
        c.key_equivalent(true, kvk::ANSI_K, "k", flags(CMD_L), false),
        KeyEquivalent::Claim(vec![key(0x5B, true, true), key(0x25, false, true)])
    );
    assert_eq!(c.key_up(kvk::ANSI_K), vec![key(0x25, false, false)]);
    assert_eq!(c.flags_changed(KVK_COMMAND, flags(0)), vec![key(0x5B, true, false)]);
}

#[test]
fn cmd_c_is_claimed_instead_of_being_eaten_by_the_edit_menu() {
    let mut c = controller();
    c.flags_changed(KVK_COMMAND, flags(CMD_L));
    let KeyEquivalent::Claim(events) = c.key_equivalent(true, kvk::ANSI_C, "c", flags(CMD_L), false) else {
        panic!("Cmd+C must be claimed");
    };
    assert_eq!(events, vec![key(0x5B, true, true), key(0x2E, false, true)]);
}

#[test]
fn cmd_as_ctrl_sends_left_control() {
    let prefs = KeyboardPrefs { cmd_as: CmdAs::Ctrl, type_with_mac_layout: false };
    let mut c = InputController::new(prefs, KeyboardType::Ansi, ScrollConfig::default());
    c.flags_changed(KVK_COMMAND, flags(CMD_L));
    assert_eq!(
        c.key_equivalent(true, kvk::ANSI_C, "c", flags(CMD_L), false),
        KeyEquivalent::Claim(vec![key(0x1D, false, true), key(0x2E, false, true)])
    );
}

#[test]
fn ctrl_tab_is_claimed() {
    let mut c = controller();
    assert_eq!(c.flags_changed(KVK_CONTROL, flags(CTRL_L)), vec![key(0x1D, false, true)]);
    assert_eq!(
        c.key_equivalent(true, KVK_TAB, "\t", flags(CTRL_L), false),
        KeyEquivalent::Claim(vec![key(0x0F, false, true)])
    );
    assert_eq!(c.key_up(KVK_TAB), vec![key(0x0F, false, false)]);
}

#[test]
fn repeated_key_equivalents_stay_claimed_but_send_nothing() {
    let mut c = controller();
    c.flags_changed(KVK_COMMAND, flags(CMD_L));
    assert!(matches!(c.key_equivalent(true, kvk::ANSI_K, "k", flags(CMD_L), false), KeyEquivalent::Claim(_)));
    assert_eq!(c.key_equivalent(true, kvk::ANSI_K, "k", flags(CMD_L), true), KeyEquivalent::Claim(vec![]));
}

#[test]
fn claimed_combo_in_unicode_mode_still_uses_scancodes() {
    let mut c = unicode_controller();
    c.flags_changed(KVK_CONTROL, flags(CTRL_L));
    assert_eq!(
        c.key_equivalent(true, kvk::ANSI_C, "c", flags(CTRL_L), false),
        KeyEquivalent::Claim(vec![key(0x2E, false, true)])
    );
}

// ---------------------------------------------------------------------------------------
// keyDown / text (M2-2)

#[test]
fn scancode_mode_sends_positional_keys() {
    let mut c = controller();
    assert_eq!(c.key_down(kvk::ANSI_A, flags(0), false, false), KeyDown::Send(vec![key(0x1E, false, true)]));
    assert_eq!(c.key_up(kvk::ANSI_A), vec![key(0x1E, false, false)]);
}

#[test]
fn unicode_mode_routes_text_keys_through_the_text_input_system() {
    let mut c = unicode_controller();
    assert_eq!(c.key_down(kvk::ANSI_E, flags(ModifierFlags::OPTION), false, false), KeyDown::InterpretText);
    // Dead key Option+E then E commits "é".
    assert_eq!(
        c.insert_text("é"),
        vec![InputEvent::Unicode { ch: 0xE9, down: true }, InputEvent::Unicode { ch: 0xE9, down: false }]
    );
    // Arrow keys are not text: scancodes (0x4B extended).
    assert_eq!(c.key_down(0x7B, flags(0), false, false), KeyDown::Send(vec![key(0x4B, true, true)]));
    // doCommandBySelector: fallback (e.g. Return) sends the scancode.
    assert_eq!(c.key_down_scancode(0x24, flags(0)), vec![key(0x1C, false, true)]);
}

#[test]
fn keyboard_prefs_can_change_while_connected() {
    let mut c = controller();
    c.set_keyboard_prefs(KeyboardPrefs { cmd_as: CmdAs::Super, type_with_mac_layout: true });
    assert_eq!(c.key_down(kvk::ANSI_A, flags(0), false, false), KeyDown::InterpretText);
}

#[test]
fn ctrl_alt_del_sequence() {
    let mut c = controller();
    assert_eq!(
        c.ctrl_alt_del(),
        vec![
            key(0x1D, false, true),
            key(0x38, false, true),
            key(0x53, true, true),
            key(0x53, true, false),
            key(0x38, false, false),
            key(0x1D, false, false),
        ]
    );
}

// ---------------------------------------------------------------------------------------
// Pointer

#[test]
fn button_numbers_map_to_rdp_buttons() {
    assert_eq!(mouse_button(0), Some(MouseButton::Left));
    assert_eq!(mouse_button(1), Some(MouseButton::Right));
    assert_eq!(mouse_button(2), Some(MouseButton::Middle));
    assert_eq!(mouse_button(3), Some(MouseButton::X1));
    assert_eq!(mouse_button(4), Some(MouseButton::X2));
    assert_eq!(mouse_button(5), None);
    assert_eq!(mouse_button(-1), None);
}

#[test]
fn pointer_events_are_dropped_without_a_desktop() {
    let mut c = controller();
    c.set_view_geometry(ViewGeometry { points: Size::new(800.0, 500.0), backing_scale: 2.0 });
    assert_eq!(c.mouse_move(Point::new(10.0, 10.0)), vec![]);
    assert_eq!(c.mouse_button(MouseButton::Left, true, Point::new(10.0, 10.0)), vec![]);
}

#[test]
fn retina_desktop_maps_points_to_pixels() {
    let mut c = connected((800.0, 500.0), 2.0, (1600, 1000), ScaleMode::Fit);
    assert_eq!(c.mouse_move(Point::new(100.25, 50.0)), vec![InputEvent::MouseMove { x: 200, y: 100 }]);
    // Same pixel again: nothing new to send.
    assert_eq!(c.mouse_move(Point::new(100.4, 50.2)), vec![]);
    assert_eq!(
        c.mouse_button(MouseButton::Left, true, Point::new(10.0, 10.0)),
        vec![InputEvent::MouseButton { button: MouseButton::Left, down: true, x: 20, y: 20 }]
    );
    assert_eq!(
        c.mouse_button(MouseButton::Left, false, Point::new(10.0, 10.0)),
        vec![InputEvent::MouseButton { button: MouseButton::Left, down: false, x: 20, y: 20 }]
    );
}

#[test]
fn letterboxed_desktop_clamps_bar_points_to_the_edge() {
    let mut c = connected((1000.0, 500.0), 1.0, (1000, 1000), ScaleMode::Fit);
    assert_eq!(c.mouse_move(Point::new(250.0, 0.0)), vec![InputEvent::MouseMove { x: 0, y: 0 }]);
    assert_eq!(c.mouse_move(Point::new(100.0, 10.0)), vec![InputEvent::MouseMove { x: 0, y: 20 }]);
    assert_eq!(c.mouse_move(Point::new(999.0, 499.0)), vec![InputEvent::MouseMove { x: 999, y: 998 }]);
}

#[test]
fn button_release_without_press_is_not_sent() {
    let mut c = connected((800.0, 500.0), 1.0, (800, 500), ScaleMode::Fit);
    assert_eq!(c.mouse_button(MouseButton::Right, false, Point::new(1.0, 1.0)), vec![]);
}

#[test]
fn deferred_command_is_flushed_before_a_click() {
    let mut c = connected((800.0, 500.0), 1.0, (800, 500), ScaleMode::Fit);
    c.flags_changed(KVK_COMMAND, flags(CMD_L));
    assert_eq!(
        c.mouse_button(MouseButton::Left, true, Point::new(5.0, 6.0)),
        vec![
            key(0x5B, true, true),
            InputEvent::MouseButton { button: MouseButton::Left, down: true, x: 5, y: 6 }
        ]
    );
}

#[test]
fn focus_loss_releases_keys_and_buttons() {
    let mut c = connected((800.0, 500.0), 1.0, (800, 500), ScaleMode::Fit);
    c.key_down(kvk::ANSI_A, flags(0), false, false);
    c.mouse_move(Point::new(30.0, 40.0));
    c.mouse_button(MouseButton::Left, true, Point::new(30.0, 40.0));
    let released = c.focus_lost();
    assert!(released.contains(&key(0x1E, false, false)), "{released:?}");
    assert!(
        released.contains(&InputEvent::MouseButton { button: MouseButton::Left, down: false, x: 30, y: 40 }),
        "{released:?}"
    );
    assert_eq!(c.focus_lost(), vec![], "nothing left to release");
    // The mouse-up that AppKit delivers afterwards is not sent again.
    assert_eq!(c.mouse_button(MouseButton::Left, false, Point::new(30.0, 40.0)), vec![]);
}

#[test]
fn focus_gain_syncs_lock_state() {
    let mut c = controller();
    let events = c.focus_gained(flags(ModifierFlags::CAPS_LOCK));
    assert!(events.contains(&InputEvent::SyncToggles { caps: true, num: true }), "{events:?}");
}

#[test]
fn clear_desktop_stops_pointer_events() {
    let mut c = connected((800.0, 500.0), 1.0, (800, 500), ScaleMode::Fit);
    c.clear_desktop();
    assert_eq!(c.mouse_move(Point::new(1.0, 1.0)), vec![]);
}

// ---------------------------------------------------------------------------------------
// Scroll

#[test]
fn precise_and_line_scrolling() {
    let mut c = controller();
    assert_eq!(
        c.scroll(ScrollDelta::Precise { dx: 0.0, dy: 6.0 }, true),
        vec![InputEvent::Wheel { horizontal: false, units: 12 }]
    );
    assert_eq!(
        c.scroll(ScrollDelta::Lines { dx: 0.0, dy: 1.0 }, false),
        vec![InputEvent::Wheel { horizontal: false, units: 120 }]
    );
}

#[test]
fn a_new_gesture_drops_the_previous_remainder() {
    let mut c = controller();
    assert_eq!(c.scroll(ScrollDelta::Precise { dx: 0.0, dy: 0.3 }, true), vec![]);
    // Without the reset the accumulated 1.2 units would emit one.
    assert_eq!(c.scroll(ScrollDelta::Precise { dx: 0.0, dy: 0.3 }, true), vec![]);
    assert_eq!(
        c.scroll(ScrollDelta::Precise { dx: 0.0, dy: 0.3 }, false),
        vec![InputEvent::Wheel { horizontal: false, units: 1 }]
    );
}

// ---- EXT-2: precise scroll and gesture values from AppKit -------------------------------

#[test]
fn precise_scroll_carries_points_phases_and_the_wheel_fallback() {
    use drift_core::{MomentumPhase, ScrollPhase};
    let mut c = controller();
    let mut plain = controller();
    let wheel = plain.scroll(ScrollDelta::Precise { dx: 0.0, dy: 7.5 }, true);
    let InputEvent::PreciseScroll { dx, dy, phase, momentum, wheel_v, wheel_h } =
        c.precise_scroll(0.0, 7.5, ScrollPhase::Began, MomentumPhase::None)
    else {
        panic!()
    };
    assert_eq!((dx, dy, phase, momentum), (0, 750, ScrollPhase::Began, MomentumPhase::None));
    assert_eq!(drift_input::scroll::wheel_events(wheel_v, wheel_h), wheel);
}

#[test]
fn appkit_phase_bits_become_wire_phases() {
    use drift_core::{MomentumPhase, ScrollPhase};
    use drift_macos::input::{momentum_phase, scroll_phase};
    assert_eq!(scroll_phase(0), ScrollPhase::None);
    assert_eq!(scroll_phase(1), ScrollPhase::Began);
    assert_eq!(scroll_phase(2), ScrollPhase::Changed, "stationary fingers are still the gesture");
    assert_eq!(scroll_phase(4), ScrollPhase::Changed);
    assert_eq!(scroll_phase(8), ScrollPhase::Ended);
    assert_eq!(scroll_phase(16), ScrollPhase::Cancelled);
    assert_eq!(scroll_phase(32), ScrollPhase::MayBegin);
    assert_eq!(momentum_phase(0), MomentumPhase::None);
    assert_eq!(momentum_phase(1), MomentumPhase::Began);
    assert_eq!(momentum_phase(4), MomentumPhase::Changed);
    assert_eq!(momentum_phase(8), MomentumPhase::Ended);
}
