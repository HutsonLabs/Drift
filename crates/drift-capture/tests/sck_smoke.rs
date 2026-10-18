//! HOST-3 Red: ScreenCaptureKit smoke test. It needs the Screen Recording permission for the
//! process running the tests; without it every test prints why and passes (the permissioned
//! run is a manual step in docs/acceptance.md).
#![cfg(target_os = "macos")]
#![allow(missing_docs, clippy::unwrap_used, clippy::print_stderr)]

use std::sync::mpsc;
use std::time::Duration;

use drift_capture::config::CaptureConfig;
use drift_capture::sck::{CaptureStream, displays, has_permission};

fn permitted(test: &str) -> bool {
    let ok = has_permission();
    if !ok {
        eprintln!("{test}: skipped — this process has no Screen Recording permission");
    }
    ok
}

#[test]
fn lists_displays_with_pixel_sizes() {
    if !permitted("lists_displays_with_pixel_sizes") {
        return;
    }
    let list = displays().unwrap();
    assert!(!list.is_empty());
    for d in &list {
        assert!(d.pixels.width >= d.points.width && d.pixels.height >= d.points.height, "{d:?}");
    }
}

#[test]
fn streams_iosurface_frames_of_the_display_size() {
    if !permitted("streams_iosurface_frames_of_the_display_size") {
        return;
    }
    let (tx, rx) = mpsc::sync_channel(8);
    let stream = CaptureStream::start(CaptureConfig::default(), move |frame| {
        let _ = tx.try_send((frame.size, frame.is_iosurface_backed(), frame.damage.clone()));
    })
    .unwrap();
    let (size, iosurface, damage) = rx.recv_timeout(Duration::from_secs(10)).expect("a first frame");
    assert_eq!(size, stream.display().pixels);
    assert!(iosurface, "frames are IOSurface-backed");
    assert_eq!(damage, drift_capture::rect::Damage::Full, "the first frame is a full update: {damage:?}");
    stream.stop().unwrap();
}

#[test]
fn the_main_display_has_a_pixel_size_without_screencapturekit() {
    let main = drift_capture::sck::main_display();
    assert!(main.points.width > 0 && main.points.height > 0, "{main:?}");
    assert!(main.pixels.width >= main.points.width, "{main:?}");
}

#[test]
fn samples_the_cursor_separately() {
    // Needs no Screen Recording permission: the position is a CGEvent location and the shape
    // comes from NSCursor.
    let sample = drift_capture::cursor_sample::sample(&drift_capture::sck::main_display());
    if let Some(shape) = &sample.shape {
        assert_eq!(shape.rgba().len(), shape.size().width as usize * shape.size().height as usize * 4);
        assert!(shape.rgba().chunks_exact(4).any(|px| px[3] > 0), "the pointer image is not empty");
    }
    eprintln!(
        "cursor sample: position {:?}, shape {:?}",
        sample.position,
        sample.shape.as_ref().map(|s| (s.size(), s.hotspot()))
    );
}
