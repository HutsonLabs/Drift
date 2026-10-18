//! HOST-6 Red: the CGEvent poster builds the right events (nothing is posted; posting into the
//! logged-in session is the ignored `posts_a_harmless_move` test and a manual step).
#![cfg(target_os = "macos")]
#![allow(missing_docs, clippy::unwrap_used)]

use drift_inject::cg::{self, EventKind};
use drift_inject::plan::{InjectEvent, ModFlags, MouseButton, ScrollPhase};
use drift_input::keymap::kvk;

#[test]
fn key_events_carry_keycode_direction_and_flags() {
    let e = cg::describe(&InjectEvent::Key {
        kvk: kvk::ANSI_A,
        down: true,
        flags: ModFlags::SHIFT | ModFlags::COMMAND,
    })
    .unwrap();
    assert_eq!(e.kind, EventKind::KeyDown);
    assert_eq!(e.keycode, Some(i64::from(kvk::ANSI_A)));
    assert!(e.shift && e.command && !e.control && !e.option);
    let up =
        cg::describe(&InjectEvent::Key { kvk: kvk::ANSI_A, down: false, flags: ModFlags::NONE }).unwrap();
    assert_eq!(up.kind, EventKind::KeyUp);
}

#[test]
fn mouse_events_have_the_right_type_location_and_clicks() {
    let mv =
        cg::describe(&InjectEvent::Move { x: 10.0, y: 20.0, drag: None, flags: ModFlags::NONE }).unwrap();
    assert_eq!((mv.kind, mv.location), (EventKind::MouseMoved, (10.0, 20.0)));
    let drag = cg::describe(&InjectEvent::Move {
        x: 1.0,
        y: 2.0,
        drag: Some(MouseButton::Right),
        flags: ModFlags::NONE,
    })
    .unwrap();
    assert_eq!(drag.kind, EventKind::RightMouseDragged);
    let dbl = cg::describe(&InjectEvent::Button {
        button: MouseButton::Left,
        down: true,
        x: 5.0,
        y: 6.0,
        clicks: 2,
        flags: ModFlags::NONE,
    })
    .unwrap();
    assert_eq!((dbl.kind, dbl.clicks), (EventKind::LeftMouseDown, 2));
    let x1 = cg::describe(&InjectEvent::Button {
        button: MouseButton::Other(3),
        down: false,
        x: 0.0,
        y: 0.0,
        clicks: 1,
        flags: ModFlags::NONE,
    })
    .unwrap();
    assert_eq!((x1.kind, x1.button_number), (EventKind::OtherMouseUp, 3));
}

#[test]
fn scrolls_are_pixel_continuous_with_their_phase() {
    let s = cg::describe(&InjectEvent::Scroll { dx: -4.0, dy: 30.0, phase: ScrollPhase::Changed }).unwrap();
    assert_eq!(s.kind, EventKind::ScrollWheel);
    assert_eq!((s.scroll_pixels, s.continuous, s.phase), ((30, -4), true, 2));
}

#[test]
fn text_events_carry_the_utf16_unit() {
    let t = cg::describe(&InjectEvent::Text { utf16: 0x00E9, down: true }).unwrap();
    assert_eq!((t.kind, t.text.as_deref()), (EventKind::KeyDown, Some("é")));
}

#[test]
fn lock_state_needs_no_event() {
    assert!(cg::describe(&InjectEvent::Locks { caps_lock: true }).is_none());
}

#[test]
fn accessibility_trust_can_be_queried() {
    // Posting needs the Accessibility permission; the answer depends on the machine.
    let trusted = cg::accessibility_trusted();
    eprintln!("Accessibility trusted: {trusted}");
}

#[test]
#[ignore = "posts a real event into the logged-in session; run by hand (docs/acceptance.md)"]
fn posts_a_harmless_move() {
    assert!(cg::accessibility_trusted(), "grant Accessibility to the terminal first");
    let here = cg::current_pointer().unwrap();
    cg::post(&InjectEvent::Move { x: here.0, y: here.1, drag: None, flags: ModFlags::NONE });
}
