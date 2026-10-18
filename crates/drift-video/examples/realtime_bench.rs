//! HOST-4 measurement: can the real-time encoder keep 60 fps at 4K, as AVC420 and as AVC444's
//! two streams? (ADR `IOS-0` open question 5, ADR `HOST-4-realtime-encoder`.)
//!
//! ```text
//! cargo run --release -p drift-video --example realtime_bench -- [width height frames]
//! ```
//!
//! Frames are submitted on a 60 Hz wall clock (late frames are submitted at once, so falling
//! behind shows as a lower rate). Latency is submission → VideoToolbox's output callback; for
//! AVC444 it also includes the CPU conversion and split, and ends when *both* views are out.
#![allow(missing_docs, clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use drift_core::Size;
use drift_video::avc444::{
    Avc444Encoder, CHROMA_FILTER_THRESHOLD, bgra_to_yuv444, combine, combine_filtered,
};
use drift_video::decode::VtDecoder;
use drift_video::encode::{EncodedFrame, EncoderConfig, PixelBuffer, VtEncoder};
use drift_video::quality::psnr;

/// A desktop-like picture: text-ish stripes and a moving window.
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

struct Stats {
    latencies: Vec<Duration>,
    bytes: usize,
    elapsed: Duration,
    frames: usize,
}

impl Stats {
    fn report(&mut self, name: &str) {
        self.latencies.sort();
        let pct = |p: usize| self.latencies.get(self.latencies.len() * p / 100).copied().unwrap_or_default();
        let secs = self.elapsed.as_secs_f64();
        println!(
            "{name}: {:.1} fps over {} frames, latency p50 {:.1} ms p95 {:.1} ms max {:.1} ms, {:.1} Mbit/s",
            self.frames as f64 / secs,
            self.frames,
            pct(50).as_secs_f64() * 1e3,
            pct(95).as_secs_f64() * 1e3,
            self.latencies.last().copied().unwrap_or_default().as_secs_f64() * 1e3,
            self.bytes as f64 * 8.0 / secs / 1e6,
        );
    }
}

fn pace(start: Instant, n: u32) {
    let due = start + Duration::from_micros(u64::from(n) * 16_667);
    if let Some(wait) = due.checked_duration_since(Instant::now()) {
        std::thread::sleep(wait);
    }
}

/// Source pixels for AVC420: BGRA (VideoToolbox converts) or NV12 (already 4:2:0, as the
/// host's capture can deliver it).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Input {
    Bgra,
    Nv12,
}

fn bench_avc420(size: Size<u32>, sources: &[Vec<u8>], frames: u32, input: Input, low_latency: bool) -> Stats {
    let buffers: Vec<PixelBuffer> = sources
        .iter()
        .map(|px| match input {
            Input::Bgra => {
                let buf = PixelBuffer::new_bgra(size).unwrap();
                buf.fill_bgra(|x, y| {
                    let i = ((y * size.width + x) * 4) as usize;
                    [px[i], px[i + 1], px[i + 2], px[i + 3]]
                })
                .unwrap();
                buf
            }
            Input::Nv12 => {
                let pic = bgra_to_yuv444(px, size, (size.width * 4) as usize).unwrap();
                let (main, _) = drift_video::avc444::split(&pic).unwrap();
                let buf = PixelBuffer::new_nv12(size).unwrap();
                buf.write_nv12(&main).unwrap();
                buf
            }
        })
        .collect();
    let config = EncoderConfig { low_latency, ..EncoderConfig::realtime(size, 60) };
    let mut enc = VtEncoder::new(config).unwrap();
    let mut submitted: HashMap<Duration, Instant> = HashMap::new();
    let mut out: Vec<EncodedFrame> = Vec::new();
    let start = Instant::now();
    let mut first_pts = None;
    let mut call = Duration::ZERO;
    for n in 0..frames {
        pace(start, n);
        let at = Instant::now();
        let got = enc.encode(&buffers[n as usize % buffers.len()], at).unwrap();
        call += at.elapsed();
        let pts = at.duration_since(*first_pts.get_or_insert(at));
        submitted.insert(pts, at);
        out.extend(got);
    }
    out.extend(enc.flush().unwrap());
    let elapsed = start.elapsed();
    println!(
        "  encode call {:.1} ms mean, low-latency rate control: {}",
        call.as_secs_f64() * 1e3 / f64::from(frames),
        enc.is_low_latency()
    );
    let latencies = out
        .iter()
        .filter_map(|f| {
            let at = submitted.iter().min_by_key(|(p, _)| p.abs_diff(f.pts()))?.1;
            Some(f.completed_at().saturating_duration_since(*at))
        })
        .collect();
    Stats { latencies, bytes: out.iter().map(|f| f.avcc().len()).sum(), elapsed, frames: out.len() }
}

