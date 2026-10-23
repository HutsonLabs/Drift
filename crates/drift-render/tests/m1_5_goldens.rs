//! M1-5 Red: offscreen Metal goldens (per-channel tolerance ≤ 2), colour accuracy against
//! g-r-d's integer encoder (plan §1.4), and `presented` exactly once per frame.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use common::*;
use drift_core::{Bgra, Nv12Frame, Point, Rect, Size};
use drift_gfx::FrameSink;
use drift_render::{CpuCompositor, color};
use drift_testkit::golden::assert_golden;

const TOL: u8 = 2;

fn run_scene(name: &str, scene: fn(&mut dyn FrameSink)) {
    let mut cpu = CpuCompositor::new();
    scene(&mut cpu);
    let reference = cpu.composite();

    let mut gpu = offscreen(reference.size);
    scene(&mut gpu);
    let log = PresentLog::default();
    gpu.end_frame(1, log.callback(1));
    gpu.wait_idle();
    let output = gpu.read_output().expect("output after reset");
    assert_golden(&golden_path(name), &g(&reference), &g(&output), TOL);
    assert_eq!(log.ids(), vec![1], "{name}: presented exactly once");
}

#[test]
fn golden_solid_fill() {
    run_scene("solid_fill", scenes::solid_fill);
}

#[test]
fn golden_bgra_blit_at_offset() {
    run_scene("bgra_blit_offset", scenes::bgra_blit_offset);
}

#[test]
fn golden_overlapping_surface_to_surface() {
    run_scene("surface_to_surface_overlap", scenes::surface_to_surface_overlap);
}

#[test]
fn golden_cache_round_trip() {
    run_scene("cache_round_trip", scenes::cache_round_trip);
}

#[test]
fn golden_nv12_region_limited() {
    run_scene("nv12_region_limited", scenes::nv12_region_limited);
    // Pixels outside the regions keep the pre-fill colour exactly.
    let mut gpu = offscreen(scenes::OUT);
    scenes::nv12_region_limited(&mut gpu);
    gpu.wait_idle();
    let s = gpu.read_surface(1).unwrap();
    for (x, y) in [(8, 0), (0, 8), (15, 8), (26, 8), (16, 14), (31, 0), (29, 23)] {
        assert_eq!(s.pixel(x, y), [0, 255, 0, 255], "pixel ({x},{y}) outside regions was touched");
    }
    assert_ne!(s.pixel(0, 0), [0, 255, 0, 255]);
    assert_ne!(s.pixel(30, 22), [0, 255, 0, 255]);
}

#[test]
fn golden_reset() {
    run_scene("reset", scenes::reset);
    let mut gpu = offscreen(Size::new(40, 20));
    scenes::reset(&mut gpu);
    gpu.end_frame(1, PresentLog::default().callback(1));
    gpu.wait_idle();
    assert!(gpu.read_surface(1).is_none(), "reset drops old surfaces");
    let out = gpu.read_output().unwrap();
    assert_eq!(out.size, Size::new(40, 20));
    assert_eq!(out.pixel(0, 0), [0, 0, 0, 255], "unmapped output area is black");
    assert_eq!(out.pixel(15, 5), [200, 100, 50, 255]);
}

