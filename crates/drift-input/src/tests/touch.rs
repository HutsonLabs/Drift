use super::*;
use crate::coords::ScaleMode;
use crate::scroll::ScrollConfig;
use drift_core::{DesktopSize, MouseButton, Size, ViewGeometry};

/// 1000×500 points showing a 2000×1000 desktop (2 desktop pixels per point).
fn pointer() -> Pointer {
    let mut p = Pointer::new(ScrollConfig::default());
    p.set_view_geometry(ViewGeometry { points: Size::new(1000.0, 500.0), backing_scale: 2.0 });
    p.set_desktop(DesktopSize { width: 2000, height: 1000 }, ScaleMode::Fit);
    p
}

fn zoom() -> Zoom {
    let mut z = Zoom::default();
    z.set_view(Size::new(1000.0, 500.0));
    z
}

fn at(x: f64, y: f64) -> Point<f64> {
    Point::new(x, y)
}

fn click(button: MouseButton, x: u16, y: u16) -> Vec<InputEvent> {
    vec![
        InputEvent::MouseButton { button, down: true, x, y },
        InputEvent::MouseButton { button, down: false, x, y },
    ]
}

fn mv(x: u16, y: u16) -> InputEvent {
    InputEvent::MouseMove { x, y }
}

// ---- trackpad mode -------------------------------------------------------------------------

#[test]
fn trackpad_drag_moves_the_pointer_relatively() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Trackpad, false);
    p.move_to(at(100.0, 100.0));
    assert!(t.began(&mut p, &z, 1, at(500.0, 300.0), 0.0).events.is_empty());
    assert!(t.moved(&mut p, &z, 1, at(503.0, 300.0), 0.01).events.is_empty(), "inside the tap slop");
    // Past the slop: the whole movement so far counts (20 points × gain 1.5 = 30 points = 60 px).
    assert_eq!(t.moved(&mut p, &z, 1, at(520.0, 300.0), 0.05).events, vec![mv(260, 200)]);
    assert_eq!(t.moved(&mut p, &z, 1, at(520.0, 310.0), 0.06).events, vec![mv(260, 230)]);
    assert!(t.ended(&mut p, &z, 1, at(520.0, 310.0), 0.5).events.is_empty(), "a drag is not a click");
}

#[test]
fn trackpad_tap_clicks_where_the_pointer_is() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Trackpad, false);
    p.move_to(at(100.0, 100.0));
    t.began(&mut p, &z, 1, at(700.0, 400.0), 0.0);
    assert_eq!(t.ended(&mut p, &z, 1, at(702.0, 401.0), 0.1).events, click(MouseButton::Left, 200, 200));
}

#[test]
fn trackpad_slow_press_is_not_a_tap() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Trackpad, false);
    p.move_to(at(100.0, 100.0));
    t.began(&mut p, &z, 1, at(700.0, 400.0), 0.0);
    assert!(t.ended(&mut p, &z, 1, at(700.0, 400.0), 1.0).events.is_empty());
}

#[test]
fn trackpad_tap_and_drag_holds_the_button() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Trackpad, false);
    p.move_to(at(100.0, 100.0));
    t.began(&mut p, &z, 1, at(500.0, 300.0), 0.0);
    t.ended(&mut p, &z, 1, at(500.0, 300.0), 0.1);
    t.began(&mut p, &z, 2, at(505.0, 300.0), 0.25);
    let out = t.moved(&mut p, &z, 2, at(525.0, 300.0), 0.3).events;
    assert_eq!(
        out,
        vec![InputEvent::MouseButton { button: MouseButton::Left, down: true, x: 200, y: 200 }, mv(260, 200)]
    );
    assert_eq!(
        t.ended(&mut p, &z, 2, at(525.0, 300.0), 0.6).events,
        vec![InputEvent::MouseButton { button: MouseButton::Left, down: false, x: 260, y: 200 }]
    );
}