/// AVC444's two encodes alone: both views split once up front, so this measures VideoToolbox,
/// not the CPU split (which the host should run on the GPU).
fn bench_avc444_hw(size: Size<u32>, sources: &[Vec<u8>], frames: u32) -> Stats {
    use drift_video::avc444::{aux_height, split_bgra};
    let stride = (size.width * 4) as usize;
    let aux_size = Size::new(size.width, aux_height(size.height));
    let views: Vec<(PixelBuffer, PixelBuffer)> = sources
        .iter()
        .map(|src| {
            let (main, aux) = split_bgra(src, size, stride).unwrap();
            let a = PixelBuffer::new_nv12(size).unwrap();
            a.write_nv12(&main).unwrap();
            let b = PixelBuffer::new_nv12(aux_size).unwrap();
            b.write_nv12(&aux).unwrap();
            (a, b)
        })
        .collect();
    let mut main = VtEncoder::new(EncoderConfig::realtime(size, 60)).unwrap();
    let mut aux = VtEncoder::new(EncoderConfig::realtime(aux_size, 60)).unwrap();
    let (mut main_out, mut aux_out, mut submitted) = (Vec::new(), Vec::new(), Vec::new());
    let start = Instant::now();
    for n in 0..frames {
        pace(start, n);
        let at = Instant::now();
        submitted.push(at);
        let (a, b) = &views[n as usize % views.len()];
        let (m, x) = std::thread::scope(|s| {
            let x = s.spawn(|| aux.encode(b, at).unwrap());
            (main.encode(a, at).unwrap(), x.join().unwrap())
        });
        main_out.extend(m);
        aux_out.extend(x);
    }
    main_out.extend(main.flush().unwrap());
    aux_out.extend(aux.flush().unwrap());
    let elapsed = start.elapsed();
    let latencies = main_out
        .iter()
        .zip(&aux_out)
        .zip(&submitted)
        .map(|((m, x), at)| m.completed_at().max(x.completed_at()).saturating_duration_since(*at))
        .collect();
    let bytes = main_out.iter().chain(&aux_out).map(|f| f.avcc().len()).sum();
    Stats { latencies, bytes, elapsed, frames: main_out.len().min(aux_out.len()) }
}

