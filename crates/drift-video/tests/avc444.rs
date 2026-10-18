//! HOST-4 Red: the AVC444 (v1) split of a YUV444 picture into the main and auxiliary YUV420
//! views (MS-RDPEGFX 3.3.8.3.2), its inverse, and the BGRA → YUV444 conversion (g-r-d's integer
//! BT.709 full-range matrix, which the client's decoder inverts). Pure; runs on every target.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use drift_core::{Nv12Source as _, Size};
use drift_video::avc444::{Yuv444, bgra_to_yuv444, combine, split};
use proptest::prelude::*;

/// A picture whose every chroma sample is distinct enough to locate it after the split.
fn tagged(size: Size<u32>) -> Yuv444 {
    let (w, h) = (size.width as usize, size.height as usize);
    let y = (0..w * h).map(|i| (i % 251) as u8).collect();
    let u = (0..w * h).map(|i| ((i * 7) % 253) as u8).collect();
    let v = (0..w * h).map(|i| ((i * 11 + 5) % 255) as u8).collect();
    Yuv444::new(size, y, u, v).unwrap()
}

#[test]
fn main_view_is_luma_plus_averaged_chroma() {
    let size = Size::new(32, 32);
    let pic = tagged(size);
    let (main, _) = split(&pic).unwrap();
    assert_eq!(main.size(), size);
    assert_eq!(main.y(), pic.y(), "main luma is the full-resolution luma");
    let w = 32usize;
    for (cy, cx) in [(0usize, 0usize), (3, 5), (15, 15)] {
        let at = |p: &[u8], x: usize, y: usize| u32::from(p[y * w + x]);
        let avg = |p: &[u8]| {
            ((at(p, 2 * cx, 2 * cy)
                + at(p, 2 * cx + 1, 2 * cy)
                + at(p, 2 * cx, 2 * cy + 1)
                + at(p, 2 * cx + 1, 2 * cy + 1)
                + 2)
                / 4) as u8
        };
        let uv = main.uv();
        assert_eq!(uv[cy * w + 2 * cx], avg(pic.u()), "U at ({cx},{cy})");
        assert_eq!(uv[cy * w + 2 * cx + 1], avg(pic.v()), "V at ({cx},{cy})");
    }
}

#[test]
fn auxiliary_view_carries_the_other_chroma_samples() {
    let size = Size::new(32, 32);
    let pic = tagged(size);
    let (_, aux) = split(&pic).unwrap();
    let w = 32usize;
    let (u, v) = (pic.u(), pic.v());
    // Luma of the auxiliary view: in each 16-row band, 8 odd rows of U, then 8 odd rows of V.
    for band in 0..2usize {
        for r in 0..8usize {
            let src_row = 2 * (8 * band + r) + 1;
            assert_eq!(&aux.y()[(16 * band + r) * w..][..w], &u[src_row * w..][..w], "band {band} U row {r}");
            assert_eq!(
                &aux.y()[(16 * band + 8 + r) * w..][..w],
                &v[src_row * w..][..w],
                "band {band} V row {r}"
            );
        }
    }
    // Chroma of the auxiliary view: odd columns of the even rows.
    for (cy, cx) in [(0usize, 0usize), (7, 9), (15, 15)] {
        assert_eq!(aux.uv()[cy * w + 2 * cx], u[2 * cy * w + 2 * cx + 1], "aux U ({cx},{cy})");
        assert_eq!(aux.uv()[cy * w + 2 * cx + 1], v[2 * cy * w + 2 * cx + 1], "aux V ({cx},{cy})");
    }
}

#[test]
fn heights_that_are_not_a_multiple_of_16_pad_the_auxiliary_luma_with_grey() {
    let size = Size::new(8, 20);
    let (main, aux) = split(&tagged(size)).unwrap();
    assert_eq!((main.size(), aux.size()), (size, Size::new(8, 32)), "whole 16-row bands");
    // Band 1 holds U rows 2*(8+r)+1 for r = 0..8; only r = 0 and 1 exist (rows 17, 19).
    let w = 8usize;
    assert!(aux.y()[(16 + 2) * w..(16 + 4) * w].iter().all(|&s| s == 128));
}

#[test]
fn odd_sizes_are_rejected() {
    let pic = Yuv444::new(Size::new(3, 4), vec![0; 12], vec![0; 12], vec![0; 12]).unwrap();
    assert!(split(&pic).is_none());
    assert!(Yuv444::new(Size::new(4, 4), vec![0; 15], vec![0; 16], vec![0; 16]).is_none());
}

#[test]
fn combine_inverts_split_exactly_for_flat_chroma() {
    let size = Size::new(16, 16);
    let pic = Yuv444::new(size, vec![90; 256], vec![60; 256], vec![200; 256]).unwrap();
    let (main, aux) = split(&pic).unwrap();
    assert_eq!(combine(&main, &aux).unwrap(), pic);
}

