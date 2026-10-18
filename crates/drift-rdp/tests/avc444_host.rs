//! AVC444 end to end on loopback (ADR `AVC444-client`): a session for a Drift Host advertises
//! V10, the vendored `ironrdp-server` negotiates AVC444 and sends one frame encoded by HOST-4's
//! `Avc444Encoder`, and Drift draws it with full chroma. A GNOME-style session (the default caps)
//! keeps V8.1 and the host can only send AVC420.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Harness, PASS, USER, options, profile};
use drift_core::{Bgra, ConnectMode, Nv12Frame, Point, Rect, SessionState, Size, SystemClock};
use drift_gfx::{FrameSink, GfxCaps, PresentedCallback};
use drift_rdp::{SessionOptions, SessionSecrets};
use drift_testkit::TestCert;
use drift_testkit::host_spike::SpikeHost;
use drift_video::avc444::{Avc444Encoder, bgra_to_yuv444};
use drift_video::quality::psnr;

const W: u32 = 640;
const H: u32 = 368;

fn text_like() -> Vec<u8> {
    let mut bgra = vec![255u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            let px: [u8; 4] = match (x % 7, y % 5) {
                (0, _) => [0, 0, 220, 255],
                (3, _) if y % 2 == 0 => [220, 0, 0, 255],
                _ => [255, 255, 255, 255],
            };
            bgra[i..i + 4].copy_from_slice(&px);
        }
    }
    bgra
}

/// A sink that paints BGRA blits into one picture of the surface's size.
#[derive(Clone, Default)]
struct Canvas {
    pixels: Arc<Mutex<Vec<u8>>>,
    blits: Arc<Mutex<u32>>,
    nv12: Arc<Mutex<u32>>,
}

impl FrameSink for Canvas {
    fn reset(&mut self, _output: Size<u32>) {}
    fn create_surface(&mut self, _id: u16, size: Size<u32>) {
        *self.pixels.lock().unwrap() = vec![0; (size.width * size.height * 4) as usize];
    }
    fn delete_surface(&mut self, _id: u16) {}
    fn map_surface_to_output(&mut self, _id: u16, _origin: Point<u32>) {}
    fn blit_bgra(&mut self, _id: u16, rect: Rect, stride: usize, data: &[u8]) {
        let mut px = self.pixels.lock().unwrap();
        for row in 0..rect.height as usize {
            let dst = ((rect.y as usize + row) * W as usize + rect.x as usize) * 4;
            let src = row * stride;
            let n = rect.width as usize * 4;
            if dst + n <= px.len() && src + n <= data.len() {
                px[dst..dst + n].copy_from_slice(&data[src..src + n]);
            }
        }
        *self.blits.lock().unwrap() += 1;
    }
    fn blit_nv12(&mut self, _id: u16, _frame: &Nv12Frame, _regions: &[Rect]) {
        *self.nv12.lock().unwrap() += 1;
    }
    fn solid_fill(&mut self, _id: u16, _color: Bgra, _rects: &[Rect]) {}
    fn surface_to_surface(&mut self, _src: u16, _dst: u16, _rect: Rect, _dests: &[Point<u32>]) {}
    fn surface_to_cache(&mut self, _id: u16, _rect: Rect, _slot: u16) {}
    fn cache_to_surface(&mut self, _slot: u16, _id: u16, _dests: &[Point<u32>]) {}
    fn evict_cache(&mut self, _slot: u16) {}
    fn end_frame(&mut self, _frame_id: u32, presented: PresentedCallback) {
        presented();
    }
    fn set_visible(&mut self, _visible: bool) {}
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

fn encode() -> (Vec<u8>, Vec<u8>) {
    let mut enc = Avc444Encoder::new(Size::new(W, H), 60).unwrap();
    let mut pics = enc.encode_bgra(&text_like(), (W * 4) as usize, Instant::now()).unwrap();
    pics.extend(enc.flush().unwrap());
    (pics[0].main.to_annex_b().unwrap(), pics[0].aux.to_annex_b().unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_drift_host_session_negotiates_v10_and_draws_an_avc444_frame() {
    let cert = TestCert::generate("127.0.0.1");
    let host =
        SpikeHost::start_avc444(cert.clone(), USER, PASS, Size::new(W, H), vec![encode()]).await.unwrap();
    let canvas = Canvas::default();
    let opts = SessionOptions { gfx_caps: GfxCaps::DriftHost, ..options() };
    let (handle, events) = drift_rdp::spawn_session(
        profile(ConnectMode::Headless, host.port(), Some(cert.fingerprint())),
        SessionSecrets::new(PASS),
        Box::new(canvas.clone()),
        Arc::new(SystemClock),
        opts,
    );
    let mut h = Harness::from_parts(
        handle,
        events,
        drift_testkit::RecordingFrameSink::new(drift_testkit::PresentMode::Immediate).1,
    );
    h.wait_for("connected", Duration::from_secs(15), |e| {
        matches!(e, drift_rdp::SessionEvent::State(SessionState::Connected { .. }))
    })
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while *canvas.blits.lock().unwrap() == 0 {
        assert!(tokio::time::Instant::now() < deadline, "no AVC444 picture; host log {:?}", host.log());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let log = host.log();
    assert!(log.avc444_negotiated, "V10 should let the host choose AVC444: {log:?}");
    let p = chroma_psnr(&text_like(), &canvas.pixels.lock().unwrap());
    eprintln!("AVC444 over loopback: chroma PSNR {p:.2} dB; host log {log:?}");
    assert!(p >= 30.0, "chroma PSNR {p:.2} dB");
    h.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gnome_style_session_keeps_v81_so_the_host_cannot_send_avc444() {
    let cert = TestCert::generate("127.0.0.1");
    let host =
        SpikeHost::start_avc444(cert.clone(), USER, PASS, Size::new(W, H), vec![encode()]).await.unwrap();
    let canvas = Canvas::default();
    let (handle, events) = drift_rdp::spawn_session(
        profile(ConnectMode::Headless, host.port(), Some(cert.fingerprint())),
        SessionSecrets::new(PASS),
        Box::new(canvas.clone()),
        Arc::new(SystemClock),
        options(),
    );
    let mut h = Harness::from_parts(
        handle,
        events,
        drift_testkit::RecordingFrameSink::new(drift_testkit::PresentMode::Immediate).1,
    );
    h.wait_for("connected", Duration::from_secs(15), |e| {
        matches!(e, drift_rdp::SessionEvent::State(SessionState::Connected { .. }))
    })
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let log = host.log();
        if log.avc420_negotiated || log.error.is_some() {
            assert!(!log.avc444_negotiated, "{log:?}");
            assert_eq!(log.frames_sent, 0, "AVC444 frames must not be sent to a V8.1 client: {log:?}");
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no negotiation; host log {log:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    h.close().await;
}
