//! AVC444 end to end, decode half (ADR `AVC444-client`): `VtAvc444Decoder` decodes the main and
//! auxiliary AVC420 views with two VideoToolbox sessions, recombines them with HOST-4's filtered
//! `combine` and converts the full-chroma picture to BGRA for the compositor.
//!
//! Content: coloured one-pixel lines and text-like strokes on white, where 4:2:0 smears chroma.
//! The pass mark is a chroma PSNR that the luma view alone (what AVC420 would show) misses and
//! the two views together reach.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::time::Instant;

use drift_core::Size;
use drift_core::video::Avc444Decoder;
use drift_video::avc444::{Avc444Encoder, bgra_to_yuv444, yuv444_to_bgra};
use drift_video::decode::VtAvc444Decoder;
use drift_video::quality::psnr;

const W: u32 = 640;
const H: u32 = 368;

/// White page with red, blue and green one-pixel strokes every few pixels (sharp chroma edges).
fn text_like() -> Vec<u8> {
    let mut bgra = vec![255u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            let px: [u8; 4] = match (x % 7, y % 5) {
                (0, _) => [0, 0, 220, 255],               // red vertical strokes
                (3, _) if y % 2 == 0 => [220, 0, 0, 255], // blue dotted strokes
                (_, 2) if x % 3 == 0 => [0, 160, 0, 255], // green horizontal dashes
                _ => [255, 255, 255, 255],
            };
            bgra[i..i + 4].copy_from_slice(&px);
        }
    }
    bgra
}

/// PSNR of the U and V planes (BT.709 full range, g-r-d's matrix) between two BGRA pictures.
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

#[test]
fn yuv444_to_bgra_inverts_the_grd_matrix_within_four() {
    let size = Size::new(16, 8);
    let mut src = Vec::new();
    for i in 0..(16 * 8) {
        let v = (i * 37 % 256) as u8;
        src.extend_from_slice(&[v, v.wrapping_mul(3), 255 - v, 255]);
    }
    let yuv = bgra_to_yuv444(&src, size, 64).unwrap();
    let back = yuv444_to_bgra(&yuv);
    assert_eq!(back.len(), src.len());
    for (i, (a, b)) in src.iter().zip(&back).enumerate() {
        if i % 4 == 3 {
            assert_eq!(*b, 255, "alpha is opaque");
        } else {
            // g-r-d's forward matrix truncates (>> 8) and its luma gains sum to 255/256.
            assert!(a.abs_diff(*b) <= 4, "byte {i}: {a} vs {b}");
        }
    }
}

#[test]
fn both_views_restore_chroma_that_the_luma_view_alone_loses() {
    let src = text_like();
    let mut enc = Avc444Encoder::new(Size::new(W, H), 60).unwrap();
    let mut pics = enc.encode_bgra(&src, (W * 4) as usize, Instant::now()).unwrap();
    pics.extend(enc.flush().unwrap());
    let pic = pics.first().expect("one picture");
    let (main, aux) = (pic.main.to_annex_b().unwrap(), pic.aux.to_annex_b().unwrap());

    let mut full = VtAvc444Decoder::new();
    let both = full.decode(Some(&main), Some(&aux)).unwrap().expect("a picture from both views");
    assert_eq!(both.size(), Size::new(W, H));

    let mut luma_only = VtAvc444Decoder::new();
    let main_only = luma_only.decode(Some(&main), None).unwrap().expect("a picture from the luma view");

    let (p444, p420) = (chroma_psnr(&src, both.data()), chroma_psnr(&src, main_only.data()));
    eprintln!("AVC444 chroma PSNR {p444:.2} dB, luma view alone {p420:.2} dB");
    assert!(p420 < 30.0, "the luma view alone should smear the strokes: {p420:.2} dB");
    assert!(p444 >= 30.0, "both views should keep the strokes: {p444:.2} dB");
    assert!(p444 > p420 + 3.0, "AVC444 {p444:.2} dB vs AVC420 {p420:.2} dB");
}

#[test]
fn a_chroma_only_update_reuses_the_last_luma() {
    let src = text_like();
    let mut enc = Avc444Encoder::new(Size::new(W, H), 60).unwrap();
    let mut pics = enc.encode_bgra(&src, (W * 4) as usize, Instant::now()).unwrap();
    pics.extend(enc.flush().unwrap());
    let pic = &pics[0];
    let mut dec = VtAvc444Decoder::new();
    assert!(
        dec.decode(None, Some(&pic.aux.to_annex_b().unwrap())).unwrap().is_none(),
        "no luma yet: nothing to show"
    );
    let luma = dec.decode(Some(&pic.main.to_annex_b().unwrap()), None).unwrap().unwrap();
    assert_eq!(luma.size(), Size::new(W, H));
}
