//! EXT-2 Red: precise scroll events carry their phase and the wheel units plain RDP gets
//! instead.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_core::{DesktopSize, InputEvent, MomentumPhase, Point, ScrollPhase};
use drift_input::pointer::Pointer;
use drift_input::scroll::{ScrollConfig, ScrollDelta, wheel_events};
use drift_input::touch::{Mode, Touches};
use drift_input::{ScaleMode, Zoom};

#[test]
fn wheel_events_split_units_into_rdp_sized_events_vertical_first() {
    assert_eq!(
        wheel_events(300, -20),
        vec![
            InputEvent::Wheel { horizontal: false, units: 255 },
            InputEvent::Wheel { horizontal: false, units: 45 },
            InputEvent::Wheel { horizontal: true, units: -20 },
        ]
    );
    assert!(wheel_events(0, 0).is_empty());
}

fn pointer(reverse: bool) -> Pointer {
    let mut p = Pointer::new(ScrollConfig { units_per_point: 2.0, reverse });
    p.set_desktop(DesktopSize { width: 1280, height: 800 }, ScaleMode::Fit);
    p.set_view_geometry(drift_core::ViewGeometry {
        points: drift_core::Size::new(1280.0, 800.0),
        backing_scale: 1.0,
    });
    p
}

#[test]
fn a_precise_scroll_keeps_points_and_phase_and_the_same_wheel_fallback() {
    let mut p = pointer(false);
    let mut plain = pointer(false);
    let wheel: Vec<InputEvent> = plain.scroll(ScrollDelta::Precise { dx: 1.5, dy: 10.25 }, true);
    let event = p.precise_scroll(1.5, 10.25, ScrollPhase::Began, MomentumPhase::None);
    let InputEvent::PreciseScroll { dx, dy, phase, momentum, wheel_v, wheel_h } = event else {
        panic!("{event:?}")
    };
    assert_eq!((dx, dy, phase, momentum), (150, 1025, ScrollPhase::Began, MomentumPhase::None));
    assert_eq!(wheel_events(wheel_v, wheel_h), wheel, "plain RDP gets exactly today's wheel events");
}

#[test]
fn reverse_applies_to_the_precise_deltas_too() {
    let mut p = pointer(true);
    let InputEvent::PreciseScroll { dx, dy, .. } =
        p.precise_scroll(2.0, -3.0, ScrollPhase::Changed, MomentumPhase::None)
    else {
        panic!()
    };
    assert_eq!((dx, dy), (-200, 300));
}

#[test]
fn a_two_finger_pan_is_a_phased_precise_scroll_and_ends_on_lift() {
    let mut p = pointer(false);
    let z = Zoom::default();
    let mut t = Touches::new(Mode::Trackpad, false);
    p.move_to(Point::new(100.0, 100.0));
    let at = |x, y| Point::new(x, y);
    t.began(&mut p, &z, 1, at(400.0, 300.0), 0.0);
    t.began(&mut p, &z, 2, at(450.0, 300.0), 0.0);
    let mut out = t.moved(&mut p, &z, 1, at(400.0, 330.0), 0.05).events;
    out.extend(t.moved(&mut p, &z, 2, at(450.0, 330.0), 0.05).events);
    out.extend(t.ended(&mut p, &z, 1, at(400.0, 330.0), 0.4).events);
    out.extend(t.ended(&mut p, &z, 2, at(450.0, 330.0), 0.4).events);
    let phases: Vec<ScrollPhase> = out
        .iter()
        .map(|e| match e {
            InputEvent::PreciseScroll { phase, .. } => *phase,
            other => panic!("only precise scrolls: {other:?}"),
        })
        .collect();
    assert_eq!(phases.first(), Some(&ScrollPhase::Began));
    assert_eq!(phases.last(), Some(&ScrollPhase::Ended));
    let units: i32 = out
        .iter()
        .map(|e| match e {
            InputEvent::PreciseScroll { wheel_v, .. } => *wheel_v,
            _ => 0,
        })
        .sum();
    assert_eq!(units, 60, "the wheel fallback is unchanged: 30 points × 2 units");
}

#[test]
fn a_pinch_without_local_zoom_sends_nothing() {
    let mut p = pointer(false);
    let z = Zoom::default();
    let mut t = Touches::new(Mode::Direct, false);
    let at = |x, y| Point::new(x, y);
    t.began(&mut p, &z, 1, at(400.0, 300.0), 0.0);
    t.began(&mut p, &z, 2, at(500.0, 300.0), 0.0);
    let mut out = t.moved(&mut p, &z, 2, at(600.0, 300.0), 0.05).events;
    out.extend(t.ended(&mut p, &z, 1, at(400.0, 300.0), 0.3).events);
    assert!(out.is_empty(), "GNOME has no pinch: {out:?}");
}
