//! HOST-4 Red: the real-time encoder profile the Mac host streams with — no reordering, low-
//! latency rate control, bounded keyframe interval, keyframes on demand, completion times for
//! latency stats — and the AVC444 encoder (two AVC420 streams) round-tripped through the
//! client's `VtDecoder`. Not behind `recording`: the host needs no AVFoundation.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use drift_core::Size;
use drift_video::annexb::AccessUnit;
use drift_video::avc444::{Avc444Encoder, bgra_to_yuv444, combine};
use drift_video::decode::VtDecoder;
use drift_video::encode::{EncoderConfig, PixelBuffer, VtEncoder, realtime_bitrate};
use drift_video::quality::psnr;

fn bgra_frame(size: Size<u32>, n: u32) -> Vec<u8> {
    let (w, h) = (size.width, size.height);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let in_box = (x + 1000 - (n * 8) % 1000) % 1000 < 96 && y > h / 4 && y < h / 4 + 96;
            let (r, g, b) =
                if in_box { (240, 60, 40) } else { ((x / 3 % 256) as u8, (y / 2 % 256) as u8, 128) };
            out.extend_from_slice(&[b, g, r, 255]);
        }
    }
    out
}

#[test]
fn realtime_config_bounds_latency_and_keyframes() {
    let c = EncoderConfig::realtime(Size::new(3840, 2160), 60);
    // Measured (ADR HOST-4): low-latency rate control can't hold 60 fps at 4K; below that size it
    // has the tightest latency.
    assert!(!c.low_latency, "4K uses the normal rate control");
    assert!(
        EncoderConfig::realtime(Size::new(1920, 1080), 60).low_latency,
        "1080p: low-latency rate control"
    );
    assert_eq!(c.expected_fps, 60);
    assert!(c.max_keyframe_interval <= Duration::from_secs(4), "bounded keyframe interval");
    assert_eq!(c.average_bitrate, realtime_bitrate(Size::new(3840, 2160), 60));
    assert!(!EncoderConfig::new(Size::new(1280, 800)).low_latency, "recording keeps its profile");
}

#[test]
fn realtime_bitrate_scales_with_pixels_and_is_clamped() {
    let b1080 = realtime_bitrate(Size::new(1920, 1080), 60);
    let b4k = realtime_bitrate(Size::new(3840, 2160), 60);
    assert!(b4k > b1080);
    assert!((2_000_000..=80_000_000).contains(&realtime_bitrate(Size::new(64, 64), 30)));
    assert!(realtime_bitrate(Size::new(7680, 4320), 120) <= 80_000_000);
}

#[test]
fn keyframes_come_on_request_and_frames_complete_in_order() {
    let size = Size::new(640, 360);
    let mut enc = VtEncoder::new(EncoderConfig::realtime(size, 60)).unwrap();
    let t0 = Instant::now();
    let mut frames = Vec::new();
    for n in 0..30u32 {
        if n == 20 {
            enc.request_keyframe();
        }
        let buf = PixelBuffer::new_bgra(size).unwrap();
        let px = bgra_frame(size, n);
        buf.fill_bgra(|x, y| {
            let i = ((y * size.width + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        })
        .unwrap();
        frames.extend(enc.encode(&buf, t0 + Duration::from_millis(u64::from(n) * 16)).unwrap());
    }
    frames.extend(enc.flush().unwrap());
    assert_eq!(frames.len(), 30, "one picture per frame");
    let pts: Vec<_> = frames.iter().map(|f| f.pts()).collect();
    assert!(pts.windows(2).all(|w| w[0] < w[1]), "no reordering: output in input order");
    assert!(frames[0].is_keyframe());
    assert!(frames[20].is_keyframe(), "requested keyframe");
    assert!(frames[1..20].iter().all(|f| !f.is_keyframe()));
    assert!(frames.iter().all(|f| f.completed_at() >= t0));
}

#[test]
fn avc444_streams_decode_and_combine_to_the_source() {
    let size = Size::new(640, 368);
    let mut enc = Avc444Encoder::new(size, 60).unwrap();
    let mut main_dec = VtDecoder::new();
    let mut aux_dec = VtDecoder::new();
    let t0 = Instant::now();
    let mut checked = 0;
    for n in 0..12u32 {
        let bgra = bgra_frame(size, n);
        let pictures = enc
            .encode_bgra(&bgra, (size.width * 4) as usize, t0 + Duration::from_millis(u64::from(n) * 16))
            .unwrap();
        for picture in pictures {
            let main =
                main_dec.decode_picture(&picture.main.to_annex_b().unwrap()).unwrap().expect("main picture");
            let aux =
                aux_dec.decode_picture(&picture.aux.to_annex_b().unwrap()).unwrap().expect("aux picture");
            assert!(AccessUnit::from_annex_b(&picture.aux.to_annex_b().unwrap()).is_ok());
            let back = combine(&main.to_planes().unwrap(), &aux.to_planes().unwrap()).unwrap();
            let source = bgra_to_yuv444(
                &bgra_frame(size, u32::try_from(picture.index).unwrap()),
                size,
                (size.width * 4) as usize,
            )
            .unwrap();
            let p_y = psnr(back.y(), source.y()).unwrap();
            let p_u = psnr(back.u(), source.u()).unwrap();
            let p_v = psnr(back.v(), source.v()).unwrap();
            assert!(
                p_y > 35.0 && p_u > 35.0 && p_v > 35.0,
                "frame {}: Y {p_y:.1} U {p_u:.1} V {p_v:.1} dB",
                picture.index
            );
            checked += 1;
        }
    }
    assert!(checked >= 10, "most frames come out before the flush: {checked}");
}

/// EXT-4: a quality step lowers the bit rate of the running session, which keeps encoding.
#[test]
fn the_bit_rate_changes_on_a_running_session() {
    let size = Size::new(640, 360);
    let mut enc = VtEncoder::new(EncoderConfig::realtime(size, 60)).unwrap();
    let t0 = Instant::now();
    let mut frames = Vec::new();
    for n in 0..20u32 {
        if n == 10 {
            enc.set_average_bitrate(1_500_000).unwrap();
        }
        let buf = PixelBuffer::new_bgra(size).unwrap();
        let px = bgra_frame(size, n);
        buf.fill_bgra(|x, y| {
            let i = ((y * size.width + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        })
        .unwrap();
        frames.extend(enc.encode(&buf, t0 + Duration::from_millis(u64::from(n) * 16)).unwrap());
    }
    frames.extend(enc.flush().unwrap());
    assert_eq!(frames.len(), 20);
    assert_eq!(enc.config().average_bitrate, 1_500_000);
    let mut avc444 = Avc444Encoder::new(Size::new(640, 368), 60).unwrap();
    avc444.set_average_bitrate(1_500_000).unwrap();
}