/// Known RGB → g-r-d's integer encoder (Y = (54R+183G+18B)>>8, …, 2×2 chroma average) →
/// our NV12 shader → RGB within ±2 per channel, over a 16³ grid of the RGB cube.
#[test]
fn colour_accuracy_matches_grd_encoder_within_2() {
    const STEPS: u32 = 16;
    let blocks = STEPS * STEPS * STEPS; // 4096 colours, one 2×2 block each
    let bw = 64u32;
    let size = Size::new(bw * 2, (blocks / bw) * 2);
    let mut bgra = vec![0u8; (size.width * size.height * 4) as usize];
    let level = |i: u32| ((i * 255) / (STEPS - 1)) as u8;
    for i in 0..blocks {
        let (r, g, b) = (level(i / (STEPS * STEPS)), level((i / STEPS) % STEPS), level(i % STEPS));
        let (bx, by) = ((i % bw) * 2, (i / bw) * 2);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let o = (((by + dy) * size.width + bx + dx) * 4) as usize;
            bgra[o..o + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }
    let frame = Nv12Frame::new(color::grd_encode_nv12(size, &bgra).unwrap());
    let mut gpu = offscreen(size);
    gpu.reset(size);
    gpu.create_surface(1, size);
    gpu.blit_nv12(1, &frame, &[Rect::new(0, 0, size.width, size.height)]);
    gpu.wait_idle();
    let out = gpu.read_surface(1).unwrap();
    let mut worst = 0u8;
    for (o, e) in out.data.as_chunks::<4>().0.iter().zip(bgra.as_chunks::<4>().0) {
        for c in 0..3 {
            worst = worst.max(o[c].abs_diff(e[c]));
        }
        assert_eq!(o[3], 255);
    }
    assert!(worst <= 2, "max channel error {worst} > 2");
}

/// The zero-copy path (IOSurface-backed NV12 `CVPixelBuffer` → `CVMetalTextureCache`)
/// produces the same pixels as the CPU-plane path.
#[test]
fn nv12_pixel_buffer_path_matches_planes() {
    let size = Size::new(32, 24);
    let (_, bgra) = gradient(size.width, size.height, 0, 77);
    let planes = color::grd_encode_nv12(size, &bgra).unwrap();
    let pb = drift_render::PixelBufferNv12::from_planes(&planes).unwrap();
    let mut a = offscreen(size);
    let mut b = offscreen(size);
    for (sink, frame) in [(&mut a, Nv12Frame::new(planes)), (&mut b, Nv12Frame::new(pb))] {
        sink.reset(size);
        sink.create_surface(1, size);
        sink.blit_nv12(1, &frame, &[Rect::new(0, 0, 32, 24)]);
        sink.wait_idle();
    }
    assert_eq!(a.read_surface(1).unwrap(), b.read_surface(1).unwrap());
}

#[test]
fn presented_exactly_once_per_frame() {
    let mut gpu = offscreen(scenes::OUT);
    scenes::solid_fill(&mut gpu);
    let log = PresentLog::default();
    for id in 0..200u32 {
        gpu.solid_fill(1, Bgra::new(id as u8, 0, 0, 255), &[Rect::new(0, 0, 4, 4)]);
        gpu.end_frame(id, log.callback(id));
    }
    // Frames still in flight when the compositor goes away are still acknowledged.
    gpu.end_frame(200, log.callback(200));
    drop(gpu);
    let deadline = Instant::now() + Duration::from_secs(5);
    while log.ids().len() < 201 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(50));
    let mut counts = BTreeMap::new();
    for id in log.ids() {
        *counts.entry(id).or_insert(0) += 1;
    }
    assert_eq!(counts.len(), 201);
    assert!(counts.values().all(|&n| n == 1), "a frame was presented more than once: {counts:?}");
    assert_eq!(log.ids(), (0..=200).collect::<Vec<_>>(), "presented in frame order");
}

#[test]
fn hidden_renderer_presents_nothing_but_still_releases_frames() {
    let mut gpu = offscreen(scenes::OUT);
    scenes::solid_fill(&mut gpu);
    gpu.end_frame(0, PresentLog::default().callback(0));
    gpu.wait_idle();
    let before = gpu.stats().presents;
    assert_eq!(before, 1);
    gpu.set_visible(false);
    let log = PresentLog::default();
    for id in 1..=10 {
        gpu.solid_fill(1, Bgra::new(9, 9, 9, 255), &[Rect::new(0, 0, 2, 2)]);
        gpu.end_frame(id, log.callback(id));
    }
    gpu.wait_idle();
    assert_eq!(gpu.stats().presents, before, "zero presents while hidden");
    assert_eq!(log.ids(), (1..=10).collect::<Vec<_>>(), "callbacks still fire exactly once");
    // Surface updates made while hidden are not lost.
    assert_eq!(gpu.read_surface(1).unwrap().pixel(0, 0), [9, 9, 9, 255]);
    gpu.set_visible(true);
    gpu.end_frame(11, log.callback(11));
    gpu.wait_idle();
    assert_eq!(gpu.stats().presents, before + 1);
}

#[test]
fn malformed_operations_never_panic() {
    let mut gpu = offscreen(scenes::OUT);
    gpu.reset(scenes::OUT);
    gpu.create_surface(1, Size::new(8, 8));
    gpu.create_surface(2, Size::new(0, 0));
    gpu.map_surface_to_output(1, Point::new(30, 22));
    gpu.map_surface_to_output(9, Point::new(0, 0));
    gpu.blit_bgra(1, Rect::new(0, 0, 8, 8), 32, &[0; 10]); // short data
    gpu.blit_bgra(1, Rect::new(0, 0, 8, 8), 3, &[0; 1000]); // stride < row
    gpu.blit_bgra(7, Rect::new(0, 0, 1, 1), 4, &[0; 4]); // unknown surface
    gpu.blit_bgra(1, Rect::new(u32::MAX - 1, 0, 8, 8), 32, &[0; 256]);
    gpu.solid_fill(1, Bgra::default(), &[Rect::new(100, 100, 5, 5)]);
    gpu.surface_to_surface(1, 3, Rect::new(0, 0, 4, 4), &[Point::new(0, 0)]);
    gpu.surface_to_surface(1, 1, Rect::new(6, 6, 4, 4), &[Point::new(u32::MAX, 0)]);
    gpu.surface_to_cache(1, Rect::new(4, 4, 10, 10), 1);
    gpu.surface_to_cache(1, Rect::new(40, 40, 10, 10), 2);
    gpu.cache_to_surface(99, 1, &[Point::new(0, 0)]);
    gpu.blit_nv12(1, &nv12_gradient(Size::new(4, 4), 1), &[Rect::new(0, 0, 8, 8)]);
    gpu.delete_surface(1);
    gpu.delete_surface(1);
    gpu.end_frame(1, PresentLog::default().callback(1));
    gpu.wait_idle();
}
