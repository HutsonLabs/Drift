//! Shared setup for the HOST-11 loopback tests (included with `#[path]`).
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use drift_core::{ConnectMode, ConnectionProfile, SessionState, Size, SystemClock};
use drift_host::audio::AudioSourceFactory;
use drift_host::clipboard::PasteboardFactory;
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::server::{Host, HostOptions};
use drift_rdp::{SessionEvent, SessionEvents, SessionHandle, SessionOptions, SessionSecrets};
use drift_testkit::{PresentMode, RecordingFrameSink};

pub const USER: &str = "drift-host-test";
pub const PASS: &str = "fake-host11-password";

pub fn start_host(
    dir: &std::path::Path,
    clipboard: Option<PasteboardFactory>,
    audio: Option<AudioSourceFactory>,
) -> (Host, HostIdentity) {
    let identity = HostIdentity::load_or_create(dir).unwrap();
    let host = Host::start(HostOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        identity: identity.clone(),
        username: USER.into(),
        password: zeroize::Zeroizing::new(PASS.to_owned()),
        desktop: Size::new(1280, 800),
        frames: Box::new(|| Box::new(TestPattern::new())),
        live: None,
        input: Box::new(RecordingInput::default()),
        clipboard,
        audio,
    })
    .unwrap();
    (host, identity)
}

pub fn connect(
    host: &Host,
    identity: &HostIdentity,
    options: SessionOptions,
) -> (SessionHandle, SessionEvents) {
    let mut p = ConnectionProfile::new("Drift Host", "127.0.0.1", ConnectMode::Headless);
    p.port = host.port();
    USER.clone_into(&mut p.rdp_username);
    p.cert_pin = Some(identity.fingerprint());
    drift_rdp::spawn_session(
        p,
        SessionSecrets::new(PASS),
        Box::new(RecordingFrameSink::new(PresentMode::Immediate).0),
        Arc::new(SystemClock),
        SessionOptions { connect_timeout: Duration::from_secs(15), reconnect_seed: Some(7), ..options },
    )
}

pub async fn wait_until(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Waits for an event matching `pred`, returning it.
pub async fn wait_event(
    events: &mut SessionEvents,
    what: &str,
    pred: impl Fn(&SessionEvent) -> bool,
) -> SessionEvent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(e)) if pred(&e) => return e,
            Ok(Some(_)) => {}
            other => panic!("no {what}: {other:?}"),
        }
    }
}

pub fn connected(e: &SessionEvent) -> bool {
    matches!(e, SessionEvent::State(SessionState::Connected { .. }))
}
