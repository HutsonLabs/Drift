//! HOST-15 Red: pairing approved on the Mac, end to end on loopback (ADR `HOST-15-guided-setup`).
//! A device that asks waits past the usual authentication deadline while the Mac's user
//! decides; Allow shows it the desktop, Don't Allow and no answer refuse it without a retry.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use drift_apple::pairing::DeviceKey;
use drift_core::{
    CertFingerprint, ConnectMode, ConnectStage, ConnectionProfile, DisconnectReason, SessionState, Size,
    SystemClock,
};
use drift_ext::pairing::{ClientPairing, PairStatus, PairingOutcome};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::pairing::{DeviceStore, Pairing};
use drift_host::server::{Host, HostOptions};
use drift_rdp::{SessionCommand, SessionEvent, SessionEvents, SessionHandle, SessionOptions, SessionSecrets};
use drift_testkit::{FrameLog, FrameSinkCall, PresentMode, RecordingFrameSink};

const USER: &str = "drift-host-test";
const PASS: &str = "fake-host15-password";

struct Client {
    handle: SessionHandle,
    events: SessionEvents,
    frames: FrameLog,
}

fn connect(port: u16, pin: CertFingerprint, pairing: ClientPairing) -> Client {
    let mut p = ConnectionProfile::new("Paired Mac", "127.0.0.1", ConnectMode::Headless);
    p.port = port;
    USER.clone_into(&mut p.rdp_username);
    p.cert_pin = Some(pin);
    let (sink, frames) = RecordingFrameSink::new(PresentMode::Immediate);
    let (handle, events) = drift_rdp::spawn_session(
        p,
        SessionSecrets::new(PASS),
        Box::new(sink),
        Arc::new(SystemClock),
        SessionOptions {
            connect_timeout: Duration::from_secs(15),
            reconnect_seed: Some(5),
            drift_mac: Some(drift_ext::channel::Link::new()),
            pairing: Some(pairing),
            ..SessionOptions::default()
        },
    );
    Client { handle, events, frames }
}

fn has_picture(frames: &FrameLog) -> bool {
    frames.calls().iter().any(|c| matches!(c, FrameSinkCall::BlitNv12 { .. }))
}

async fn wait_until(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn failed(client: &mut Client, what: &str) -> DisconnectReason {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        match tokio::time::timeout_at(deadline, client.events.recv()).await {
            Ok(Some(SessionEvent::State(SessionState::Failed { reason }))) => return reason,
            Ok(Some(_)) => {}
            other => panic!("no failure for {what}: {other:?}"),
        }
    }
}

async fn close(client: Client) {
    let _ = client.handle.send(SessionCommand::Close);
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_the_mac_allows_pairs_and_others_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let pin = identity.fingerprint();
    // A short authentication deadline: a device waiting for the Mac's user must outlive it.
    let pairing = Pairing::new(DeviceStore::memory(), Arc::new(SystemClock))
        .with_auth_timeout(Duration::from_secs(2))
        .with_approval_timeout(Duration::from_secs(6));
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
        displays: Box::new(drift_host::display::PhysicalOnly),
        pairing: Some(pairing.clone()),
        menus: None,
    })
    .unwrap();
    let port = host.port();
    let device = Arc::new(DeviceKey::generate().unwrap());

    // 1. Allow, after the usual deadline has passed.
    let asking = ClientPairing::new(device.clone(), *pin.as_bytes(), "Test iPad").asking_approval();
    let mut client = connect(port, pin, asking.clone());
    wait_until("the request on the Mac", Duration::from_secs(15), || !pairing.approvals().is_empty()).await;
    assert!(asking.awaiting_approval(), "the device knows it waits for the Mac");
    // The device says so instead of showing an empty desktop.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(deadline, client.events.recv()).await {
            Ok(Some(SessionEvent::State(SessionState::Connecting {
                stage: ConnectStage::Approval, ..
            }))) => break,
            Ok(Some(SessionEvent::State(s @ SessionState::Connected { .. }))) => panic!("{s:?} before Allow"),
            Ok(Some(_)) => {}
            other => panic!("no approval stage: {other:?}"),
        }
    }
    assert_eq!(pairing.approvals()[0].device_name, "Test iPad");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!has_picture(&client.frames), "nothing is shown before Allow");
    assert!(pairing.approve(pairing.approvals()[0].id));
    wait_until("paired", Duration::from_secs(10), || asking.outcome() == Some(PairingOutcome::Paired)).await;
    wait_until("the picture after Allow", Duration::from_secs(15), || has_picture(&client.frames)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(deadline, client.events.recv()).await {
            Ok(Some(SessionEvent::State(SessionState::Connected { .. }))) => break,
            Ok(Some(_)) => {}
            other => panic!("not connected after Allow: {other:?}"),
        }
    }
    assert_eq!(pairing.devices().len(), 1);
    close(client).await;

    // 2. The device reconnects with its key alone; nobody is asked.
    let again = ClientPairing::new(device.clone(), *pin.as_bytes(), "Test iPad");
    let client = connect(port, pin, again.clone());
    wait_until("authenticated", Duration::from_secs(15), || {
        again.outcome() == Some(PairingOutcome::Authenticated)
    })
    .await;
    assert!(pairing.approvals().is_empty());
    close(client).await;

    // 3. Don't Allow.
    let stranger = ClientPairing::new(Arc::new(DeviceKey::generate().unwrap()), *pin.as_bytes(), "Stranger")
        .asking_approval();
    let mut client = connect(port, pin, stranger.clone());
    wait_until("the stranger's request", Duration::from_secs(15), || !pairing.approvals().is_empty()).await;
    assert!(pairing.decline(pairing.approvals()[0].id));
    let reason = failed(&mut client, "Don't Allow").await;
    assert!(
        matches!(&reason, DisconnectReason::PairingRefused(why) if why.contains("declined")),
        "{reason:?}"
    );
    assert_eq!(stranger.outcome(), Some(PairingOutcome::PairRefused(PairStatus::Declined)));
    assert!(!has_picture(&client.frames));
    close(client).await;

    // 4. Nobody answers.
    let ignored = ClientPairing::new(Arc::new(DeviceKey::generate().unwrap()), *pin.as_bytes(), "Ignored")
        .asking_approval();
    let mut client = connect(port, pin, ignored.clone());
    let reason = failed(&mut client, "no answer").await;
    assert!(
        matches!(&reason, DisconnectReason::PairingRefused(why) if why.contains("in time")),
        "{reason:?}"
    );
    assert_eq!(ignored.outcome(), Some(PairingOutcome::PairRefused(PairStatus::NoAnswer)));
    close(client).await;

    assert_eq!(pairing.devices().len(), 1);
    let stats = host.stats();
    assert_eq!(stats.authorized, 2, "{stats:?}");
    assert_eq!(stats.auth_timeouts, 0, "a waiting device is not dropped as silent: {stats:?}");
}
