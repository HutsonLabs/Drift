//! HOST-1 Red: the vendored `ironrdp-server` (with `egfx`) streams an AVC420 frame to Drift's own
//! client on loopback, and Drift decodes it to the same picture ffmpeg does.
//!
//! The host is `drift_testkit::host_spike::SpikeHost`: `ironrdp-server` with NLA, a fixed-size
//! display, the EGFX bridge and the Display Control channel. It sends access unit 0 of
//! `fixtures/h264/leg3.h264` (1280×800) as one AVC420 frame once the graphics channel is ready.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Harness, PASS, USER, options, profile};
use drift_core::{Bgra, ConnectMode, Nv12Frame, Nv12Planes, Point, Rect, SessionState, Size, SystemClock};
use drift_gfx::{FrameSink, PresentedCallback};
use drift_rdp::{SessionCommand, SessionSecrets};
use drift_testkit::host_spike::SpikeHost;
use drift_testkit::{PresentMode, RecordingFrameSink, TestCert};
use drift_video::annexb::{nal_type, nal_unit_type, split_nals};
use drift_video::decode::DecodedPicture;
use drift_video::quality::psnr_nv12;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(drift_testkit::fixtures::fixtures_dir().join(name)).unwrap()
}

/// Access unit 0 of a captured g-r-d stream (every unit starts with an AUD).
fn first_access_unit(stream: &[u8]) -> Vec<u8> {
    let mut au = Vec::new();
    for nal in split_nals(stream) {
        if nal_unit_type(nal) == Some(nal_type::AUD) && !au.is_empty() {
            break;
        }
        au.extend_from_slice(&[0, 0, 0, 1]);
        au.extend_from_slice(nal);
    }
    au
}

/// A recording sink that also keeps the planes of the first decoded NV12 picture.
#[derive(Clone)]
struct CapturingSink {
    inner: Arc<Mutex<RecordingFrameSink>>,
    first: Arc<Mutex<Option<Nv12Planes>>>,
}

impl FrameSink for CapturingSink {
    fn reset(&mut self, output: Size<u32>) {
        self.inner.lock().unwrap().reset(output);
    }
    fn create_surface(&mut self, id: u16, size: Size<u32>) {
        self.inner.lock().unwrap().create_surface(id, size);
    }
    fn delete_surface(&mut self, id: u16) {
        self.inner.lock().unwrap().delete_surface(id);
    }
    fn map_surface_to_output(&mut self, id: u16, origin: Point<u32>) {
        self.inner.lock().unwrap().map_surface_to_output(id, origin);
    }
    fn blit_bgra(&mut self, id: u16, rect: Rect, stride: usize, data: &[u8]) {
        self.inner.lock().unwrap().blit_bgra(id, rect, stride, data);
    }
    fn blit_nv12(&mut self, id: u16, frame: &Nv12Frame, regions: &[Rect]) {
        let mut first = self.first.lock().unwrap();
        if first.is_none() {
            *first = frame.downcast_ref::<DecodedPicture>().and_then(|p| p.to_planes().ok());
        }
        self.inner.lock().unwrap().blit_nv12(id, frame, regions);
    }
    fn solid_fill(&mut self, id: u16, color: Bgra, rects: &[Rect]) {
        self.inner.lock().unwrap().solid_fill(id, color, rects);
    }
    fn surface_to_surface(&mut self, src: u16, dst: u16, rect: Rect, dests: &[Point<u32>]) {
        self.inner.lock().unwrap().surface_to_surface(src, dst, rect, dests);
    }
    fn surface_to_cache(&mut self, id: u16, rect: Rect, slot: u16) {
        self.inner.lock().unwrap().surface_to_cache(id, rect, slot);
    }
    fn cache_to_surface(&mut self, slot: u16, id: u16, dests: &[Point<u32>]) {
        self.inner.lock().unwrap().cache_to_surface(slot, id, dests);
    }
    fn evict_cache(&mut self, slot: u16) {
        self.inner.lock().unwrap().evict_cache(slot);
    }
    fn end_frame(&mut self, frame_id: u32, presented: PresentedCallback) {
        self.inner.lock().unwrap().end_frame(frame_id, presented);
    }
    fn set_visible(&mut self, visible: bool) {
        self.inner.lock().unwrap().set_visible(visible);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ironrdp_server_streams_an_avc420_frame_that_drift_decodes() {
    let cert = TestCert::generate("127.0.0.1");
    let au = first_access_unit(&fixture("h264/leg3.h264"));
    let host = SpikeHost::start(cert.clone(), USER, PASS, Size::new(1280, 800), vec![au]).await.unwrap();

    let (recording, frames) = RecordingFrameSink::new(PresentMode::Immediate);
    let sink = CapturingSink { inner: Arc::new(Mutex::new(recording)), first: Arc::default() };
    let first = Arc::clone(&sink.first);
    let (handle, events) = drift_rdp::spawn_session(
        profile(ConnectMode::Headless, host.port(), Some(cert.fingerprint())),
        SessionSecrets::new(PASS),
        Box::new(sink),
        Arc::new(SystemClock),
        options(),
    );
    let mut h = Harness::from_parts(handle, events, frames);
    h.wait_for("connected", Duration::from_secs(15), |e| {
        matches!(e, drift_rdp::SessionEvent::State(SessionState::Connected { .. }))
    })
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let planes = loop {
        if let Some(p) = first.lock().unwrap().clone() {
            break p;
        }
        assert!(tokio::time::Instant::now() < deadline, "no AVC420 picture; host log {:?}", host.log());
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(planes.y().len(), 1280 * 800, "a 1280×800 picture");
    let golden = fixture("h264/goldens/leg3_f000.nv12");
    let luma = 1280 * 800;
    let want =
        Nv12Planes::new(Size::new(1280, 800), golden[..luma].to_vec(), golden[luma..].to_vec()).unwrap();
    let psnr = psnr_nv12(&planes, &want).unwrap();
    assert!(psnr >= 40.0, "PSNR {psnr:.2} dB < 40 against ffmpeg's decode");

    let log = host.log();
    eprintln!("HOST-1: PSNR {psnr:.2} dB against ffmpeg; host log {log:?}");
    assert!(log.avc420_negotiated, "the host should negotiate AVC420 with Drift: {log:?}");
    assert_eq!(log.frames_sent, 1, "{log:?}");
    h.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_resize_reaches_the_host_display_as_a_monitor_layout() {
    let cert = TestCert::generate("127.0.0.1");
    let host = SpikeHost::start(cert.clone(), USER, PASS, Size::new(1280, 800), Vec::new()).await.unwrap();
    let mut h = Harness::start(profile(ConnectMode::Headless, host.port(), Some(cert.fingerprint())), PASS);
    h.wait_for("connected", Duration::from_secs(15), |e| {
        matches!(e, drift_rdp::SessionEvent::State(SessionState::Connected { .. }))
    })
    .await;
    let geometry = drift_core::ViewGeometry { points: Size::new(1000.0, 700.0), backing_scale: 1.0 };
    h.handle.send(SessionCommand::Resize(geometry)).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(layout) = host.log().layouts.last() {
            assert_eq!(*layout, (1000, 700), "{:?}", host.log());
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no layout; host log {:?}", host.log());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    h.close().await;
}