proptest! {
    #[test]
    fn combine_recovers_every_sample_within_two(seed in any::<u64>(), wq in 1u32..6, hq in 1u32..6) {
        let size = Size::new(wq * 4, hq * 6);
        let n = (size.width * size.height) as usize;
        let mut s = seed | 1;
        let mut next = || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; (s >> 24) as u8 };
        let y: Vec<u8> = (0..n).map(|_| next()).collect();
        let u: Vec<u8> = (0..n).map(|_| next()).collect();
        let v: Vec<u8> = (0..n).map(|_| next()).collect();
        let pic = Yuv444::new(size, y, u, v).unwrap();
        let (main, aux) = split(&pic).unwrap();
        let back = combine(&main, &aux).unwrap();
        prop_assert_eq!(back.y(), pic.y());
        for (a, b) in back.u().iter().zip(pic.u()).chain(back.v().iter().zip(pic.v())) {
            prop_assert!(a.abs_diff(*b) <= 2, "{} vs {}", a, b);
        }
    }
}

#[test]
fn bgra_to_yuv444_uses_grds_integer_matrix() {
    // Y = (54R + 183G + 18B) >> 8; U = ((-29R - 99G + 128B) >> 8) + 128; V = ((128R - 116G - 12B) >> 8) + 128
    let px = |b: u8, g: u8, r: u8| [b, g, r, 255u8];
    let mut bgra = Vec::new();
    for p in [px(0, 0, 0), px(255, 255, 255), px(0, 0, 255), px(255, 0, 0), px(10, 200, 30), px(77, 77, 77)] {
        bgra.extend_from_slice(&p);
    }
    // 6 pixels as a 2x3 picture with a padded stride of 9 pixels.
    let stride = 9 * 4;
    let mut padded = vec![0u8; stride * 3];
    for row in 0..3 {
        padded[row * stride..row * stride + 8].copy_from_slice(&bgra[row * 8..row * 8 + 8]);
    }
    let pic = bgra_to_yuv444(&padded, Size::new(2, 3), stride).unwrap();
    let expect = |r: i32, g: i32, b: i32| {
        let c = |v: i32| v.clamp(0, 255) as u8;
        (
            c((54 * r + 183 * g + 18 * b) >> 8),
            c(((-29 * r - 99 * g + 128 * b) >> 8) + 128),
            c(((128 * r - 116 * g - 12 * b) >> 8) + 128),
        )
    };
    let rgb = [(0, 0, 0), (255, 255, 255), (255, 0, 0), (0, 0, 255), (30, 200, 10), (77, 77, 77)];
    for (i, (r, g, b)) in rgb.into_iter().enumerate() {
        assert_eq!((pic.y()[i], pic.u()[i], pic.v()[i]), expect(r, g, b), "pixel {i}");
    }
    assert!(bgra_to_yuv444(&padded, Size::new(2, 4), stride).is_none(), "buffer too short");
}

#[test]
fn filtered_combine_keeps_the_average_unless_the_rebuilt_sample_is_far_off() {
    use drift_video::avc444::{CHROMA_FILTER_THRESHOLD, combine_filtered};
    // One 2x2 chroma block: a small step (kept as the average) and a sharp edge (rebuilt).
    let size = Size::new(2, 2);
    let soft = Yuv444::new(size, vec![50; 4], vec![100, 104, 104, 104], vec![128; 4]).unwrap();
    let (main, aux) = split(&soft).unwrap();
    let back = combine_filtered(&main, &aux, CHROMA_FILTER_THRESHOLD).unwrap();
    assert_eq!(back.u()[0], 103, "within the threshold: the average");
    assert_eq!(back.u()[1..], [104, 104, 104]);
    let edge = Yuv444::new(size, vec![50; 4], vec![240, 20, 20, 20], vec![128; 4]).unwrap();
    let (main, aux) = split(&edge).unwrap();
    assert_eq!(
        combine_filtered(&main, &aux, CHROMA_FILTER_THRESHOLD).unwrap().u()[0],
        240,
        "an edge is rebuilt"
    );
    assert_eq!(combine_filtered(&main, &aux, 0).unwrap(), combine(&main, &aux).unwrap());
}

proptest! {
    #[test]
    fn the_one_pass_split_matches_convert_then_split(seed in any::<u64>(), wq in 1u32..5, hq in 1u32..12, pad in 0usize..3) {
        use drift_video::avc444::split_bgra;
        let size = Size::new(wq * 2, hq * 2);
        let stride = (size.width as usize + pad) * 4;
        let mut s = seed | 1;
        let mut next = || { s ^= s << 13; s ^= s >> 7; s ^= s << 17; (s >> 24) as u8 };
        let bgra: Vec<u8> = (0..stride * size.height as usize).map(|_| next()).collect();
        let expect = split(&bgra_to_yuv444(&bgra, size, stride).unwrap()).unwrap();
        let got = split_bgra(&bgra, size, stride).unwrap();
        prop_assert_eq!(got.0.y(), expect.0.y());
        prop_assert_eq!(got.0.uv(), expect.0.uv());
        prop_assert_eq!(got.1.y(), expect.1.y());
        prop_assert_eq!(got.1.uv(), expect.1.uv());
    }
}
