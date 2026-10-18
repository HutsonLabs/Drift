//! EXT-2 Red: gestures and precise scrolling become Mac events on the host.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_core::{MomentumPhase, ScrollPhase as WirePhase, SwipeDirection};
use drift_ext::gestures::Gesture;
use drift_inject::gesture::{GesturePlanner, PINCH_STEP};
use drift_inject::plan::{InjectEvent, ModFlags, ScrollPhase};

const CONTROL: u16 = 0x3B;
const COMMAND: u16 = 0x37;
const LEFT: u16 = 0x7B;
const RIGHT: u16 = 0x7C;
const DOWN: u16 = 0x7D;
const UP: u16 = 0x7E;
const EQUAL: u16 = 0x18;
const MINUS: u16 = 0x1B;

fn keys(events: &[InjectEvent]) -> Vec<(u16, bool)> {
    events
        .iter()
        .map(|e| match e {
            InjectEvent::Key { kvk, down, .. } => (*kvk, *down),
            other => panic!("only keys: {other:?}"),
        })
        .collect()
}

fn chord(modifier: u16, key: u16) -> Vec<(u16, bool)> {
    vec![(modifier, true), (key, true), (key, false), (modifier, false)]
}

#[test]
fn swipes_become_mission_control_expose_and_spaces_shortcuts() {
    let mut p = GesturePlanner::default();
    let swipe = |d| Gesture::Swipe { direction: d, fingers: 3 };
    assert_eq!(keys(&p.plan(&swipe(SwipeDirection::Up))), chord(CONTROL, UP), "Mission Control");
    assert_eq!(keys(&p.plan(&swipe(SwipeDirection::Down))), chord(CONTROL, DOWN), "App Exposé");
    // Fingers moving left bring the Space on the right, as on a Mac trackpad.
    assert_eq!(keys(&p.plan(&swipe(SwipeDirection::Left))), chord(CONTROL, RIGHT));
    assert_eq!(keys(&p.plan(&swipe(SwipeDirection::Right))), chord(CONTROL, LEFT));
}

#[test]
fn the_control_key_carries_its_flag() {
    let mut p = GesturePlanner::default();
    let events = p.plan(&Gesture::Swipe { direction: SwipeDirection::Up, fingers: 4 });
    let InjectEvent::Key { flags, .. } = &events[1] else { panic!() };
    assert_eq!(*flags, ModFlags::CONTROL);
}

#[test]
fn pinch_steps_become_zoom_shortcuts_and_a_new_pinch_starts_from_zero() {
    let mut p = GesturePlanner::default();
    let pinch = |m, phase| Gesture::Pinch { magnification: m, phase };
    assert!(p.plan(&pinch(100, WirePhase::Began)).is_empty(), "below one step");
    let step = i32::try_from(PINCH_STEP).unwrap();
    assert_eq!(keys(&p.plan(&pinch(step, WirePhase::Changed))), chord(COMMAND, EQUAL), "zoom in");
    assert!(p.plan(&pinch(-50, WirePhase::Ended)).is_empty());
    // The remainder of the last pinch is dropped: a small pinch out does nothing.
    assert!(p.plan(&pinch(-100, WirePhase::Began)).is_empty());
    assert_eq!(keys(&p.plan(&pinch(-step, WirePhase::Changed))), chord(COMMAND, MINUS), "zoom out");
}

#[test]
fn smart_zoom_has_no_shortcut() {
    assert!(GesturePlanner::default().plan(&Gesture::SmartZoom).is_empty());
}

#[test]
fn precise_scroll_is_a_pixel_scroll_with_its_phase() {
    let mut p = GesturePlanner::default();
    let plan = |p: &mut GesturePlanner, phase, momentum| {
        p.plan(&Gesture::Scroll { dx: -250, dy: 1200, phase, momentum })
    };
    assert_eq!(
        plan(&mut p, WirePhase::Began, MomentumPhase::None),
        vec![InjectEvent::Scroll { dx: -2.5, dy: 12.0, phase: ScrollPhase::Began }]
    );
    assert_eq!(
        plan(&mut p, WirePhase::None, MomentumPhase::Began),
        vec![InjectEvent::Scroll { dx: -2.5, dy: 12.0, phase: ScrollPhase::MomentumBegan }]
    );
    assert_eq!(
        plan(&mut p, WirePhase::None, MomentumPhase::Changed),
        vec![InjectEvent::Scroll { dx: -2.5, dy: 12.0, phase: ScrollPhase::Momentum }]
    );
    assert_eq!(
        plan(&mut p, WirePhase::None, MomentumPhase::Ended),
        vec![InjectEvent::Scroll { dx: -2.5, dy: 12.0, phase: ScrollPhase::MomentumEnded }]
    );
    // `MayBegin` (fingers resting) carries no movement worth posting.
    assert!(plan(&mut p, WirePhase::MayBegin, MomentumPhase::None).is_empty());
}
