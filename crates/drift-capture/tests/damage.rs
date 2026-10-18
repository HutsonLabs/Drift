//! HOST-3 Red: the pure half of capture — frame status, dirty rects in output pixels, damage
//! coalescing, display selection, cursor change detection, configuration.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_capture::config::{CaptureConfig, CaptureFormat};
use drift_capture::cursor::{CursorEvent, CursorSample, CursorShape, CursorTracker};
use drift_capture::display::{DisplayChoice, DisplayInfo, select};
use drift_capture::frame::{FrameMeta, FrameStatus, damage};
use drift_capture::rect::{Damage, PixelRect, coalesce};
use drift_core::{Point, Size};

const BOUNDS: Size<u32> = Size { width: 1000, height: 800 };

fn r(x: u32, y: u32, w: u32, h: u32) -> PixelRect {
    PixelRect { x, y, width: w, height: h }
}

#[test]
fn frame_status_follows_sc_frame_status() {
    assert_eq!(FrameStatus::from_raw(0), Some(FrameStatus::Complete));
    assert_eq!(FrameStatus::from_raw(1), Some(FrameStatus::Idle));
    assert_eq!(FrameStatus::from_raw(2), Some(FrameStatus::Blank));
    assert_eq!(FrameStatus::from_raw(3), Some(FrameStatus::Suspended));
    assert_eq!(FrameStatus::from_raw(4), Some(FrameStatus::Started));
    assert_eq!(FrameStatus::from_raw(5), Some(FrameStatus::Stopped));
    assert_eq!(FrameStatus::from_raw(9), None);
    assert!(FrameStatus::Complete.has_pixels());
    assert!(FrameStatus::Started.has_pixels());
    for s in [FrameStatus::Idle, FrameStatus::Blank, FrameStatus::Suspended, FrameStatus::Stopped] {
        assert!(!s.has_pixels(), "{s:?}");
    }
}

#[test]
fn from_frame_coords_rounds_outward_scales_and_clips() {
    assert_eq!(PixelRect::from_frame_coords(10.4, 20.6, 5.2, 5.0, 1.0, BOUNDS), Some(r(10, 20, 6, 6)));
    assert_eq!(PixelRect::from_frame_coords(10.0, 10.0, 5.0, 5.0, 2.0, BOUNDS), Some(r(20, 20, 10, 10)));
    assert_eq!(
        PixelRect::from_frame_coords(990.0, 790.0, 50.0, 50.0, 1.0, BOUNDS),
        Some(r(990, 790, 10, 10))
    );
    assert_eq!(PixelRect::from_frame_coords(-5.0, -5.0, 10.0, 10.0, 1.0, BOUNDS), Some(r(0, 0, 5, 5)));
    assert_eq!(PixelRect::from_frame_coords(2000.0, 0.0, 10.0, 10.0, 1.0, BOUNDS), None);
    assert_eq!(PixelRect::from_frame_coords(0.0, 0.0, 0.0, 10.0, 1.0, BOUNDS), None);
    assert_eq!(PixelRect::from_frame_coords(f64::NAN, 0.0, 10.0, 10.0, 1.0, BOUNDS), None);
}

#[test]
fn coalesce_merges_overlaps_and_touching_rects() {
    let d = coalesce(&[r(0, 0, 10, 10), r(5, 5, 10, 10), r(100, 100, 10, 10)], BOUNDS, 16);
    assert_eq!(d, Damage::Rects(vec![r(0, 0, 15, 15), r(100, 100, 10, 10)]));
    let d = coalesce(&[r(0, 0, 10, 10), r(10, 0, 10, 10)], BOUNDS, 16);
    assert_eq!(d, Damage::Rects(vec![r(0, 0, 20, 10)]), "edge-adjacent rects merge");
    assert_eq!(coalesce(&[], BOUNDS, 16), Damage::None);
    assert_eq!(coalesce(&[r(0, 0, 0, 0)], BOUNDS, 16), Damage::None);
}

#[test]
fn coalesce_falls_back_to_a_bounding_box_or_the_full_frame() {
    let many: Vec<PixelRect> = (0..20).map(|i| r(i * 20, 0, 5, 5)).collect();
    assert_eq!(coalesce(&many, BOUNDS, 8), Damage::Rects(vec![r(0, 0, 385, 5)]));
    assert_eq!(
        coalesce(&[r(0, 0, 900, 500)], BOUNDS, 16),
        Damage::Full,
        "over half the frame is a full update"
    );
    assert_eq!(coalesce(&[r(0, 0, 2000, 2000)], BOUNDS, 16), Damage::Full);
}

#[test]
fn frame_damage_uses_dirty_rects_only_for_complete_frames() {
    let meta = FrameMeta { status: FrameStatus::Complete, scale: 1.0, dirty: vec![(10.0, 10.0, 20.0, 20.0)] };
    assert_eq!(damage(&meta, BOUNDS, 16), Damage::Rects(vec![r(10, 10, 20, 20)]));
    let started = FrameMeta { status: FrameStatus::Started, ..meta.clone() };
    assert_eq!(damage(&started, BOUNDS, 16), Damage::Full, "the first frame is a full update");
    let idle = FrameMeta { status: FrameStatus::Idle, ..meta.clone() };
    assert_eq!(damage(&idle, BOUNDS, 16), Damage::None);
    let unknown = FrameMeta { dirty: Vec::new(), ..meta };
    assert_eq!(damage(&unknown, BOUNDS, 16), Damage::Full, "a complete frame without dirty rects is full");
}

