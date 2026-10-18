//! EXT-4 Red: a Drift client's quality preference reaches the host, which caps its quality and
//! answers with its status, which the client shows in its statistics.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use drift_core::{ConnectMode, ConnectionProfile, SessionState, Size, SystemClock};
use drift_ext::status::{QualityLevel, QualityPreference};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::server::{Host, HostOptions};
use drift_rdp::{SessionEvent, SessionOptions, SessionSecrets};
use drift_testkit::{PresentMode, RecordingFrameSink};

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

fn start_host(input: &RecordingInput, identity: &HostIdentity) -> Host {
    Host::start(HostOptions {
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
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_away_client_caps_the_hosts_quality_and_sees_its_status() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let input = RecordingInput::default();
    let host = start_host(&input, &identity);
    let link = drift_ext::channel::Link::new();
    let (_handle, mut events) = drift_rdp::spawn_session(
        profile(host.port(), &identity),
        SessionSecrets::new(PASS),
        Box::new(RecordingFrameSink::new(PresentMode::Immediate).0),
        Arc::new(SystemClock),
        SessionOptions {
            drift_mac: Some(link.clone()),
            quality: Some(QualityPreference::default_for(true)),
            ..options()
        },
    );
    wait_state(&mut events, "connected", |s| matches!(s, SessionState::Connected { .. })).await;
    wait_until("the cap at the host", Duration::from_secs(10), || {
        host.stats().quality_max == QualityLevel::Medium
    })
    .await;
    assert_eq!(host.stats().quality, QualityLevel::Medium, "{:?}", host.stats());
    // The host answers with its status; the client's statistics carry it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SessionEvent::Stats(s))) if s.host_quality == Some(QualityLevel::Medium) => break,
            Ok(Some(_)) => {}
            other => panic!("no statistics with the host's quality: {other:?}"),
        }
    }
}
