//! HOST-pipeline glue Red: the live picture follows HOST-10's virtual display (a client layout
//! that gets a virtual display re-opens capture on it, at its mode's size), and a client that
//! confirmed AVC444 (Drift's V10 caps for a paired Drift Host) gets AVC444 frames, while a V8.1
//! client stays on AVC420. Synthetic sources only: no Screen Recording grant needed.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_core::{ConnectMode, ConnectionProfile, SessionState, Size, SystemClock};
use drift_host::display::{DisplayManager, DisplayTarget, LayoutRequest};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::pipeline::{LiveAvc444Encoder, LiveEncoder, LiveRequest, LiveSource, MovingSquare};
use drift_host::server::{Host, HostOptions, LiveFactory};
use drift_host::vdisplay_plan::ModePlan;
use drift_rdp::{SessionCommand, SessionEvent, SessionOptions, SessionSecrets};
use drift_testkit::{PresentMode, RecordingFrameSink};

const USER: &str = "drift-host-test";
const PASS: &str = "fake-glue-password";
const VIRTUAL_ID: u32 = 42;

fn profile(port: u16, identity: &HostIdentity) -> ConnectionProfile {
    let mut p = ConnectionProfile::new("Drift Host", "127.0.0.1", ConnectMode::Headless);
    p.port = port;
    USER.clone_into(&mut p.rdp_username);
    p.cert_pin = Some(identity.fingerprint());
    p
}

async fn wait_until(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_connected(events: &mut drift_rdp::SessionEvents) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SessionEvent::State(SessionState::Connected { .. }))) => return,
            Ok(Some(_)) => {}
            other => panic!("not connected: {other:?}"),
        }
    }
}

/// A virtual display for every layout, at a fixed mode.
#[derive(Clone)]
struct FixedVirtual(Size<u32>);

impl DisplayManager for FixedVirtual {
    fn apply(&mut self, _request: &LayoutRequest) -> DisplayTarget {
        let plan = ModePlan {
            pixels: self.0,
            points: Size::new(self.0.width / 2, self.0.height / 2),
            hidpi: true,
            refresh_hz: 60,
            size_mm: (300, 200),
        };
        DisplayTarget::Virtual { display_id: VIRTUAL_ID, plan }
    }
}

/// A live factory recording every request; each request's display decides the source's size.
fn factory(
    requests: Arc<Mutex<Vec<LiveRequest>>>,
    physical: Size<u32>,
    virtual_size: Size<u32>,
) -> LiveFactory {
    Box::new(move |request: LiveRequest| {
        requests.lock().unwrap().push(request);
        let size = if request.display == Some(VIRTUAL_ID) { virtual_size } else { physical };
        if request.avc444 {
            LiveAvc444Encoder::new(MovingSquare::bgra(size, 30), 30)
                .map(|e| Box::new(e) as Box<dyn LiveSource>)
        } else {
            LiveEncoder::new(MovingSquare::new(size, 30), 30).map(|e| Box::new(e) as Box<dyn LiveSource>)
        }
    })
}

fn start(identity: &HostIdentity, live: LiveFactory, displays: Box<dyn DisplayManager>) -> Host {
    Host::start(HostOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        identity: identity.clone(),
        username: USER.into(),
        password: zeroize::Zeroizing::new(PASS.to_owned()),
        desktop: Size::new(1280, 800),
        frames: Box::new(|| Box::new(TestPattern::new())),
        live: Some(live),
        input: Box::new(RecordingInput::default()),
        clipboard: None,
        audio: None,
        displays,
        pairing: None,
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_virtual_display_is_captured_at_its_mode_size() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (physical, virtual_size) = (Size::new(640, 400), Size::new(1024, 640));
    let host = start(
        &identity,
        factory(Arc::clone(&requests), physical, virtual_size),
        Box::new(FixedVirtual(virtual_size)),
    );
    let (sink, _frames) = RecordingFrameSink::new(PresentMode::Immediate);
    let (handle, mut events) = drift_rdp::spawn_session(
        profile(host.port(), &identity),
        SessionSecrets::new(PASS),
        Box::new(sink),
        Arc::new(SystemClock),
        SessionOptions { connect_timeout: Duration::from_secs(15), ..SessionOptions::default() },
    );
    wait_connected(&mut events).await;
    wait_until("frames of the physical display", Duration::from_secs(20), || host.stats().frames_sent >= 5)
        .await;

    handle
        .send(SessionCommand::Resize(drift_core::ViewGeometry {
            points: Size::new(900.0, 600.0),
            backing_scale: 1.0,
        }))
        .unwrap();
    wait_until("capture re-opened on the virtual display", Duration::from_secs(15), || {
        requests.lock().unwrap().iter().any(|r| r.display == Some(VIRTUAL_ID))
    })
    .await;
    wait_until("the desktop at the virtual mode's size", Duration::from_secs(15), || {
        host.stats().layouts.last() == Some(&virtual_size)
    })
    .await;
    let sent = host.stats().frames_sent;
    wait_until("frames of the virtual display", Duration::from_secs(15), || {
        host.stats().frames_sent >= sent + 5
    })
    .await;
    // A V8.1 client (every non-paired profile) never gets AVC444.
    assert_eq!(host.stats().avc444_frames, 0, "{:?}", host.stats());
    assert!(requests.lock().unwrap().iter().all(|r| !r.avc444));
    let _ = handle.send(SessionCommand::Close);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_confirmed_avc444_gets_avc444_frames() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let size = Size::new(640, 400);
    let host = start(
        &identity,
        factory(Arc::clone(&requests), size, size),
        Box::new(drift_host::display::PhysicalOnly),
    );
    let (sink, frames) = RecordingFrameSink::new(PresentMode::Immediate);
    let (handle, mut events) = drift_rdp::spawn_session(
        profile(host.port(), &identity),
        SessionSecrets::new(PASS),
        Box::new(sink),
        Arc::new(SystemClock),
        SessionOptions {
            connect_timeout: Duration::from_secs(15),
            gfx_caps: drift_gfx::GfxCaps::DriftHost,
            ..SessionOptions::default()
        },
    );
    wait_connected(&mut events).await;
    wait_until("an AVC444 source", Duration::from_secs(20), || {
        requests.lock().unwrap().iter().any(|r| r.avc444)
    })
    .await;
    wait_until("AVC444 frames sent", Duration::from_secs(20), || host.stats().avc444_frames >= 5).await;
    let before = frames.calls().len();
    wait_until("the client drawing them", Duration::from_secs(15), || frames.calls().len() > before + 5)
        .await;
    let _ = handle.send(SessionCommand::Close);
}
