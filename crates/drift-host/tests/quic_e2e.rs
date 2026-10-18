//! HOST-12 Red: a paired Drift client reaches Drift Host over QUIC first (the RDP connection on
//! QUIC's main stream, through a lossy link), falls back to TCP when UDP gets no answer, and any
//! other profile uses TCP only (ADR `IOS-0` decision 6, ADR `HOST-12-quic-transport`).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use drift_apple::pairing::DeviceKey;
use drift_core::pairing::ShortCode;
use drift_core::{ConnectMode, ConnectionProfile, SessionState, Size, SystemClock};
use drift_ext::pairing::{ClientPairing, PairingOutcome};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::pairing::{DeviceStore, Pairing};
use drift_host::server::{Host, HostOptions};
use drift_rdp::{SessionCommand, SessionEvent, SessionEvents, SessionOptions, SessionSecrets};
use drift_testkit::{FrameLog, FrameSinkCall, PresentMode, RecordingFrameSink};
use drift_transport::lossy::{LinkConfig, LossyUdpProxy};
use uuid::Uuid;

const USER: &str = "drift-host-test";
const PASS: &str = "fake-host12-password";

async fn wait_until(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_connected(events: &mut SessionEvents) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SessionEvent::State(SessionState::Connected { .. }))) => return,
            Ok(Some(_)) => {}
            other => panic!("not connected: {other:?}"),
        }
    }
}

fn has_picture(frames: &FrameLog) -> bool {
    frames.calls().iter().any(|c| matches!(c, FrameSinkCall::BlitNv12 { .. }))
}

fn start_host(dir: &std::path::Path, quic: bool) -> (Host, HostIdentity, Pairing) {
    let identity = HostIdentity::load_or_create(dir).unwrap();
    let pairing = Pairing::new(DeviceStore::memory(), Arc::new(SystemClock));
    let host = Host::start(HostOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        identity: identity.clone(),
        username: USER.into(),
        password: zeroize::Zeroizing::new(PASS.to_owned()),
        desktop: Size::new(1280, 800),
        frames: Box::new(|| Box::new(TestPattern::new())),
        input: Box::new(RecordingInput::default()),
        pairing: Some(pairing.clone()),
    })
    .unwrap();
    if !quic {
        host.stop_quic();
    }
    (host, identity, pairing)
}

fn paired_profile(port: u16, identity: &HostIdentity) -> ConnectionProfile {
    let mut p = ConnectionProfile::new("Paired Mac", "127.0.0.1", ConnectMode::Headless);
    p.port = port;
    USER.clone_into(&mut p.rdp_username);
    p.cert_pin = Some(identity.fingerprint());
    p.paired_host = Some(Uuid::from_u128(0x12));
    p
}

fn connect(
    profile: ConnectionProfile,
    pairing: ClientPairing,
) -> (drift_rdp::SessionHandle, SessionEvents, FrameLog) {
    let (sink, frames) = RecordingFrameSink::new(PresentMode::Immediate);
    let (handle, events) = drift_rdp::spawn_session(
        profile,
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
    (handle, events, frames)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paired_client_uses_quic_through_a_lossy_link() {
    let dir = tempfile::tempdir().unwrap();
    let (host, identity, pairing) = start_host(dir.path(), true);
    let code = pairing.open_window_with(ShortCode::from_body(1_212_121).unwrap());
    let link = LinkConfig {
        loss_percent: 2.0,
        one_way_delay: Duration::from_millis(5),
        jitter: Duration::ZERO,
        seed: 9,
    };
    let proxy =
        LossyUdpProxy::start(host.quic_addr().expect("the host listens on QUIC"), link).await.unwrap();

    // The proxy only carries UDP: if the client did not use QUIC it could not connect at all.
    let device = Arc::new(DeviceKey::generate().unwrap());
    let client_pairing =
        ClientPairing::new(device, *identity.fingerprint().as_bytes(), "Test iPad").with_code(code.as_str());
    let (handle, mut events, frames) =
        connect(paired_profile(proxy.addr().port(), &identity), client_pairing.clone());
    wait_connected(&mut events).await;
    wait_until("paired over QUIC", Duration::from_secs(15), || {
        client_pairing.outcome() == Some(PairingOutcome::Paired)
    })
    .await;
    wait_until("the picture over QUIC", Duration::from_secs(15), || has_picture(&frames)).await;
    let stats = host.stats();
    assert_eq!((stats.quic_sessions, stats.tcp_sessions), (1, 0), "{stats:?}");
    assert!(proxy.stats().dropped > 0, "the link really lost packets: {:?}", proxy.stats());
    let _ = handle.send(SessionCommand::Close);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_answer_on_udp_a_paired_client_falls_back_to_tcp() {
    let dir = tempfile::tempdir().unwrap();
    let (host, identity, pairing) = start_host(dir.path(), false);
    let code = pairing.open_window_with(ShortCode::from_body(3_434_343).unwrap());
    let device = Arc::new(DeviceKey::generate().unwrap());
    let client_pairing =
        ClientPairing::new(device, *identity.fingerprint().as_bytes(), "Test iPad").with_code(code.as_str());
    let (handle, mut events, frames) = connect(paired_profile(host.port(), &identity), client_pairing);
    wait_connected(&mut events).await;
    wait_until("the picture over TCP", Duration::from_secs(15), || has_picture(&frames)).await;
    let stats = host.stats();
    assert_eq!((stats.quic_sessions, stats.tcp_sessions), (0, 1), "{stats:?}");
    let _ = handle.send(SessionCommand::Close);
}
