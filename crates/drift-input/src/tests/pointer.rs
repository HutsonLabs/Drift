use super::*;
use drift_core::Size;

fn view(w: f64, h: f64, scale: f64) -> ViewGeometry {
    ViewGeometry { points: Size::new(w, h), backing_scale: scale }
}

/// A 1000×500-point view showing a 2000×1000 desktop: 0.5 points per pixel, no bars.
fn attached() -> Pointer {
    let mut p = Pointer::new(ScrollConfig::default());
    p.set_view_geometry(view(1000.0, 500.0, 2.0));
    p.set_desktop(DesktopSize { width: 2000, height: 1000 }, ScaleMode::Fit);
    p
}

#[test]
fn absolute_moves_map_through_the_viewport_and_skip_repeats() {
    let mut p = attached();
    assert_eq!(p.move_to(Point::new(100.0, 50.0)), vec![InputEvent::MouseMove { x: 200, y: 100 }]);
    assert_eq!(p.move_to(Point::new(100.2, 50.1)), vec![], "same pixel");
    assert_eq!(p.position(), Some(Point::new(200, 100)));
}

#[test]
fn nothing_is_sent_without_a_desktop() {
    let mut p = Pointer::new(ScrollConfig::default());
    assert!(p.move_to(Point::new(1.0, 1.0)).is_empty());
    assert!(p.move_by(5.0, 5.0).is_empty());
    assert!(p.button(MouseButton::Left, true).is_empty());
    let mut p = attached();
    p.move_to(Point::new(1.0, 1.0));
    p.clear_desktop();
    assert!(p.move_by(5.0, 5.0).is_empty());
    assert_eq!(p.position(), None);
}

#[test]
fn relative_moves_scale_points_to_pixels_and_clamp_to_the_desktop() {
    let mut p = attached();
    p.move_to(Point::new(0.0, 0.0));
    // 10 points right, 5 down at 0.5 points per pixel = 20 px, 10 px.
    assert_eq!(p.move_by(10.0, 5.0), vec![InputEvent::MouseMove { x: 20, y: 10 }]);
    // Fractions accumulate instead of being lost.
    assert_eq!(p.move_by(0.25, 0.0), vec![]);
    assert_eq!(p.move_by(0.25, 0.0), vec![InputEvent::MouseMove { x: 21, y: 10 }]);
    assert_eq!(p.move_by(-5000.0, 9000.0), vec![InputEvent::MouseMove { x: 0, y: 999 }]);
    assert_eq!(p.move_by(-1.0, 1.0), vec![], "pinned at the corner");
}

#[test]
fn relative_moves_start_from_the_desktop_centre() {
    let mut p = attached();
    assert_eq!(p.move_by(0.5, 0.0), vec![InputEvent::MouseMove { x: 1001, y: 500 }]);
}

#[test]
fn buttons_press_at_the_current_position_and_release_once() {
    let mut p = attached();
    p.move_to(Point::new(10.0, 10.0));
    let down = InputEvent::MouseButton { button: MouseButton::Right, down: true, x: 20, y: 20 };
    let up = InputEvent::MouseButton { button: MouseButton::Right, down: false, x: 20, y: 20 };
    assert_eq!(p.button(MouseButton::Right, true), vec![down]);
    assert_eq!(p.button(MouseButton::Right, true), vec![], "already held");
    assert_eq!(p.button(MouseButton::Right, false), vec![up]);
    assert_eq!(p.button(MouseButton::Right, false), vec![], "not held");
}

#[test]
fn release_all_lifts_held_buttons() {
    let mut p = attached();
    p.move_to(Point::new(10.0, 10.0));
    p.button(MouseButton::Left, true);
    p.button(MouseButton::Middle, true);
    assert_eq!(
        p.release_all(),
        vec![
            InputEvent::MouseButton { button: MouseButton::Left, down: false, x: 20, y: 20 },
            InputEvent::MouseButton { button: MouseButton::Middle, down: false, x: 20, y: 20 },
        ]
    );
    assert!(p.release_all().is_empty());
}

#[test]
fn scroll_uses_the_wheel_accumulator() {
    let mut p = attached();
    assert_eq!(
        p.scroll(ScrollDelta::Lines { dx: 0.0, dy: 1.0 }, true),
        vec![InputEvent::Wheel { horizontal: false, units: 120 }]
    );
    assert!(p.scroll(ScrollDelta::Precise { dx: 0.0, dy: 0.2 }, false).is_empty());
    assert_eq!(
        p.scroll(ScrollDelta::Precise { dx: 0.0, dy: 0.3 }, false),
        vec![InputEvent::Wheel { horizontal: false, units: 1 }]
    );
}
