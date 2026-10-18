//! HOST-8 / IOS-9 Red: pairing end to end on loopback. Drift's client pairs with a code,
//! reconnects with its device key, and the host refuses unpaired devices, wrong codes, clients
//! that never authenticate, and a client whose pin does not match never sends its code.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use drift_apple::pairing::DeviceKey;
use drift_core::pairing::ShortCode;
use drift_core::{
    CertFingerprint, ConnectMode, ConnectionProfile, DisconnectReason, SessionState, Size, SystemClock,
};
use drift_ext::pairing::{AuthStatus, ClientPairing, PairStatus, PairingOutcome};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::pairing::{DeviceStore, Pairing};
use drift_host::server::{Host, HostOptions};
use drift_rdp::{SessionCommand, SessionEvent, SessionEvents, SessionHandle, SessionOptions, SessionSecrets};
use drift_testkit::{FrameLog, FrameSinkCall, PresentMode, RecordingFrameSink};

const USER: &str = "drift-host-test";
const PASS: &str = "fake-host8-password";

fn profile(port: u16, pin: CertFingerprint) -> ConnectionProfile {
    let mut p = ConnectionProfile::new("Paired Mac", "127.0.0.1", ConnectMode::Headless);
    p.port = port;
    USER.clone_into(&mut p.rdp_username);
    p.cert_pin = Some(pin);
    p
}

async fn wait_until(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_state(
    events: &mut SessionEvents,
    what: &str,
    pred: impl Fn(&SessionState) -> bool,
) -> SessionState {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SessionEvent::State(s))) if pred(&s) => return s,
            Ok(Some(_)) => {}
            other => panic!("no {what}: {other:?}"),
        }
    }
}

struct Client {
    handle: SessionHandle,
    events: SessionEvents,
    frames: FrameLog,
}

fn connect(port: u16, pin: CertFingerprint, pairing: Option<ClientPairing>) -> Client {
    let (sink, frames) = RecordingFrameSink::new(PresentMode::Immediate);
    let (handle, events) = drift_rdp::spawn_session(
        profile(port, pin),
        SessionSecrets::new(PASS),
        Box::new(sink),
        Arc::new(SystemClock),
        SessionOptions {
            connect_timeout: Duration::from_secs(15),
            reconnect_seed: Some(3),
            drift_mac: pairing.is_some().then(drift_ext::channel::Link::new),
            pairing,
            ..SessionOptions::default()
        },
    );
    Client { handle, events, frames }
}

fn has_picture(frames: &FrameLog) -> bool {
    frames.calls().iter().any(|c| matches!(c, FrameSinkCall::BlitNv12 { .. }))
}

async fn close(client: Client) {
    let _ = client.handle.send(SessionCommand::Close);
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// The session fails without retrying, because the host refused the device.
async fn refused(mut client: Client, why: &str) -> Client {
    let state = wait_state(&mut client.events, why, |s| matches!(s, SessionState::Failed { .. })).await;
    match state {
        SessionState::Failed { reason: DisconnectReason::PairingRefused(message) } => {
            assert!(!message.is_empty(), "{why}");
        }
        other => panic!("{why}: {other:?}"),
    }
    assert!(!has_picture(&client.frames), "{why}: the host showed nothing before refusing");
    client
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_pairs_reconnects_and_strangers_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let pin = identity.fingerprint();
    let pairing =
        Pairing::new(DeviceStore::memory(), Arc::new(SystemClock)).with_auth_timeout(Duration::from_secs(3));
    let code = pairing.open_window_with(ShortCode::from_body(4_242_424).unwrap());
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
    })
    .unwrap();
    let port = host.port();
    let device = Arc::new(DeviceKey::generate().unwrap());

    // 1. Pairing with the code on the Mac's screen.
    let first = ClientPairing::new(device.clone(), *pin.as_bytes(), "Test iPad").with_code(code.as_str());
    let mut client = connect(port, pin, Some(first.clone()));
    wait_state(&mut client.events, "connected", |s| matches!(s, SessionState::Connected { .. })).await;
    wait_until("paired", Duration::from_secs(10), || first.outcome() == Some(PairingOutcome::Paired)).await;
    wait_until("the picture after pairing", Duration::from_secs(15), || has_picture(&client.frames)).await;
    assert_eq!(pairing.devices().len(), 1);
    assert_eq!(pairing.devices()[0].name, "Test iPad");
    close(client).await;

    // 2. The same device reconnects with its key alone.
    let again = ClientPairing::new(device.clone(), *pin.as_bytes(), "Test iPad");
    let mut client = connect(port, pin, Some(again.clone()));
    wait_until("authenticated", Duration::from_secs(15), || {
        again.outcome() == Some(PairingOutcome::Authenticated)
    })
    .await;
    wait_until("the picture after authenticating", Duration::from_secs(15), || has_picture(&client.frames))
        .await;
    wait_state(&mut client.events, "connected", |s| matches!(s, SessionState::Connected { .. })).await;
    close(client).await;

    // 3. A device that never paired is refused.
    let stranger = ClientPairing::new(Arc::new(DeviceKey::generate().unwrap()), *pin.as_bytes(), "Stranger");
    let _ = refused(connect(port, pin, Some(stranger.clone())), "an unpaired device").await;
    assert_eq!(stranger.outcome(), Some(PairingOutcome::AuthRefused(AuthStatus::UnknownDevice)));

    // 4. A wrong code is refused (the window is closed after the first pairing anyway; reopen it).
    let open = pairing.open_window_with(ShortCode::from_body(1_010_101).unwrap());
    let wrong = ShortCode::from_body(2_020_202).unwrap();
    assert_ne!(open, wrong);
    let guesser = ClientPairing::new(Arc::new(DeviceKey::generate().unwrap()), *pin.as_bytes(), "Guesser")
        .with_code(wrong.as_str());
    let _ = refused(connect(port, pin, Some(guesser.clone())), "a wrong code").await;
    assert_eq!(guesser.outcome(), Some(PairingOutcome::PairRefused(PairStatus::WrongCode)));
    assert_eq!(pairing.wrong_codes(), 1);

    // 5. A client whose pin does not match the host stops at TLS: the code never reaches the Mac.
    let other_pin = CertFingerprint::from_bytes([0xAB; 32]);
    let mismatched =
        ClientPairing::new(device.clone(), *other_pin.as_bytes(), "Test iPad").with_code(open.as_str());
    let mut client = connect(port, other_pin, Some(mismatched.clone()));
    let state =
        wait_state(&mut client.events, "a pin mismatch", |s| matches!(s, SessionState::Failed { .. })).await;
    assert_eq!(state, SessionState::Failed { reason: DisconnectReason::CertMismatch });
    assert_eq!(mismatched.outcome(), None);
    assert_eq!(pairing.wrong_codes(), 1, "nothing was tried");
    assert_eq!(pairing.code(), Some(open), "the window is still open");

    // 6. A plain RDP client that never opens Drift::Mac::v1 sees nothing and is disconnected.
    let mut plain = connect(port, pin, None);
    wait_until("the host to drop the unauthenticated session", Duration::from_secs(15), || {
        host.stats().auth_timeouts >= 1
    })
    .await;
    assert!(!has_picture(&plain.frames));
    let _ = plain.handle.send(SessionCommand::Close);
    let _ = plain.events.recv().await;

    let stats = host.stats();
    assert_eq!(stats.authorized, 2, "{stats:?}");
    assert_eq!(stats.auth_refused, 2, "{stats:?}");
}
