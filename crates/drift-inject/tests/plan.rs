//! HOST-6 Red: RDP fast-path input → the macOS events Drift Host injects (pure plan).
#![allow(missing_docs, clippy::unwrap_used)]

use drift_inject::plan::{
    DisplayMap, InjectEvent, Injector, ModFlags, MouseButton, PIXELS_PER_NOTCH, ScrollPhase,
};
use drift_input::KeyboardType;
use drift_input::keymap::kvk;
use ironrdp_pdu::input::fast_path::{FastPathInputEvent as F, KeyboardFlags as K, SynchronizeFlags};
use ironrdp_pdu::input::mouse::{MousePdu, PointerFlags as P};
use ironrdp_pdu::input::mouse_rel::{MouseRelPdu, PointerRelFlags};
use ironrdp_pdu::input::mouse_x::{MouseXPdu, PointerXFlags};

/// A 1280×800 desktop shown at half size, its top-left corner at (100, 50) points.
fn injector() -> Injector {
    Injector::new(
        DisplayMap { origin: (100.0, 50.0), desktop: (1280, 800), points_per_pixel: 0.5 },
        KeyboardType::Ansi,
    )
}

fn key(code: u8) -> F {
    F::KeyboardEvent(K::empty(), code)
}
fn key_up(code: u8) -> F {
    F::KeyboardEvent(K::RELEASE, code)
}
fn mouse(flags: P, x: u16, y: u16) -> F {
    F::MouseEvent(MousePdu { flags, number_of_wheel_rotation_units: 0, x_position: x, y_position: y })
}
fn wheel(flags: P, units: i16) -> F {
    F::MouseEvent(MousePdu { flags, number_of_wheel_rotation_units: units, x_position: 0, y_position: 0 })
}

#[test]
fn keys_carry_the_modifiers_that_are_held() {
    let mut i = injector();
    assert_eq!(
        i.apply(&key(0x2A), 0),
        vec![InjectEvent::Key { kvk: kvk::SHIFT, down: true, flags: ModFlags::SHIFT }]
    );
    assert_eq!(
        i.apply(&key(0x1E), 0),
        vec![InjectEvent::Key { kvk: kvk::ANSI_A, down: true, flags: ModFlags::SHIFT }]
    );
    assert_eq!(
        i.apply(&key_up(0x1E), 0),
        vec![InjectEvent::Key { kvk: kvk::ANSI_A, down: false, flags: ModFlags::SHIFT }]
    );
    assert_eq!(
        i.apply(&key_up(0x2A), 0),
        vec![InjectEvent::Key { kvk: kvk::SHIFT, down: false, flags: ModFlags::NONE }]
    );
}

#[test]
fn extended_keys_and_the_windows_key() {
    let mut i = injector();
    assert_eq!(
        i.apply(&F::KeyboardEvent(K::EXTENDED, 0x4B), 0),
        vec![InjectEvent::Key { kvk: kvk::LEFT_ARROW, down: true, flags: ModFlags::NONE }]
    );
    assert_eq!(
        i.apply(&F::KeyboardEvent(K::EXTENDED, 0x5B), 0),
        vec![InjectEvent::Key { kvk: kvk::COMMAND, down: true, flags: ModFlags::COMMAND }]
    );
}

#[test]
fn unknown_scancodes_inject_nothing() {
    let mut i = injector();
    assert!(i.apply(&key(0x45), 0).is_empty(), "Num Lock has no Mac key");
    assert!(i.apply(&key(0x00), 0).is_empty());
}

#[test]
fn unicode_keys_become_text_events() {
    let mut i = injector();
    let e = u16::from(b'e') + 0x84; // é
    assert_eq!(
        i.apply(&F::UnicodeKeyboardEvent(K::empty(), e), 0),
        vec![InjectEvent::Text { utf16: e, down: true }]
    );
    assert_eq!(
        i.apply(&F::UnicodeKeyboardEvent(K::RELEASE, e), 0),
        vec![InjectEvent::Text { utf16: e, down: false }]
    );
}

#[test]
fn moves_map_desktop_pixels_to_display_points_and_clamp() {
    let mut i = injector();
    assert_eq!(
        i.apply(&mouse(P::MOVE, 200, 100), 0),
        vec![InjectEvent::Move { x: 200.0, y: 100.0, drag: None, flags: ModFlags::NONE }]
    );
    assert_eq!(
        i.apply(&mouse(P::MOVE, 5000, 5000), 0),
        vec![InjectEvent::Move {
            x: 100.0 + 1279.0 * 0.5,
            y: 50.0 + 799.0 * 0.5,
            drag: None,
            flags: ModFlags::NONE
        }]
    );
}

#[test]
fn moves_with_a_button_held_are_drags() {
    let mut i = injector();
    i.apply(&mouse(P::LEFT_BUTTON | P::DOWN, 0, 0), 0);
    assert_eq!(
        i.apply(&mouse(P::MOVE, 20, 0), 10),
        vec![InjectEvent::Move { x: 110.0, y: 50.0, drag: Some(MouseButton::Left), flags: ModFlags::NONE }]
    );
    let up = i.apply(&mouse(P::LEFT_BUTTON, 20, 0), 20);
    assert!(
        matches!(up.as_slice(), [InjectEvent::Button { button: MouseButton::Left, down: false, .. }]),
        "{up:?}"
    );
}

