//! EXT-5 measurement: can VideoToolbox's real-time HEVC encoder keep 60 fps at 4K as Main
//! (from NV12), Main 4:4:4 (from BGRA, converted by VideoToolbox's pixel transfer) and Main 10?
//! (ADR `EXT-5-hevc-hdr`.)
//!
//! ```text
//! cargo run --release -p drift-video --example hevc_bench -- [width height frames]
//! ```
//!
//! Frames are submitted on a 60 Hz wall clock (late frames go at once, so falling behind shows
//! as a lower rate). Latency is submission → VideoToolbox's output callback, including the pixel
//! transfer for 4:4:4 and Main 10. Every input frame is pre-rendered, so drawing costs nothing.
#![allow(missing_docs, clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use drift_core::video::{HevcProfile, Transfer};
use drift_core::{Nv12Planes, Size};
use drift_video::encode::PixelBuffer;
use drift_video::hevc::{HevcConfig, HevcEncoder, probe_encoder};

/// A desktop-like picture: text-ish stripes and a moving window (BGRA).
fn frame(size: Size<u32>, n: u32) -> Vec<u8> {
    let (w, h) = (size.width, size.height);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let text = (x / 7 + y / 13) % 5 == 0 && (y % 13) < 9;
            let win_x = (n * 12) % w.max(1);
            let in_win = x >= win_x && x < win_x + w / 4 && y > h / 3 && y < h / 3 + h / 4;
            let (r, g, b) = match (in_win, text) {
                (true, true) => (20, 20, 20),
                (true, false) => (250, 250, 245),
                (false, true) => (230, 80, 60),
                (false, false) => ((x * 255 / w) as u8, (y * 255 / h) as u8, 140),
            };
            out[i..i + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }
    out
}

fn bgra_buffer(size: Size<u32>, px: &[u8]) -> PixelBuffer {
    let b = PixelBuffer::new_bgra(size).unwrap();
    let w = size.width;
    b.fill_bgra(|x, y| {
        let i = ((y * w + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    })
    .unwrap();
    b
}

fn nv12_buffer(size: Size<u32>, px: &[u8]) -> PixelBuffer {
    let yuv = drift_video::avc444::bgra_to_yuv444(px, size, size.width as usize * 4).unwrap();
    let (w, h) = (size.width as usize, size.height as usize);
    let mut uv = vec![0u8; w * h.div_ceil(2)];
    for row in 0..h.div_ceil(2) {
        for col in 0..w / 2 {
            uv[row * w + col * 2] = yuv.u()[(row * 2) * w + col * 2];
            uv[row * w + col * 2 + 1] = yuv.v()[(row * 2) * w + col * 2];
        }
    }
    let planes = Nv12Planes::new(size, yuv.y().to_vec(), uv).unwrap();
    let b = PixelBuffer::new_nv12(size).unwrap();
    b.write_nv12(&planes).unwrap();
    b
}

fn run(size: Size<u32>, frames: u32, profile: HevcProfile, inputs: &[PixelBuffer]) {
    let transfer = if profile.ten_bit() { Transfer::Pq } else { Transfer::Sdr };
    let mut enc = HevcEncoder::new(HevcConfig::realtime(size, 60, profile, transfer)).unwrap();
    let interval = Duration::from_secs(1) / 60;
    let start = Instant::now();
    let mut submitted = Vec::new();
    let mut latencies = Vec::new();
    let mut bytes = 0usize;
    for n in 0..frames {
        let due = start + interval * n;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let at = Instant::now();
        submitted.push(at);
        for f in enc.encode(&inputs[n as usize % inputs.len()], at).unwrap() {
            bytes += f.annex_b().len();
            let i = latencies.len();
            latencies.push(f.completed_at().saturating_duration_since(submitted[i]));
        }
    }
    for f in enc.flush().unwrap() {
        bytes += f.annex_b().len();
        let i = latencies.len();
        latencies.push(f.completed_at().saturating_duration_since(submitted[i]));
    }
    let elapsed = start.elapsed();
    latencies.sort();
    let pct = |p: usize| latencies.get(latencies.len() * p / 100).copied().unwrap_or_default();
    println!(
        "{profile:?} {}x{}: {:.1} fps over {frames} frames, latency p50 {:.1} ms p95 {:.1} ms, {:.1} Mbit/s, low-latency rc {}",
        size.width,
        size.height,
        f64::from(frames) / elapsed.as_secs_f64(),
        pct(50).as_secs_f64() * 1e3,
        pct(95).as_secs_f64() * 1e3,
        bytes as f64 * 8.0 / elapsed.as_secs_f64() / 1e6,
        enc.is_low_latency(),
    );
}

fn main() {
    let args: Vec<u32> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let size = Size::new(*args.first().unwrap_or(&3840), *args.get(1).unwrap_or(&2160));
    let frames = *args.get(2).unwrap_or(&300);
    let chip = std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    let profiles = probe_encoder(size);
    println!("{chip}; HEVC encoder profiles at {}x{}: {profiles:?}", size.width, size.height);
    let pictures: Vec<Vec<u8>> = (0..8).map(|n| frame(size, n * 7)).collect();
    let nv12: Vec<PixelBuffer> = pictures.iter().map(|p| nv12_buffer(size, p)).collect();
    let bgra: Vec<PixelBuffer> = pictures.iter().map(|p| bgra_buffer(size, p)).collect();
    for profile in profiles {
        let inputs = if profile.full_chroma() { &bgra } else { &nv12 };
        run(size, frames, profile, inputs);
    }
}