fn display(id: u32, w: u32, h: u32, scale: u32) -> DisplayInfo {
    DisplayInfo { id, origin: (0.0, 0.0), points: Size::new(w, h), pixels: Size::new(w * scale, h * scale) }
}

#[test]
fn display_selection_prefers_the_requested_id_then_main() {
    let displays = [display(7, 1512, 982, 2), display(9, 2560, 1440, 1)];
    assert_eq!(select(&displays, DisplayChoice::Main, 9).map(|d| d.id), Some(9));
    assert_eq!(select(&displays, DisplayChoice::Id(7), 9).map(|d| d.id), Some(7));
    assert_eq!(select(&displays, DisplayChoice::Id(42), 9).map(|d| d.id), None);
    assert_eq!(select(&displays, DisplayChoice::Main, 42).map(|d| d.id), Some(7), "no main: the first one");
    assert_eq!(select(&[], DisplayChoice::Main, 1), None);
    assert_eq!(displays[0].scale(), 2.0);
}

#[test]
fn cursor_positions_map_to_display_pixels() {
    let d = DisplayInfo {
        id: 1,
        origin: (100.0, 50.0),
        points: Size::new(800, 600),
        pixels: Size::new(1600, 1200),
    };
    assert_eq!(d.point_to_pixels(100.0, 50.0), Some(Point::new(0, 0)));
    assert_eq!(d.point_to_pixels(500.5, 350.25), Some(Point::new(801, 600)));
    assert_eq!(d.point_to_pixels(99.0, 50.0), None, "left of the display");
    assert_eq!(d.point_to_pixels(900.0, 50.0), None, "right edge is outside");
}

fn shape(seed: u8) -> CursorShape {
    CursorShape::new(Size::new(2, 2), Point::new(1, 1), vec![seed; 16]).unwrap()
}

#[test]
fn cursor_tracker_reports_only_changes() {
    let mut t = CursorTracker::default();
    let s = shape(1);
    let first = t.update(&CursorSample { position: Some(Point::new(5, 5)), shape: Some(s.clone()) });
    assert_eq!(first, vec![CursorEvent::Shape(s.clone()), CursorEvent::Moved(Point::new(5, 5))]);
    assert!(t.update(&CursorSample { position: Some(Point::new(5, 5)), shape: Some(s.clone()) }).is_empty());
    assert_eq!(
        t.update(&CursorSample { position: Some(Point::new(6, 5)), shape: Some(s.clone()) }),
        vec![CursorEvent::Moved(Point::new(6, 5))]
    );
    let s2 = shape(2);
    assert_eq!(
        t.update(&CursorSample { position: Some(Point::new(6, 5)), shape: Some(s2.clone()) }),
        vec![CursorEvent::Shape(s2.clone())]
    );
    assert_eq!(
        t.update(&CursorSample { position: None, shape: Some(s2.clone()) }),
        vec![CursorEvent::Hidden]
    );
    assert_eq!(
        t.update(&CursorSample { position: Some(Point::new(6, 5)), shape: Some(s2) }),
        vec![CursorEvent::Moved(Point::new(6, 5))],
        "back on this display"
    );
}

#[test]
fn cursor_shapes_validate_and_compare_by_content() {
    assert!(CursorShape::new(Size::new(2, 2), Point::new(0, 0), vec![0; 15]).is_none(), "not w*h*4 bytes");
    assert!(CursorShape::new(Size::new(2, 2), Point::new(2, 0), vec![0; 16]).is_none(), "hotspot outside");
    assert!(CursorShape::new(Size::new(0, 2), Point::new(0, 0), Vec::new()).is_none());
    assert_eq!(shape(3), shape(3));
    assert_ne!(shape(3).key(), shape(4).key());
}

#[test]
fn capture_config_defaults_and_validation() {
    let c = CaptureConfig::default();
    assert_eq!(c.display, DisplayChoice::Main);
    assert_eq!(c.format, CaptureFormat::Nv12FullRange);
    assert_eq!(c.max_fps, 60);
    assert_eq!(c.min_frame_interval(), (1, 60));
    assert_eq!(CaptureFormat::Nv12FullRange.fourcc(), u32::from_be_bytes(*b"420f"));
    assert_eq!(CaptureFormat::Bgra.fourcc(), u32::from_be_bytes(*b"BGRA"));
    assert!(c.validate().is_ok());
    assert!(CaptureConfig { max_fps: 0, ..c.clone() }.validate().is_err());
    assert!(CaptureConfig { max_fps: 241, ..c.clone() }.validate().is_err());
    assert!(CaptureConfig { queue_depth: 1, ..c.clone() }.validate().is_err(), "ScreenCaptureKit needs ≥ 3");
    assert!(CaptureConfig { max_dirty_rects: 0, ..c }.validate().is_err());
}
