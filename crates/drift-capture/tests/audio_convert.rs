//! HOST-11 Red: ScreenCaptureKit audio (Float32, interleaved or planar) → 16-bit interleaved PCM
//! for rdpsnd.
#![allow(missing_docs)]

use drift_capture::audio::{Layout, to_s16_stereo};

fn s16(bytes: &[u8]) -> Vec<i16> {
    bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
}

#[test]
fn interleaved_stereo_float_becomes_s16() {
    let samples = [0.0_f32, 1.0, -1.0, 0.5, 2.0, -3.0];
    let out = to_s16_stereo(&samples, 2, Layout::Interleaved);
    assert_eq!(s16(&out), [0, 32767, -32767, 16384, 32767, -32767], "scaled and clamped");
}

#[test]
fn planar_stereo_float_is_interleaved() {
    // Left plane then right plane, 3 frames.
    let samples = [0.1_f32, 0.2, 0.3, -0.1, -0.2, -0.3];
    let out = s16(&to_s16_stereo(&samples, 2, Layout::Planar));
    assert_eq!(out.len(), 6);
    assert_eq!(out[0], (0.1_f32 * 32767.0).round() as i16);
    assert_eq!(out[1], (-0.1_f32 * 32767.0).round() as i16);
    assert_eq!(out[4], (0.3_f32 * 32767.0).round() as i16);
}

#[test]
fn mono_is_copied_to_both_channels_and_extra_channels_are_dropped() {
    assert_eq!(s16(&to_s16_stereo(&[0.5, -0.5], 1, Layout::Interleaved)), [16384, 16384, -16384, -16384]);
    let quad = [0.5_f32, -0.5, 0.9, 0.9];
    assert_eq!(s16(&to_s16_stereo(&quad, 4, Layout::Interleaved)), [16384, -16384]);
    assert!(to_s16_stereo(&[0.1, 0.2, 0.3], 2, Layout::Interleaved).len() == 4, "a partial frame is dropped");
    assert!(to_s16_stereo(&[], 0, Layout::Planar).is_empty());
}