#[test]
fn trackpad_double_tap_is_two_clicks() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Trackpad, false);
    p.move_to(at(100.0, 100.0));
    t.began(&mut p, &z, 1, at(500.0, 300.0), 0.0);
    let first = t.ended(&mut p, &z, 1, at(500.0, 300.0), 0.1).events;
    t.began(&mut p, &z, 2, at(502.0, 300.0), 0.2);
    let second = t.ended(&mut p, &z, 2, at(502.0, 300.0), 0.28).events;
    assert_eq!(first, click(MouseButton::Left, 200, 200));
    assert_eq!(second, click(MouseButton::Left, 200, 200));
}

// ---- two fingers (both modes) -------------------------------------------------------------

#[test]
fn two_finger_tap_is_a_right_click() {
    for mode in [Mode::Trackpad, Mode::Direct] {
        let (mut p, z) = (pointer(), zoom());
        let mut t = Touches::new(mode, false);
        p.move_to(at(100.0, 100.0));
        t.began(&mut p, &z, 1, at(400.0, 300.0), 0.0);
        let _ = t.began(&mut p, &z, 2, at(450.0, 300.0), 0.02);
        let mut out = t.ended(&mut p, &z, 1, at(400.0, 300.0), 0.12).events;
        out.extend(t.ended(&mut p, &z, 2, at(450.0, 300.0), 0.13).events);
        let clicks: Vec<_> =
            out.iter().filter(|e| matches!(e, InputEvent::MouseButton { .. })).cloned().collect();
        assert_eq!(
            clicks,
            click(MouseButton::Right, p.position().unwrap().x, p.position().unwrap().y),
            "{mode:?}"
        );
    }
}

#[test]
fn two_finger_pan_scrolls() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Trackpad, false);
    p.move_to(at(100.0, 100.0));
    t.began(&mut p, &z, 1, at(400.0, 300.0), 0.0);
    t.began(&mut p, &z, 2, at(450.0, 300.0), 0.0);
    let mut out = t.moved(&mut p, &z, 1, at(400.0, 330.0), 0.05).events;
    out.extend(t.moved(&mut p, &z, 2, at(450.0, 330.0), 0.05).events);
    // The centre moved 30 points down: 30 × 2 units per point = 60 wheel units (natural),
    // carried as the fallback of precise scrolls since EXT-2.
    let units: i32 = out
        .iter()
        .map(|e| match e {
            InputEvent::PreciseScroll { wheel_v, wheel_h: 0, .. } => *wheel_v,
            other => panic!("only vertical precise scrolls: {other:?}"),
        })
        .sum();
    assert_eq!(units, 60);
    let mut rest = t.ended(&mut p, &z, 1, at(400.0, 330.0), 0.4).events;
    rest.extend(t.ended(&mut p, &z, 2, at(450.0, 330.0), 0.4).events);
    assert!(
        rest.iter().all(|e| matches!(e, InputEvent::PreciseScroll { wheel_v: 0, wheel_h: 0, .. })),
        "no click after a scroll, only its end: {rest:?}"
    );
}

#[test]
fn a_finger_left_over_after_two_does_not_move_the_pointer() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Trackpad, false);
    p.move_to(at(100.0, 100.0));
    t.began(&mut p, &z, 1, at(400.0, 300.0), 0.0);
    t.began(&mut p, &z, 2, at(450.0, 300.0), 0.0);
    t.ended(&mut p, &z, 2, at(450.0, 300.0), 0.5);
    assert!(t.moved(&mut p, &z, 1, at(480.0, 300.0), 0.6).events.is_empty());
    assert!(t.ended(&mut p, &z, 1, at(480.0, 300.0), 0.7).events.is_empty());
    // The next single finger works again.
    t.began(&mut p, &z, 3, at(400.0, 300.0), 1.0);
    assert!(!t.moved(&mut p, &z, 3, at(440.0, 300.0), 1.1).events.is_empty());
}

