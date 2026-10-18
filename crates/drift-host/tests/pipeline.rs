//! HOST-pipeline Red: the pure parts of the host's live picture, input injection and pointer.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use drift_capture::cursor::{CursorEvent, CursorShape};
use drift_capture::rect::{Damage, PixelRect};
use drift_core::{Point, Size};
use drift_host::inject::{InjectingInput, mouse_to_fast_path, to_fast_path};
use drift_host::input::InputSink as _;
use drift_host::pipeline::{Step, decide, regions_for};
use drift_host::pointer::{PointerCache, pointer_update};
use drift_inject::plan::{DisplayMap, InjectEvent};
use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
use ironrdp_server::{DisplayUpdate, KeyboardEvent, MouseButton, MouseEvent};

const SIZE: Size<u32> = Size::new(1920, 1080);

#[test]
fn full_damage_is_one_region_covering_the_frame() {
    let regions = regions_for(&Damage::Full, SIZE);
    assert_eq!(regions.len(), 1);
    let r = &regions[0];
    assert_eq!((r.left, r.top, r.right, r.bottom), (0, 0, 1920, 1080));
}

#[test]
fn rect_damage_becomes_clipped_regions_and_none_is_empty() {
    let damage = Damage::Rects(vec![
        PixelRect { x: 10, y: 20, width: 30, height: 40 },
        PixelRect { x: 1900, y: 1070, width: 100, height: 100 },
    ]);
    let regions = regions_for(&damage, SIZE);
    let edges: Vec<_> = regions.iter().map(|r| (r.left, r.top, r.right, r.bottom)).collect();
    assert_eq!(edges, vec![(10, 20, 40, 60), (1900, 1070, 1920, 1080)]);
    assert!(regions_for(&Damage::None, SIZE).is_empty());
}

#[test]
fn the_pump_waits_under_backpressure_and_encodes_damage_or_a_due_keyframe() {
    assert_eq!(decide(true, true, true), Step::Wait, "never encode a frame the server would drop");
    assert_eq!(decide(false, false, false), Step::Wait);
    assert_eq!(decide(false, true, false), Step::Encode { keyframe: false });
    assert_eq!(decide(false, false, true), Step::Encode { keyframe: true });
    assert_eq!(decide(false, true, true), Step::Encode { keyframe: true });
}

#[test]
fn server_keyboard_events_become_fast_path_events() {
    assert!(matches!(
        to_fast_path(&KeyboardEvent::Pressed { code: 0x1E, extended: false }),
        Some(FastPathInputEvent::KeyboardEvent(f, 0x1E)) if f.is_empty()
    ));
    assert!(matches!(
        to_fast_path(&KeyboardEvent::Released { code: 0x4B, extended: true }),
        Some(FastPathInputEvent::KeyboardEvent(f, 0x4B))
            if f.contains(KeyboardFlags::RELEASE) && f.contains(KeyboardFlags::EXTENDED)
    ));
    assert!(matches!(
        to_fast_path(&KeyboardEvent::UnicodePressed(0x00E9)),
        Some(FastPathInputEvent::UnicodeKeyboardEvent(f, 0x00E9)) if f.is_empty()
    ));
}

#[test]
fn server_mouse_events_become_fast_path_events() {
    assert!(matches!(
        mouse_to_fast_path(&MouseEvent::Move { x: 5, y: 6 }),
        Some(FastPathInputEvent::MouseEvent(p)) if p.x_position == 5 && p.y_position == 6
    ));
    assert!(matches!(
        mouse_to_fast_path(&MouseEvent::Button { x: 1, y: 2, button: MouseButton::X1, pressed: true }),
        Some(FastPathInputEvent::MouseEventEx(_))
    ));
    assert!(matches!(
        mouse_to_fast_path(&MouseEvent::VerticalScroll { value: -120 }),
        Some(FastPathInputEvent::MouseEvent(p)) if p.number_of_wheel_rotation_units == -120
    ));
}

#[test]
fn injecting_input_maps_desktop_pixels_to_display_points_and_posts_events() {
    let map = DisplayMap { origin: (100.0, 0.0), desktop: (2880, 1800), points_per_pixel: 0.5 };
    let posted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let p = posted.clone();
    let mut sink = InjectingInput::new(map, move |e: &InjectEvent| p.lock().unwrap().push(e.clone()));
    sink.mouse(MouseEvent::Move { x: 200, y: 100 });
    sink.keyboard(KeyboardEvent::Pressed { code: 0x1E, extended: false });
    sink.keyboard(KeyboardEvent::Released { code: 0x1E, extended: false });
    let posted = posted.lock().unwrap();
    assert!(
        matches!(posted[0], InjectEvent::Move { x, y, .. } if (x - 200.0).abs() < 1e-9 && (y - 50.0).abs() < 1e-9),
        "{posted:?}"
    );
    assert!(matches!(posted[1], InjectEvent::Key { kvk: 0x00, down: true, .. }), "{posted:?}");
    assert!(matches!(posted[2], InjectEvent::Key { kvk: 0x00, down: false, .. }), "{posted:?}");
}

#[test]
fn ending_an_injecting_session_releases_what_is_held() {
    let map = DisplayMap { origin: (0.0, 0.0), desktop: (100, 100), points_per_pixel: 1.0 };
    let posted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let p = posted.clone();
    let mut sink = InjectingInput::new(map, move |e: &InjectEvent| p.lock().unwrap().push(e.clone()));
    sink.keyboard(KeyboardEvent::Pressed { code: 0x2A, extended: false });
    drop(sink);
    let posted = posted.lock().unwrap();
    assert!(matches!(posted.last(), Some(InjectEvent::Key { down: false, .. })), "{posted:?}");
}

#[test]
fn pointer_events_become_display_updates_with_cached_shapes() {
    let mut cache = PointerCache::default();
    assert!(matches!(
        pointer_update(&CursorEvent::Moved(Point::new(7, 9)), &mut cache),
        Some(DisplayUpdate::PointerPosition(p)) if p.x == 7 && p.y == 9
    ));
    assert!(matches!(pointer_update(&CursorEvent::Hidden, &mut cache), Some(DisplayUpdate::HidePointer)));
    let shape = CursorShape::new(Size::new(2, 2), Point::new(1, 1), vec![255; 16]).unwrap();
    let first = pointer_update(&CursorEvent::Shape(shape.clone()), &mut cache);
    assert!(matches!(first, Some(DisplayUpdate::RGBAPointer(ref p)) if p.width == 2 && p.hot_x == 1));
    assert!(
        matches!(
            pointer_update(&CursorEvent::Shape(shape), &mut cache),
            Some(DisplayUpdate::CachedPointer(_))
        ),
        "a shape seen before goes by its cache slot"
    );
    let big = CursorShape::new(Size::new(128, 128), Point::new(0, 0), vec![0; 128 * 128 * 4]).unwrap();
    assert!(matches!(
        pointer_update(&CursorEvent::Shape(big), &mut cache),
        Some(DisplayUpdate::LargePointer(_))
    ));
}
