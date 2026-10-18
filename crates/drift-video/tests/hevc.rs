//! EXT-5 Red: HEVC through VideoToolbox (ADR `EXT-5-hevc-hdr`) — the encoder's profiles probed,
//! a real-time HEVC encoder (Main, Main 10, 4:4:4) and the client's decoder, round-tripped with a
//! PSNR pass mark, and HDR colour tags (BT.2020 PQ) carried through to the decoded picture.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use drift_core::Size;
use drift_core::video::{DecodedVideo, HevcDecoder, HevcProfile, Transfer, VideoPicture};
use drift_video::avc444::bgra_to_yuv444;
use drift_video::encode::PixelBuffer;
use drift_video::hevc::{HevcConfig, HevcEncoder, VtHevcDecoder, probe_encoder, to_bgra};
use drift_video::quality::psnr;

const W: u32 = 640;
const H: u32 = 368;

/// White page with red, blue and green one-pixel strokes (sharp chroma edges), shifted by `n`.
fn text_like(n: u32) -> Vec<u8> {
    let mut bgra = vec![255u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            let xs = x + n;
            let px: [u8; 4] = match (xs % 7, y % 5) {
                (0, _) => [0, 0, 220, 255],
                (3, _) if y % 2 == 0 => [220, 0, 0, 255],
                (_, 2) if xs.is_multiple_of(3) => [0, 160, 0, 255],
                _ => [255, 255, 255, 255],
            };
            bgra[i..i + 4].copy_from_slice(&px);
        }
    }
    bgra
}

fn buffer(bgra: &[u8]) -> PixelBuffer {
    let buf = PixelBuffer::new_bgra(Size::new(W, H)).unwrap();
    buf.fill_bgra(|x, y| {
        let i = ((y * W + x) * 4) as usize;
        [bgra[i], bgra[i + 1], bgra[i + 2], bgra[i + 3]]
    })
    .unwrap();
    buf
}

/// Encodes `frames` pictures with `profile` and decodes every access unit; returns the last
/// decoded picture as BGRA (converted from NV12 where the decoder gives NV12) and the source.
fn round_trip(profile: HevcProfile, transfer: Transfer, frames: u32) -> (Vec<u8>, Vec<u8>, DecodedVideo) {
    let size = Size::new(W, H);
    let mut enc = HevcEncoder::new(HevcConfig::realtime(size, 30, profile, transfer)).unwrap();
    let mut dec = VtHevcDecoder::new();
    let t0 = Instant::now();
    let mut last = None;
    let mut src = Vec::new();
    for n in 0..frames {
        src = text_like(n);
        let out = enc.encode(&buffer(&src), t0 + Duration::from_millis(u64::from(n) * 33)).unwrap();
        for frame in out.into_iter().chain(if n + 1 == frames { enc.flush().unwrap() } else { vec![] }) {
            assert!(frame.annex_b().starts_with(&[0, 0, 0, 1]), "Annex-B");
            if let Some(picture) = dec.decode(frame.annex_b()).unwrap() {
                last = Some(picture);
            }
        }
    }
    let picture = last.expect("decoded at least one picture");
    (to_bgra(&picture).unwrap(), src, picture)
}

fn chroma_psnr(a: &[u8], b: &[u8]) -> f64 {
    let size = Size::new(W, H);
    let (ya, yb) = (
        bgra_to_yuv444(a, size, (W * 4) as usize).unwrap(),
        bgra_to_yuv444(b, size, (W * 4) as usize).unwrap(),
    );
    let mut ua = ya.u().to_vec();
    ua.extend_from_slice(ya.v());
    let mut ub = yb.u().to_vec();
    ub.extend_from_slice(yb.v());
    psnr(&ua, &ub).unwrap()
}

/// Luma PSNR of the decoded picture's own Y plane against the source's (measuring luma through
/// BGRA would count 4:2:0's chroma smear, which clips RGB, as luma error).
fn luma_psnr(picture: &DecodedVideo, src: &[u8]) -> f64 {
    let size = Size::new(W, H);
    let source = bgra_to_yuv444(src, size, (W * 4) as usize).unwrap();
    let decoded = match &picture.picture {
        VideoPicture::Nv12(frame) => frame
            .downcast_ref::<drift_video::decode::DecodedPicture>()
            .unwrap()
            .to_planes()
            .unwrap()
            .y()
            .to_vec(),
        VideoPicture::Bgra(p) => bgra_to_yuv444(p.data(), size, (W * 4) as usize).unwrap().y().to_vec(),
    };
    psnr(&decoded, source.y()).unwrap()
}

#[test]
fn the_encoder_probe_lists_hevc_main() {
    let profiles = probe_encoder(Size::new(1920, 1080));
    eprintln!("HEVC encoder profiles: {profiles:?}");
    assert!(profiles.contains(&HevcProfile::Main), "every Apple silicon Mac encodes HEVC Main");
}