#[test]
fn pinch_zooms_only_when_enabled() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Direct, true);
    t.began(&mut p, &z, 1, at(400.0, 250.0), 0.0);
    t.began(&mut p, &z, 2, at(600.0, 250.0), 0.0);
    t.moved(&mut p, &z, 1, at(350.0, 250.0), 0.05);
    let out = t.moved(&mut p, &z, 2, at(650.0, 250.0), 0.05);
    let change = out.zoom.expect("a zoom step");
    assert!(out.events.is_empty());
    assert_eq!(change.focus, at(500.0, 250.0));
    assert!((change.factor - 300.0 / 200.0).abs() < 1e-9 || change.factor > 1.0, "{change:?}");

    let mut t = Touches::new(Mode::Direct, false);
    let (mut p, z) = (pointer(), zoom());
    t.began(&mut p, &z, 1, at(400.0, 250.0), 0.0);
    t.began(&mut p, &z, 2, at(600.0, 250.0), 0.0);
    t.moved(&mut p, &z, 1, at(350.0, 250.0), 0.05);
    assert!(t.moved(&mut p, &z, 2, at(650.0, 250.0), 0.05).zoom.is_none(), "iPad: no local zoom");
}

// ---- direct mode ---------------------------------------------------------------------------

#[test]
fn direct_tap_clicks_where_you_touch() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Direct, false);
    assert_eq!(t.began(&mut p, &z, 1, at(250.0, 125.0), 0.0).events, vec![mv(500, 250)]);
    assert_eq!(t.ended(&mut p, &z, 1, at(251.0, 125.0), 0.1).events, click(MouseButton::Left, 500, 250));
}

#[test]
fn direct_drag_holds_the_button_from_where_it_started() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Direct, false);
    t.began(&mut p, &z, 1, at(250.0, 125.0), 0.0);
    assert_eq!(
        t.moved(&mut p, &z, 1, at(280.0, 125.0), 0.1).events,
        vec![InputEvent::MouseButton { button: MouseButton::Left, down: true, x: 500, y: 250 }, mv(560, 250)]
    );
    assert_eq!(
        t.ended(&mut p, &z, 1, at(280.0, 125.0), 0.3).events,
        vec![InputEvent::MouseButton { button: MouseButton::Left, down: false, x: 560, y: 250 }]
    );
}

#[test]
fn direct_long_press_is_a_right_click() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Direct, false);
    t.began(&mut p, &z, 1, at(250.0, 125.0), 0.0);
    assert_eq!(t.ended(&mut p, &z, 1, at(250.0, 125.0), 0.8).events, click(MouseButton::Right, 500, 250));
}

#[test]
fn direct_taps_map_through_the_zoom() {
    let mut p = pointer();
    let mut z = zoom();
    z.apply(ZoomChange { focus: at(0.0, 0.0), factor: 2.0, pan: at(0.0, 0.0) });
    let mut t = Touches::new(Mode::Direct, true);
    // Screen (500, 250) at 2× from the top-left corner is view (250, 125) = desktop (500, 250).
    assert_eq!(t.began(&mut p, &z, 1, at(500.0, 250.0), 0.0).events, vec![mv(500, 250)]);
}

#[test]
fn cancel_releases_a_held_button() {
    let (mut p, z) = (pointer(), zoom());
    let mut t = Touches::new(Mode::Direct, false);
    t.began(&mut p, &z, 1, at(250.0, 125.0), 0.0);
    t.moved(&mut p, &z, 1, at(280.0, 125.0), 0.1);
    assert_eq!(
        t.cancelled(&mut p, 1).events,
        vec![InputEvent::MouseButton { button: MouseButton::Left, down: false, x: 560, y: 250 }]
    );
}

#[test]
fn mode_can_change_between_gestures() {
    let mut t = Touches::new(Mode::Trackpad, false);
    t.set_mode(Mode::Direct);
    assert_eq!(t.mode(), Mode::Direct);
}
