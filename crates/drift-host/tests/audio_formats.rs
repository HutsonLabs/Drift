//! HOST-11 Red: the host offers 16-bit stereo PCM at 48 and 44.1 kHz and picks the client's
//! format matching its source rate; Drift's client offers the same two.
#![allow(missing_docs)]

use drift_core::audio::PcmFormat;
use drift_host::audio::{choose_format, host_formats};

#[test]
fn host_offers_pcm_and_picks_the_source_rate() {
    let offered = host_formats();
    assert_eq!(offered, [PcmFormat::STEREO_48K_S16, PcmFormat::STEREO_44K1_S16]);
    assert_eq!(choose_format(&offered, 48_000), Some(0));
    assert_eq!(choose_format(&offered, 44_100), Some(1));
    // Only 44.1 in common: still play (the source is resampled by ScreenCaptureKit's config).
    assert_eq!(choose_format(&[PcmFormat::STEREO_44K1_S16], 48_000), Some(0));
    assert_eq!(choose_format(&[], 48_000), None);
}

#[test]
fn the_client_offers_the_same_pcm_formats() {
    assert_eq!(drift_rdp::audio::client_formats(), host_formats());
}
