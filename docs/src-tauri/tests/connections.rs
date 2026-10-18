//! UI-windows Red: live thumbnails for the Connections gallery (ADR UI-windows-gallery
//! decision 8): the throttle, the downscale and the PNG data URL.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures

use std::time::{Duration, Instant};

use drift_app::connections::{
    THUMBNAIL_INTERVAL, THUMBNAIL_MAX, Thumbnail, ThumbnailThrottle, base64, downscale, fit, png_data_url,
};
use drift_core::Size;
use uuid::Uuid;

const LIVE: [&str; 2] = ["session-0", "session-1"];

#[test]
fn the_interval_and_size_are_the_adrs() {
    assert_eq!(THUMBNAIL_INTERVAL, Duration::from_secs(15));
    assert_eq!(THUMBNAIL_MAX, Size::new(320, 180));
}

#[test]
fn nothing_is_sampled_while_connections_is_hidden() {
    let t0 = Instant::now();
    let mut throttle = ThumbnailThrottle::default();
    assert!(throttle.due(t0, &LIVE).is_empty(), "hidden at launch until shown");
    throttle.went_live("session-0");
    assert!(throttle.due(t0 + Duration::from_secs(60), &LIVE).is_empty());
    throttle.set_visible(true);
    throttle.set_visible(false);
    assert!(throttle.due(t0 + Duration::from_secs(120), &LIVE).is_empty());
}

#[test]
fn showing_samples_every_live_session_at_once_then_every_interval() {
    let t0 = Instant::now();
    let mut throttle = ThumbnailThrottle::default();
    throttle.set_visible(true);
    assert_eq!(throttle.due(t0, &LIVE), LIVE, "immediately on show");
    assert!(throttle.due(t0 + Duration::from_secs(1), &LIVE).is_empty());
    assert!(throttle.due(t0 + Duration::from_secs(14), &LIVE).is_empty(), "at most one per interval");
    assert_eq!(throttle.due(t0 + Duration::from_secs(15), &LIVE), LIVE);
    assert!(throttle.due(t0 + Duration::from_secs(16), &LIVE).is_empty());

    // Hiding and showing again samples at once.
    throttle.set_visible(false);
    throttle.set_visible(true);
    assert_eq!(throttle.due(t0 + Duration::from_secs(17), &LIVE), LIVE);
    // Only live sessions are sampled.
    assert_eq!(throttle.due(t0 + Duration::from_secs(40), &["session-1"]), ["session-1"]);
}

#[test]
fn a_session_going_live_is_sampled_at_once() {
    let t0 = Instant::now();
    let mut throttle = ThumbnailThrottle::default();
    throttle.set_visible(true);
    assert_eq!(throttle.due(t0, &["session-0"]), ["session-0"]);
    // session-1 goes live 3 s later; session-0 waits for its interval.
    throttle.went_live("session-1");
    assert_eq!(throttle.due(t0 + Duration::from_secs(3), &LIVE), ["session-1"]);
    assert!(throttle.due(t0 + Duration::from_secs(4), &LIVE).is_empty());
    // An ended session is forgotten; when its window reconnects it is sampled at once again.
    throttle.ended("session-1");
    throttle.went_live("session-1");
    assert_eq!(throttle.due(t0 + Duration::from_secs(5), &LIVE), ["session-1"]);
}

#[test]
fn thumbnails_fit_320_by_180() {
    assert_eq!(fit(Size::new(2560, 1600), THUMBNAIL_MAX), Size::new(288, 180));
    assert_eq!(fit(Size::new(1920, 1080), THUMBNAIL_MAX), Size::new(320, 180));
    assert_eq!(fit(Size::new(3840, 1080), THUMBNAIL_MAX), Size::new(320, 90));
    assert_eq!(fit(Size::new(200, 100), THUMBNAIL_MAX), Size::new(200, 100), "never upscaled");
    assert_eq!(fit(Size::new(10_000, 1), THUMBNAIL_MAX), Size::new(320, 1), "at least one pixel");
}

#[test]
fn downscale_box_filters_bgra_into_opaque_rgba() {
    // 4×2 BGRA: left half blue-ish (B=200), right half red (R=100); alpha 0 on the wire.
    let mut bgra = Vec::new();
    for _row in 0..2 {
        for x in 0..4 {
            bgra.extend(if x < 2 { [200, 0, 0, 0] } else { [0, 0, 100, 0] });
        }
    }
    let (rgba, size) = downscale(&bgra, Size::new(4, 2), Size::new(2, 1));
    assert_eq!(size, Size::new(2, 1));
    assert_eq!(rgba, [0, 0, 200, 255, 100, 0, 0, 255]);
    // An average, not a pick: a 2×1 gradient becomes its mean.
    let (rgba, size) = downscale(&[0, 0, 0, 255, 100, 50, 10, 255], Size::new(2, 1), Size::new(1, 1));
    assert_eq!((rgba, size), (vec![5, 25, 50, 255], Size::new(1, 1)));
    // Short input never panics.
    let (rgba, size) = downscale(&[1, 2, 3], Size::new(4, 4), Size::new(2, 2));
    assert_eq!(size, Size::new(0, 0));
    assert!(rgba.is_empty());
}

#[test]
fn base64_matches_rfc_4648() {
    for (raw, want) in [
        (&b""[..], ""),
        (b"f", "Zg=="),
        (b"fo", "Zm8="),
        (b"foo", "Zm9v"),
        (b"foob", "Zm9vYg=="),
        (b"fooba", "Zm9vYmE="),
        (b"foobar", "Zm9vYmFy"),
        (&[0xfb, 0xff, 0xbf], "+/+/"),
    ] {
        assert_eq!(base64(raw), want);
    }
}

#[test]
fn thumbnails_are_png_data_urls() {
    let rgba = [10, 20, 30, 255].repeat(6);
    let url = png_data_url(&rgba, Size::new(3, 2)).unwrap();
    let body = url.strip_prefix("data:image/png;base64,").expect(&url);
    assert!(body.starts_with("iVBORw0KGgo"), "PNG signature: {body}");
    assert_eq!(png_data_url(&rgba, Size::new(4, 4)), None, "wrong length");
}

#[test]
fn a_thumbnail_never_prints_its_pixels() {
    let t = Thumbnail { profile_id: Uuid::nil(), image: Some("data:image/png;base64,SECRETPIXELS".into()) };
    let debug = format!("{t:?}");
    assert!(!debug.contains("SECRETPIXELS"), "{debug}");
}
