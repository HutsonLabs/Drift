//! AVC444 end to end, host half (ADR `AVC444-client`): which codec the host sends. AVC444 needs
//! a client that confirmed a V10 capability set with AVC enabled, and a picture small enough for
//! both views at full rate (HOST-4 measured 60 fps up to about 1920×1080 on an M4 Pro).
#![allow(missing_docs)]

use drift_core::Size;
use drift_host::codec::{AVC444_MAX_PIXELS, Codec, choose};

#[test]
fn avc444_for_v10_clients_up_to_the_measured_size() {
    assert_eq!(choose(true, true, Size::new(1920, 1080)), Codec::Avc444);
    assert_eq!(choose(true, true, Size::new(1280, 800)), Codec::Avc444);
    assert_eq!(AVC444_MAX_PIXELS, 1920 * 1080);
}

#[test]
fn avc420_above_the_limit_or_without_avc444() {
    assert_eq!(choose(true, true, Size::new(2560, 1600)), Codec::Avc420);
    assert_eq!(choose(true, false, Size::new(1280, 800)), Codec::Avc420);
    assert_eq!(choose(true, true, Size::new(3840, 2160)), Codec::Avc420);
}

#[test]
fn no_avc_at_all_falls_back_to_the_test_pattern_path() {
    assert_eq!(choose(false, false, Size::new(1280, 800)), Codec::None);
}