#[test]
fn double_clicks_count_within_time_and_distance() {
    let mut i = injector();
    let clicks = |events: Vec<InjectEvent>| match events.as_slice() {
        [InjectEvent::Button { clicks, .. }] => *clicks,
        other => panic!("{other:?}"),
    };
    assert_eq!(clicks(i.apply(&mouse(P::LEFT_BUTTON | P::DOWN, 40, 40), 0)), 1);
    i.apply(&mouse(P::LEFT_BUTTON, 40, 40), 50);
    assert_eq!(clicks(i.apply(&mouse(P::LEFT_BUTTON | P::DOWN, 41, 40), 200)), 2);
    i.apply(&mouse(P::LEFT_BUTTON, 41, 40), 250);
    assert_eq!(clicks(i.apply(&mouse(P::LEFT_BUTTON | P::DOWN, 41, 40), 1500)), 1, "too late");
    i.apply(&mouse(P::LEFT_BUTTON, 41, 40), 1550);
    assert_eq!(clicks(i.apply(&mouse(P::LEFT_BUTTON | P::DOWN, 200, 40), 1600)), 1, "too far");
}

#[test]
fn right_middle_and_extra_buttons() {
    let mut i = injector();
    let button = |e: Vec<InjectEvent>| match e.as_slice() {
        [InjectEvent::Button { button, down: true, .. }] => *button,
        other => panic!("{other:?}"),
    };
    assert_eq!(button(i.apply(&mouse(P::RIGHT_BUTTON | P::DOWN, 0, 0), 0)), MouseButton::Right);
    assert_eq!(button(i.apply(&mouse(P::MIDDLE_BUTTON_OR_WHEEL | P::DOWN, 0, 0), 0)), MouseButton::Middle);
    let x1 = F::MouseEventEx(MouseXPdu {
        flags: PointerXFlags::BUTTON1 | PointerXFlags::DOWN,
        x_position: 0,
        y_position: 0,
    });
    assert_eq!(button(i.apply(&x1, 0)), MouseButton::Other(3));
    let x2 = F::MouseEventEx(MouseXPdu {
        flags: PointerXFlags::BUTTON2 | PointerXFlags::DOWN,
        x_position: 0,
        y_position: 0,
    });
    assert_eq!(button(i.apply(&x2, 0)), MouseButton::Other(4));
}

#[test]
fn wheel_notches_become_pixel_scrolls() {
    let mut i = injector();
    assert_eq!(
        i.apply(&wheel(P::VERTICAL_WHEEL, 120), 0),
        vec![InjectEvent::Scroll { dx: 0.0, dy: PIXELS_PER_NOTCH, phase: ScrollPhase::None }]
    );
    assert_eq!(
        i.apply(&wheel(P::VERTICAL_WHEEL | P::WHEEL_NEGATIVE, -60), 0),
        vec![InjectEvent::Scroll { dx: 0.0, dy: -PIXELS_PER_NOTCH / 2.0, phase: ScrollPhase::None }]
    );
    // Windows' horizontal wheel is positive to the right; macOS's is positive to the left.
    assert_eq!(
        i.apply(&wheel(P::HORIZONTAL_WHEEL, 120), 0),
        vec![InjectEvent::Scroll { dx: -PIXELS_PER_NOTCH, dy: 0.0, phase: ScrollPhase::None }]
    );
}

#[test]
fn relative_moves_follow_the_pointer() {
    let mut i = injector();
    i.apply(&mouse(P::MOVE, 200, 100), 0);
    let rel = F::MouseEventRel(MouseRelPdu { flags: PointerRelFlags::MOVE, x_delta: 10, y_delta: -4 });
    assert_eq!(
        i.apply(&rel, 0),
        vec![InjectEvent::Move { x: 205.0, y: 98.0, drag: None, flags: ModFlags::NONE }]
    );
}

#[test]
fn release_all_lets_go_of_held_keys_and_buttons() {
    let mut i = injector();
    i.apply(&key(0x2A), 0);
    i.apply(&mouse(P::LEFT_BUTTON | P::DOWN, 0, 0), 0);
    let released = i.release_all();
    assert!(
        released.contains(&InjectEvent::Key { kvk: kvk::SHIFT, down: false, flags: ModFlags::NONE }),
        "{released:?}"
    );
    assert!(
        released
            .iter()
            .any(|e| matches!(e, InjectEvent::Button { button: MouseButton::Left, down: false, .. })),
        "{released:?}"
    );
    assert!(i.release_all().is_empty(), "nothing is held any more");
}

#[test]
fn sync_reports_the_remote_lock_state() {
    let mut i = injector();
    assert_eq!(
        i.apply(&F::SyncEvent(SynchronizeFlags::CAPS_LOCK), 0),
        vec![InjectEvent::Locks { caps_lock: true }]
    );
}

#[test]
fn a_new_display_map_moves_the_target() {
    let mut i = injector();
    i.set_display(DisplayMap { origin: (0.0, 0.0), desktop: (1000, 1000), points_per_pixel: 1.0 });
    assert_eq!(
        i.apply(&mouse(P::MOVE, 10, 20), 0),
        vec![InjectEvent::Move { x: 10.0, y: 20.0, drag: None, flags: ModFlags::NONE }]
    );
}

#[test]
fn trackpad_scrolls_carry_their_phase_for_ext_2() {
    let i = injector();
    assert_eq!(
        i.scroll_pixels(3.0, -12.5, ScrollPhase::Changed),
        InjectEvent::Scroll { dx: 3.0, dy: -12.5, phase: ScrollPhase::Changed }
    );
}
