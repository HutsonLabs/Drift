//! HOST-11 Red: PCM formats and the playout latency budget shared by the host and the client.
#![allow(missing_docs)]

use std::time::Duration;

use drift_core::audio::{Admit, PcmFormat, PlayoutBudget};

#[test]
fn pcm_format_sizes() {
    let f = PcmFormat::STEREO_48K_S16;
    assert_eq!((f.sample_rate, f.channels, f.bits_per_sample), (48_000, 2, 16));
    assert_eq!(f.bytes_per_frame(), 4);
    assert_eq!(f.bytes_per_second(), 192_000);
    assert_eq!(f.duration_of(19_200), Duration::from_millis(100));
    assert_eq!(f.duration_of(3), Duration::ZERO, "partial frames count as nothing");
    assert_eq!(PcmFormat::STEREO_44K1_S16.bytes_per_second(), 176_400);
}

#[test]
fn playout_admits_until_the_backlog_reaches_the_cap_then_drops() {
    let budget = PlayoutBudget::default();
    assert_eq!(budget.max_backlog, Duration::from_millis(200));
    assert_eq!(budget.admit(Duration::ZERO, Duration::from_millis(20)), Admit::Play);
    assert_eq!(budget.admit(Duration::from_millis(180), Duration::from_millis(20)), Admit::Play);
    assert_eq!(budget.admit(Duration::from_millis(181), Duration::from_millis(20)), Admit::Drop);
    // A chunk larger than the whole budget still plays into an empty queue (never starve).
    assert_eq!(budget.admit(Duration::ZERO, Duration::from_millis(500)), Admit::Play);
}
