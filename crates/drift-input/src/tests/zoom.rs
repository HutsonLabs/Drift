use super::*;

fn zoom() -> Zoom {
    let mut z = Zoom::default();
    z.set_view(Size::new(400.0, 300.0));
    z
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn pinching_keeps_the_focus_point_still() {
    let mut z = zoom();
    let focus = Point::new(100.0, 60.0);
    let before = z.to_view(focus);
    z.apply(ZoomChange { focus, factor: 2.0, pan: Point::new(0.0, 0.0) });
    assert!(close(z.scale, 2.0));
    let after = z.to_view(focus);
    assert!(close(before.x, after.x) && close(before.y, after.y), "{before:?} → {after:?}");
    // A screen point maps through the zoom: half the distance from the focus.
    let p = z.to_view(Point::new(200.0, 60.0));
    assert!(close(p.x, 150.0) && close(p.y, 60.0), "{p:?}");
}

#[test]
fn scale_is_clamped_between_one_and_max() {
    let mut z = zoom();
    z.apply(ZoomChange { focus: Point::new(0.0, 0.0), factor: 100.0, pan: Point::new(0.0, 0.0) });
    assert!(close(z.scale, Zoom::MAX_SCALE));
    z.apply(ZoomChange { focus: Point::new(0.0, 0.0), factor: 0.001, pan: Point::new(0.0, 0.0) });
    assert!(close(z.scale, 1.0));
    assert_eq!(z.offset, Point::new(0.0, 0.0), "at 1× nothing is offset");
}

#[test]
fn panning_moves_the_region_and_stays_inside_the_picture() {
    let mut z = zoom();
    z.apply(ZoomChange { focus: Point::new(0.0, 0.0), factor: 2.0, pan: Point::new(0.0, 0.0) });
    // Fingers move left by 40 screen points: the region moves right by 20 view points.
    z.apply(ZoomChange { focus: Point::new(0.0, 0.0), factor: 1.0, pan: Point::new(-40.0, 0.0) });
    assert!(close(z.offset.x, 20.0), "{:?}", z.offset);
    z.apply(ZoomChange { focus: Point::new(0.0, 0.0), factor: 1.0, pan: Point::new(-10_000.0, -10_000.0) });
    // At 2× the visible region is 200×150 of 400×300.
    assert!(close(z.offset.x, 200.0) && close(z.offset.y, 150.0), "{:?}", z.offset);
    z.apply(ZoomChange { focus: Point::new(0.0, 0.0), factor: 1.0, pan: Point::new(10_000.0, 10_000.0) });
    assert_eq!(z.offset, Point::new(0.0, 0.0));
}

#[test]
fn transform_draws_the_offset_region_full_screen() {
    let mut z = zoom();
    assert_eq!(z.transform(), (1.0, 0.0, 0.0));
    z.apply(ZoomChange { focus: Point::new(0.0, 0.0), factor: 2.0, pan: Point::new(-40.0, -20.0) });
    let (s, tx, ty) = z.transform();
    // A centre-anchored transform puts view point p at c + s(p - c) + t, which must equal
    // s(p - offset) for every p.
    let (cx, cy) = (200.0, 150.0);
    for p in [Point::new(0.0, 0.0), Point::new(123.0, 45.0)] {
        let x = cx + s * (p.x - cx) + tx;
        let y = cy + s * (p.y - cy) + ty;
        assert!(close(x, s * (p.x - z.offset.x)) && close(y, s * (p.y - z.offset.y)), "{p:?}");
    }
}

#[test]
fn reset_and_resize_go_back_inside() {
    let mut z = zoom();
    z.apply(ZoomChange { focus: Point::new(400.0, 300.0), factor: 3.0, pan: Point::new(0.0, 0.0) });
    z.set_view(Size::new(100.0, 100.0));
    assert!(z.offset.x <= 100.0 - 100.0 / z.scale + 1e-9, "{:?}", z.offset);
    z.reset();
    assert_eq!((z.scale, z.offset), (1.0, Point::new(0.0, 0.0)));
}
