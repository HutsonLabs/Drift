//! HOST-2 Red: Drift's own client connects to the host on loopback, sees the test pattern, sends
//! input the host receives, and a second client is refused while the first is connected.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use drift_core::{ConnectMode, ConnectionProfile, InputEvent, SessionState, Size, SystemClock};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::server::{Host, HostOptions};
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
async fn drift_connects_sees_the_pattern_and_its_input_reaches_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let input = RecordingInput::default();
    let host = Host::start(HostOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        identity: identity.clone(),
        username: USER.into(),
        password: zeroize::Zeroizing::new(PASS.to_owned()),
        desktop: Size::new(1280, 800),
        frames: Box::new(|| Box::new(TestPattern::new())),
        live: None,
        input: Box::new(input.clone()),
        clipboard: None,
        audio: None,
        displays: Box::new(drift_host::display::PhysicalOnly),
        pairing: None,
    })
    .unwrap();

    let (sink, frames) = RecordingFrameSink::new(PresentMode::Immediate);
    let client_link = drift_ext::channel::Link::new();
    let (handle, mut events) = drift_rdp::spawn_session(
        profile(host.port(), &identity),
        SessionSecrets::new(PASS),
        Box::new(sink),
        Arc::new(SystemClock),
        SessionOptions { drift_mac: Some(client_link.clone()), ..options() },
    );
    wait_state(&mut events, "connected", |s| matches!(s, SessionState::Connected { .. })).await;
    wait_until("an AVC420 test pattern on screen", Duration::from_secs(15), || {
        frames.calls().iter().any(|c| matches!(c, FrameSinkCall::BlitNv12 { .. }))
    })
    .await;
    assert!(host.stats().avc420_negotiated, "{:?}", host.stats());
    // Drift::Mac::v1 (EXT-1) is offered by the host and negotiated with a Drift client.
    wait_until("Drift::Mac::v1 negotiated", Duration::from_secs(10), || client_link.negotiated().is_some())
        .await;
    assert!(host.stats().drift_mac.starts_with("Negotiated"), "{:?}", host.stats());

    handle.send(SessionCommand::Input(InputEvent::MouseMove { x: 10, y: 20 })).unwrap();
    handle
        .send(SessionCommand::Input(InputEvent::Key { scancode: 0x1E, extended: false, down: true }))
        .unwrap();
    wait_until("the input at the host", Duration::from_secs(10), || {
        input.mouse_events() >= 1 && input.key_events() >= 1
    })
    .await;

    // A second client while the first is connected is refused, and the first keeps going.
    let (_h2, mut events2) = drift_rdp::spawn_session(
        profile(host.port(), &identity),
        SessionSecrets::new(PASS),
        Box::new(RecordingFrameSink::new(PresentMode::Immediate).0),
        Arc::new(SystemClock),
        SessionOptions { reconnect_seed: Some(1), ..options() },
    );
    wait_state(&mut events2, "the second client to fail or wait", |s| {
        matches!(
            s,
            SessionState::Failed { .. }
                | SessionState::Reconnecting { .. }
                | SessionState::Disconnected { .. }
        )
    })
    .await;
    wait_until("the refusal counted", Duration::from_secs(10), || host.stats().refused >= 1).await;

    // The client's window size reaches the host as a new desktop size.
    handle
        .send(SessionCommand::Resize(drift_core::ViewGeometry {
            points: Size::new(1001.0, 701.0),
            backing_scale: 1.0,
        }))
        .unwrap();
    wait_until("the layout at the host", Duration::from_secs(10), || {
        host.stats().layouts.last() == Some(&Size::new(1000, 700))
    })
    .await;
    // …and the client gets a new 1000×700 output with a fresh picture on it.
    wait_until("a 1000×700 reset and picture at the client", Duration::from_secs(15), || {
        let calls = frames.calls();
        let reset = calls
            .iter()
            .rposition(|c| matches!(c, FrameSinkCall::Reset { output } if *output == Size::new(1000, 700)));
        reset.is_some_and(|i| calls[i..].iter().any(|c| matches!(c, FrameSinkCall::BlitNv12 { .. })))
    })
    .await;

    let _ = handle.send(SessionCommand::Close);
    wait_until("the session to end", Duration::from_secs(10), || host.stats().sessions_ended >= 1).await;
    assert_eq!(host.stats().sessions_served, 1);
}