#[test]
fn hevc_main_round_trips_through_the_decoder() {
    let (decoded, src, picture) = round_trip(HevcProfile::Main, Transfer::Sdr, 8);
    assert!(matches!(picture.picture, VideoPicture::Nv12(_)), "4:2:0 decodes to NV12 for the zero-copy path");
    let (y, c) = (luma_psnr(&picture, &src), chroma_psnr(&decoded, &src));
    eprintln!("HEVC Main: luma {y:.2} dB, chroma {c:.2} dB");
    // Hardware encoders give about 42 dB here (M4 Pro), the iOS simulator's software one about
    // 34 dB at this bit rate; a wrong picture is far below either.
    assert!(y > 32.0, "luma {y:.2} dB");
}

#[test]
fn hevc_444_keeps_the_chroma_that_420_smears() {
    if !probe_encoder(Size::new(W, H)).contains(&HevcProfile::Main444) {
        eprintln!("skipped: this Mac's HEVC encoder has no 4:4:4 profile");
        return;
    }
    let (main, src, _) = round_trip(HevcProfile::Main, Transfer::Sdr, 8);
    let (full, src444, picture) = round_trip(HevcProfile::Main444, Transfer::Sdr, 8);
    assert_eq!(src, src444);
    assert!(matches!(picture.picture, VideoPicture::Bgra(_)), "4:4:4 decodes to BGRA");
    let (c420, c444) = (chroma_psnr(&main, &src), chroma_psnr(&full, &src));
    eprintln!("chroma PSNR: HEVC Main {c420:.2} dB, HEVC 4:4:4 {c444:.2} dB");
    assert!(c444 > c420 + 3.0, "4:4:4 {c444:.2} dB vs 4:2:0 {c420:.2} dB");
}

#[test]
fn main10_pq_tags_reach_the_decoded_picture() {
    if !probe_encoder(Size::new(W, H)).contains(&HevcProfile::Main10) {
        eprintln!("skipped: no HEVC Main 10 encoder");
        return;
    }
    let (_, _, picture) = round_trip(HevcProfile::Main10, Transfer::Pq, 4);
    assert_eq!(picture.transfer, Transfer::Pq, "BT.2100 PQ travels in the bitstream's VUI");
    let (_, _, sdr) = round_trip(HevcProfile::Main, Transfer::Sdr, 2);
    assert_eq!(sdr.transfer, Transfer::Sdr);
}

#[test]
fn keyframes_carry_parameter_sets_and_come_on_request() {
    let size = Size::new(W, H);
    let mut enc = HevcEncoder::new(HevcConfig::realtime(size, 30, HevcProfile::Main, Transfer::Sdr)).unwrap();
    let t0 = Instant::now();
    let mut frames = Vec::new();
    for n in 0..12u32 {
        if n == 8 {
            enc.request_keyframe();
        }
        frames.extend(
            enc.encode(&buffer(&text_like(n)), t0 + Duration::from_millis(u64::from(n) * 33)).unwrap(),
        );
    }
    frames.extend(enc.flush().unwrap());
    assert_eq!(frames.len(), 12, "one picture per frame, no reordering");
    assert!(frames[0].is_keyframe() && frames[8].is_keyframe());
    assert!(frames[1..8].iter().all(|f| !f.is_keyframe()));
    // VPS (32), SPS (33), PPS (34) before the first IRAP picture.
    let types = drift_video::hevc::nal_types(frames[0].annex_b());
    assert!(types.starts_with(&[32, 33, 34]), "{types:?}");
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    let mut dec = VtHevcDecoder::new();
    assert!(matches!(dec.decode(&[0, 0, 0, 1, 0x42, 0x01, 0xFF, 0xFF]), Ok(None) | Err(_)));
    assert!(matches!(dec.decode(&[]), Ok(None) | Err(_)));
    assert!(matches!(dec.decode(&[1, 2, 3]), Ok(None) | Err(_)));
}

/// EXT-4 + EXT-5: a quality step lowers the bit rate of a running HEVC session, which keeps
/// encoding (paired clients get HEVC, so the quality ladder must reach it too).
#[test]
fn the_hevc_bit_rate_changes_on_a_running_session() {
    let size = Size::new(640, 368);
    let mut enc = HevcEncoder::new(HevcConfig::realtime(size, 60, HevcProfile::Main, Transfer::Sdr)).unwrap();
    let t0 = Instant::now();
    let mut frames = Vec::new();
    for n in 0..12u32 {
        if n == 6 {
            enc.set_average_bitrate(1_500_000).unwrap();
        }
        frames.extend(enc.encode(&buffer(&text_like(n)), t0 + Duration::from_millis(u64::from(n) * 16)).unwrap());
    }
    frames.extend(enc.flush().unwrap());
    assert_eq!(frames.len(), 12);
    assert_eq!(enc.config().average_bitrate, 1_500_000);
}