fn bench_avc444(size: Size<u32>, sources: &[Vec<u8>], frames: u32) -> Stats {
    let mut enc = Avc444Encoder::new(size, 60).unwrap();
    let mut submitted = Vec::new();
    let mut pictures = Vec::new();
    let mut cpu = Duration::ZERO;
    let start = Instant::now();
    for n in 0..frames {
        pace(start, n);
        let at = Instant::now();
        submitted.push(at);
        let src = &sources[n as usize % sources.len()];
        let t = Instant::now();
        let got = enc.encode_bgra(src, (size.width * 4) as usize, at).unwrap();
        cpu += t.elapsed();
        pictures.extend(got);
    }
    pictures.extend(enc.flush().unwrap());
    let elapsed = start.elapsed();
    println!(
        "AVC444 encode_bgra call (convert + split + 2 submits): {:.1} ms mean",
        cpu.as_secs_f64() * 1e3 / f64::from(frames)
    );
    // Quality of the first picture (a keyframe in both views).
    if let Some(p) = pictures.first() {
        let main = VtDecoder::new().decode_picture(&p.main.to_annex_b().unwrap()).ok().flatten();
        let aux = VtDecoder::new().decode_picture(&p.aux.to_annex_b().unwrap()).ok().flatten();
        if let (Some(main), Some(aux)) = (main, aux)
            && let Some(back) = combine(&main.to_planes().unwrap(), &aux.to_planes().unwrap())
        {
            let src = bgra_to_yuv444(&sources[0], size, (size.width * 4) as usize).unwrap();
            // What a 4:2:0-only client sees: the main view's chroma, upsampled by repetition.
            let main_planes = main.to_planes().unwrap();
            let w = size.width as usize;
            let up = |k: usize| -> Vec<u8> {
                (0..src.y().len()).map(|i| main_planes.uv()[(i / w / 2) * w + (i % w) / 2 * 2 + k]).collect()
            };
            println!(
                "AVC420 view alone (chroma repeated): U {:.1} V {:.1} dB",
                psnr(&up(0), src.u()).unwrap_or(0.0),
                psnr(&up(1), src.v()).unwrap_or(0.0)
            );
            if let Some(f) =
                combine_filtered(&main_planes, &aux.to_planes().unwrap(), CHROMA_FILTER_THRESHOLD)
            {
                println!(
                    "AVC444 filtered (threshold {CHROMA_FILTER_THRESHOLD}): U {:.1} V {:.1} dB",
                    psnr(f.u(), src.u()).unwrap_or(0.0),
                    psnr(f.v(), src.v()).unwrap_or(0.0)
                );
            }
            println!(
                "AVC444 PSNR (first keyframe): Y {:.1} U {:.1} V {:.1} dB",
                psnr(back.y(), src.y()).unwrap_or(0.0),
                psnr(back.u(), src.u()).unwrap_or(0.0),
                psnr(back.v(), src.v()).unwrap_or(0.0)
            );
        }
    }
    let latencies = pictures
        .iter()
        .map(|p| {
            let done = p.main.completed_at().max(p.aux.completed_at());
            done.saturating_duration_since(submitted[p.index as usize])
        })
        .collect();
    let bytes = pictures.iter().map(|p| p.main.avcc().len() + p.aux.avcc().len()).sum();
    Stats { latencies, bytes, elapsed, frames: pictures.len() }
}

fn main() {
    let args: Vec<u32> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let size = Size::new(*args.first().unwrap_or(&3840), *args.get(1).unwrap_or(&2160));
    let frames = *args.get(2).unwrap_or(&600);
    let chip = std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    println!("{chip}, {}x{}, {frames} frames at 60 Hz", size.width, size.height);
    let sources: Vec<Vec<u8>> = (0..8).map(|n| frame(size, n * 10)).collect();
    // Where the AVC444 CPU time goes.
    let stride = (size.width * 4) as usize;
    let t = Instant::now();
    let mut views = None;
    for src in sources.iter().cycle().take(16) {
        views = drift_video::avc444::split_bgra(src, size, stride);
    }
    let split_ms = t.elapsed().as_secs_f64() * 1e3 / 16.0;
    let (main, aux) = views.unwrap();
    let t = Instant::now();
    for _ in 0..16 {
        let a = PixelBuffer::new_nv12(size).unwrap();
        a.write_nv12(&main).unwrap();
        let b = PixelBuffer::new_nv12(Size::new(size.width, drift_video::avc444::aux_height(size.height)))
            .unwrap();
        b.write_nv12(&aux).unwrap();
    }
    let upload_ms = t.elapsed().as_secs_f64() * 1e3 / 16.0;
    println!("AVC444 CPU per frame: split {split_ms:.1} ms, two IOSurface NV12 buffers {upload_ms:.1} ms");
    for (input, ll, name) in [
        (Input::Nv12, true, "AVC420 NV12 in, low-latency RC"),
        (Input::Bgra, true, "AVC420 BGRA in, low-latency RC"),
        (Input::Nv12, false, "AVC420 NV12 in, normal RC"),
    ] {
        bench_avc420(size, &sources, frames, input, ll).report(name);
    }
    bench_avc444_hw(size, &sources, frames).report("AVC444 encoders only (views split up front)");
    bench_avc444(size, &sources, frames).report("AVC444 (two streams, CPU split per frame)");
}
