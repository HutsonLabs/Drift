//! HOST-11: the `AVAudioEngine` player takes PCM and stops cleanly. It plays silence only, and a
//! machine without an audio output (some CI runners) may refuse to start: that is reported, not
//! a failure.
#![allow(missing_docs)]

use drift_apple::audio_out::Player;
use drift_core::audio::PcmFormat;

#[test]
fn player_takes_silence_and_stops() {
    let player = Player::new();
    match player.start(PcmFormat::STEREO_48K_S16) {
        Ok(()) => {
            // 50 ms chunks of silence: four fit the 200 ms budget, the rest are dropped.
            for _ in 0..8 {
                player.play(&vec![0_u8; 48_000 * 4 / 20]);
            }
            assert!(player.dropped() >= 1, "the latency cap dropped the overflow");
            player.stop();
        }
        Err(e) => eprintln!("skipped: no audio output here ({e})"),
    }
    // Playing before start or after stop is a no-op.
    player.play(&[0; 64]);
}

#[test]
fn unsupported_formats_are_refused() {
    let player = Player::new();
    let f = PcmFormat { sample_rate: 48_000, channels: 6, bits_per_sample: 16 };
    assert!(player.start(f).is_err());
    let f = PcmFormat { sample_rate: 48_000, channels: 2, bits_per_sample: 24 };
    assert!(player.start(f).is_err());
}
