//! M1-4 Red: replaying real g-r-d 50.2 RFX Progressive captures (caps `[V8_1{}]`, i.e.
//! `v81noavc`) through drift-codec reproduces the reference image.
//!
//! The captures are raw GFX DVC payloads (`fixtures/gfx/*.gfx`, DRFTGFX1); the goldens are
//! the images IronRDP's `GraphicsPipelineClient` compositor produced from the very same
//! bytes at capture time (provenance in `docs/adr/M1-4-cpu-codecs.md`).
#![allow(clippy::unwrap_used)]

mod support;

use drift_core::Size;
use ironrdp_graphics::progressive::ProgressiveDecoder;
use support::{load_png_rgba, progressive_payloads, psnr_bgra_vs_rgba, read_fixture, replay_capture};

const MIN_PSNR_DB: f64 = 45.0;

fn assert_replay_matches(capture: &str, golden: &str, threads: usize) {
    let capture = read_fixture(capture);
    let (replay, size, bgra) = replay_capture(&capture, threads);
    let (golden_size, rgba) = load_png_rgba(&read_fixture(golden));
    assert_eq!(size, golden_size, "output size");
    assert_eq!(size, Size::new(1280, 800));
    assert!(replay.tiles > 0, "the capture must contain progressive tiles");
    assert!(replay.frames > 0, "the capture must contain complete frames");
    let psnr = psnr_bgra_vs_rgba(&bgra, &rgba);
    eprintln!("{golden}: PSNR {psnr:.2} dB over {} tiles, {} frames", replay.tiles, replay.frames);
    assert!(psnr >= MIN_PSNR_DB, "PSNR {psnr:.2} dB < {MIN_PSNR_DB} dB");
}

#[test]
fn greeter_progressive_replay_matches_golden() {
    assert_replay_matches("gfx/greeter_v81noavc.gfx", "goldens/greeter_progressive.png", 4);
}

#[test]
fn headless_desktop_progressive_replay_matches_golden() {
    assert_replay_matches("gfx/headless_v81noavc.gfx", "goldens/headless_progressive.png", 4);
}

#[test]
fn replay_is_independent_of_pool_size() {
    let capture = read_fixture("gfx/greeter_v81noavc.gfx");
    let (_, _, single) = replay_capture(&capture, 1);
    let (_, _, many) = replay_capture(&capture, 8);
    assert!(single == many, "1-thread and 8-thread decodes differ");
}

#[test]
fn fixture_payloads_decode_bit_identically_to_ironrdp() {
    for capture in ["gfx/greeter_v81noavc.gfx", "gfx/headless_v81noavc.gfx"] {
        let capture = read_fixture(capture);
        let payloads = progressive_payloads(&capture);
        assert!(!payloads.is_empty());
        let mut ours = drift_codec::ProgressiveCodec::new(drift_codec::TilePool::new(4).unwrap());
        let mut theirs = ProgressiveDecoder::new();
        for (i, (surface, ctx, data)) in payloads.iter().enumerate() {
            let a = ours.decode(*surface, *ctx, Size::new(1280, 800), data).unwrap();
            let b = theirs.decode_bitmap(*surface, *ctx, 1280, 800, data).unwrap();
            assert_eq!(a.len(), b.len(), "payload {i}: tile count");
            for (x, y) in a.iter().zip(&b) {
                assert_eq!((x.origin.x, x.origin.y), (u32::from(y.x_idx) * 64, u32::from(y.y_idx) * 64));
                let rgba: Vec<u8> =
                    x.data.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
                assert!(rgba == y.pixels, "payload {i}: pixels of tile ({}, {})", y.x_idx, y.y_idx);
                assert_eq!(x.update_rects.len(), y.update_rectangles.len());
            }
        }
    }
}
