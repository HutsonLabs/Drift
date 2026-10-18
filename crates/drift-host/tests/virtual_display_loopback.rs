//! HOST-10 Red: a client's layout reaches the host's display manager, and the host records
//! where it captures from.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_core::{ConnectMode, ConnectionProfile, SessionState, Size, SystemClock};
use drift_host::display::{DisplayManager, DisplayTarget, LayoutRequest};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::server::{Host, HostOptions};
use drift_host::vdisplay_plan::plan;
use drift_rdp::{SessionCommand, SessionEvent, SessionOptions, SessionSecrets};
use drift_testkit::{PresentMode, RecordingFrameSink};

const USER: &str = "drift-host-test";
const PASS: &str = "fake-host10-password";

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<LayoutRequest>>>);

impl DisplayManager for Recorder {
    fn apply(&mut self, request: &LayoutRequest) -> DisplayTarget {
        self.0.lock().unwrap().push(request.clone());
        DisplayTarget::Virtual { display_id: 77, plan: plan(request) }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_layout_reaches_the_display_manager() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let recorder = Recorder::default();
    let host = Host::start(HostOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        identity: identity.clone(),
        username: USER.into(),
        password: zeroize::Zeroizing::new(PASS.to_owned()),
        desktop: Size::new(1280, 800),
        frames: Box::new(|| Box::new(TestPattern::new())),
        live: None,
        input: Box::new(RecordingInput::default()),
        clipboard: None,
        audio: None,
        displays: Box::new(recorder.clone()),
    })
    .unwrap();
    let mut profile = ConnectionProfile::new("Drift Host", "127.0.0.1", ConnectMode::Headless);
    profile.port = host.port();
    USER.clone_into(&mut profile.rdp_username);
    profile.cert_pin = Some(identity.fingerprint());
    let (handle, mut events) = drift_rdp::spawn_session(
        profile,
        SessionSecrets::new(PASS),
        Box::new(RecordingFrameSink::new(PresentMode::Immediate).0),
        Arc::new(SystemClock),
        SessionOptions {
            connect_timeout: Duration::from_secs(15),
            reconnect_seed: Some(7),
            ..SessionOptions::default()
        },
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SessionEvent::State(SessionState::Connected { .. }))) => break,
            Ok(Some(_)) => {}
            other => panic!("not connected: {other:?}"),
        }
    }
    handle
        .send(SessionCommand::Resize(drift_core::ViewGeometry {
            points: Size::new(683.0, 512.0),
            backing_scale: 2.0,
        }))
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let seen = recorder.0.lock().unwrap().iter().any(|r| r.pixels == Size::new(1366, 1024));
        if seen {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the manager never saw 1366×1024: {:?}",
            recorder.0.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        matches!(host.stats().display, Some(DisplayTarget::Virtual { display_id: 77, .. })),
        "{:?}",
        host.stats().display
    );
    host.stop();
}
