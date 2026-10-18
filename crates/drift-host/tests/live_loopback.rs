//! HOST-pipeline Red: with a live source (synthetic frames, so no Screen Recording grant is
//! needed) the client receives a stream of AVC420 frames and acknowledges them, a resize keeps
//! the source's size, and the client's input reaches the injector (recorded, never posted).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use drift_core::{ConnectMode, ConnectionProfile, InputEvent, SessionState, Size, SystemClock};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::inject::InjectingInput;
use drift_host::pipeline::{LiveEncoder, MovingSquare};
use drift_host::server::{Host, HostOptions};
use drift_inject::plan::{DisplayMap, InjectEvent};
use drift_rdp::{SessionCommand, SessionEvent, SessionOptions, SessionSecrets};
use drift_testkit::{FrameSinkCall, PresentMode, RecordingFrameSink};

const USER: &str = "drift-host-test";
const PASS: &str = "fake-host2-password";

fn profile(port: u16, identity: &HostIdentity) -> ConnectionProfile {
    let mut p = ConnectionProfile::new("Drift Host", "127.0.0.1", ConnectMode::Headless);
    p.port = port;
    USER.clone_into(&mut p.rdp_username);
    p.cert_pin = Some(identity.fingerprint());
    p
}

fn options() -> SessionOptions {
    SessionOptions {
        connect_timeout: Duration::from_secs(15),
        reconnect_seed: Some(7),
        ..SessionOptions::default()
    }
}

async fn wait_until(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_state(
    events: &mut drift_rdp::SessionEvents,
    what: &str,
    pred: impl Fn(&SessionState) -> bool,
) -> SessionState {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SessionEvent::State(s))) if pred(&s) => return s,
            Ok(Some(_)) => {}
            other => panic!("no {what}: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_source_streams_frames_and_input_reaches_the_injector() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let size = Size::new(640, 400);
    let posted = Arc::new(std::sync::Mutex::new(Vec::<InjectEvent>::new()));
    let p = Arc::clone(&posted);
    let map = DisplayMap { origin: (0.0, 0.0), desktop: (size.width, size.height), points_per_pixel: 1.0 };
    let host = Host::start(HostOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        identity: identity.clone(),
        username: USER.into(),
        password: zeroize::Zeroizing::new(PASS.to_owned()),
        desktop: Size::new(1280, 800),
        frames: Box::new(|| Box::new(TestPattern::new())),
        live: Some(Box::new(move || {
            LiveEncoder::new(MovingSquare::new(size, 60), 60)
                .map(|e| Box::new(e) as Box<dyn drift_host::pipeline::LiveSource>)
        })),
        input: Box::new(InjectingInput::new(map, move |e: &InjectEvent| p.lock().unwrap().push(e.clone()))),
        clipboard: None,
        audio: None,
        displays: Box::new(drift_host::display::PhysicalOnly),
    })
    .unwrap();

    let (sink, frames) = RecordingFrameSink::new(PresentMode::Immediate);
    let (handle, mut events) = drift_rdp::spawn_session(
        profile(host.port(), &identity),
        SessionSecrets::new(PASS),
        Box::new(sink),
        Arc::new(SystemClock),
        options(),
    );
    wait_state(&mut events, "connected", |s| matches!(s, SessionState::Connected { .. })).await;
    wait_until("a stream of frames at the client", Duration::from_secs(20), || {
        frames.calls().iter().filter(|c| matches!(c, FrameSinkCall::BlitNv12 { .. })).count() >= 20
    })
    .await;
    wait_until("the client's acks at the host", Duration::from_secs(10), || host.stats().frames_acked >= 20)
        .await;
    assert!(host.stats().frames_sent >= 20, "{:?}", host.stats());
    // HOST-7: the session socket carries the interactive-video service class, and a link that
    // keeps up reports no backlog.
    assert!(host.stats().video_service_class, "{:?}", host.stats());
    assert_eq!(host.stats().backlog, drift_host::link::Backlog::Clear, "{:?}", host.stats());

    // A client layout does not change a live desktop: the host answers with the source's size.
    handle
        .send(SessionCommand::Resize(drift_core::ViewGeometry {
            points: Size::new(1001.0, 701.0),
            backing_scale: 1.0,
        }))
        .unwrap();
    wait_until("the layout answered", Duration::from_secs(10), || host.stats().layouts.last() == Some(&size))
        .await;

    handle.send(SessionCommand::Input(InputEvent::MouseMove { x: 10, y: 20 })).unwrap();
    handle
        .send(SessionCommand::Input(InputEvent::Key { scancode: 0x1E, extended: false, down: true }))
        .unwrap();
    wait_until("the input at the injector", Duration::from_secs(10), || {
        let posted = posted.lock().unwrap();
        posted.iter().any(|e| matches!(e, InjectEvent::Move { .. }))
            && posted.iter().any(|e| matches!(e, InjectEvent::Key { kvk: 0x00, down: true, .. }))
    })
    .await;

    let _ = handle.send(SessionCommand::Close);
    wait_until("the session to end", Duration::from_secs(10), || host.stats().sessions_ended >= 1).await;
    // Ending the session let go of the held key.
    assert!(
        posted
            .lock()
            .unwrap()
            .iter()
            .rev()
            .any(|e| matches!(e, InjectEvent::Key { kvk: 0x00, down: false, .. })),
        "the held key was released at the end"
    );
}
