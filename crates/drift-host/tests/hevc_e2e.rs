//! EXT-5 Red: HEVC between a paired Drift client and Drift Host on loopback (ADR
//! `EXT-5-hevc-hdr`). The client offers the HEVC profiles it decodes on `Drift::Mac::v1`; the host
//! picks one its encoder has and streams the live picture with it, as `WireToSurface1` frames
//! with Drift's private codec id. A Drift client that did not offer HEVC (no pairing, so no
//! codec negotiation) stays on H.264. Synthetic sources only: no Screen Recording grant needed.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_apple::pairing::DeviceKey;
use drift_core::pairing::ShortCode;
use drift_core::video::HevcProfile;
use drift_core::{CertFingerprint, ConnectMode, ConnectionProfile, SessionState, Size, SystemClock};
use drift_ext::pairing::ClientPairing;
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::pairing::{DeviceStore, Pairing};
use drift_host::pipeline::{LiveEncoder, LiveHevcEncoder, LiveRequest, LiveSource, MovingSquare};
use drift_host::server::{Host, HostOptions, LiveFactory};
use drift_rdp::{SessionCommand, SessionEvent, SessionEvents, SessionOptions, SessionSecrets};
use drift_testkit::{FrameLog, FrameSinkCall, PresentMode, RecordingFrameSink};

const USER: &str = "drift-host-test";
const PASS: &str = "fake-ext5-password";
const DESKTOP: Size<u32> = Size { width: 640, height: 400 };

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

async fn wait_connected(events: &mut SessionEvents) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SessionEvent::State(SessionState::Connected { .. }))) => return,
            Ok(Some(_)) => {}
            other => panic!("not connected: {other:?}"),
        }
    }
}

/// Records every request; HEVC requests get the HEVC encoder over the synthetic screen.
fn factory(requests: Arc<Mutex<Vec<LiveRequest>>>) -> LiveFactory {
    Box::new(move |request: LiveRequest| {
        requests.lock().unwrap().push(request);
        match request.hevc {
            Some(selection) => {
                let raw = if matches!(selection.profile, HevcProfile::Main444 | HevcProfile::Main44410) {
                    MovingSquare::bgra(DESKTOP, 30)
                } else {
                    MovingSquare::new(DESKTOP, 30)
                };
                LiveHevcEncoder::new(raw, 30, selection).map(|e| Box::new(e) as Box<dyn LiveSource>)
            }
            None => LiveEncoder::new(MovingSquare::new(DESKTOP, 30), 30)
                .map(|e| Box::new(e) as Box<dyn LiveSource>),
        }
    })
}

fn host(identity: &HostIdentity, live: LiveFactory, pairing: Option<Pairing>) -> Host {
    Host::start(HostOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        identity: identity.clone(),
        username: USER.into(),
        password: zeroize::Zeroizing::new(PASS.to_owned()),
        desktop: DESKTOP,
        frames: Box::new(|| Box::new(TestPattern::new())),
        live: Some(live),
        input: Box::new(RecordingInput::default()),
        clipboard: None,
        audio: None,
        displays: Box::new(drift_host::display::PhysicalOnly),
        pairing,
    })
    .unwrap()
}

fn picture_calls(frames: &FrameLog) -> usize {
    frames
        .calls()
        .iter()
        .filter(|c| matches!(c, FrameSinkCall::BlitNv12 { .. } | FrameSinkCall::BlitBgra { .. }))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paired_client_that_offers_hevc_gets_hevc() {
    if !drift_video::hevc::probe_encoder(DESKTOP).contains(&HevcProfile::Main) {
        eprintln!("skipped: no HEVC encoder");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let pin = identity.fingerprint();
    let pairing = Pairing::new(DeviceStore::memory(), Arc::new(SystemClock));
    let code = pairing.open_window_with(ShortCode::from_body(5_151_515).unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let host = host(&identity, factory(Arc::clone(&requests)), Some(pairing));

    let device = Arc::new(DeviceKey::generate().unwrap());
    let client_pairing = ClientPairing::new(device, *pin.as_bytes(), "HEVC iPad").with_code(code.as_str());
    let (sink, frames) = RecordingFrameSink::new(PresentMode::Immediate);
    let (handle, mut events) = drift_rdp::spawn_session(
        profile(host.port(), pin),
        SessionSecrets::new(PASS),
        Box::new(sink),
        Arc::new(SystemClock),
        SessionOptions {
            connect_timeout: Duration::from_secs(15),
            drift_mac: Some(drift_ext::channel::Link::new()),
            pairing: Some(client_pairing),
            gfx_caps: drift_gfx::GfxCaps::DriftHost,
            ..SessionOptions::default()
        },
    );
    wait_connected(&mut events).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    while host.stats().hevc_frames < 10 {
        if tokio::time::Instant::now() >= deadline {
            let mut seen = Vec::new();
            while let Ok(Some(e)) = tokio::time::timeout(Duration::from_millis(10), events.recv()).await {
                seen.push(format!("{e:?}"));
            }
            panic!(
                "no HEVC frames: {:?}, requests {:?}, client events {seen:?}",
                host.stats(),
                requests.lock().unwrap()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    wait_until("the client drew them", Duration::from_secs(15), || picture_calls(&frames) >= 10).await;
    let stats = host.stats();
    assert!(stats.hevc_profile.is_some(), "the host reports what it picked: {stats:?}");
    assert!(
        requests.lock().unwrap().iter().any(|r| r.hevc.is_some()),
        "the live picture was (re)opened for HEVC"
    );
    assert!(stats.frames_acked > 0, "the client acknowledged frames");
    let _ = handle.send(SessionCommand::Close);
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_without_codec_negotiation_stays_on_h264() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let host = host(&identity, factory(Arc::clone(&requests)), None);
    let (sink, frames) = RecordingFrameSink::new(PresentMode::Immediate);
    let (handle, mut events) = drift_rdp::spawn_session(
        profile(host.port(), identity.fingerprint()),
        SessionSecrets::new(PASS),
        Box::new(sink),
        Arc::new(SystemClock),
        SessionOptions {
            connect_timeout: Duration::from_secs(15),
            drift_mac: Some(drift_ext::channel::Link::new()),
            ..SessionOptions::default()
        },
    );
    wait_connected(&mut events).await;
    wait_until("H.264 frames", Duration::from_secs(20), || host.stats().frames_sent >= 10).await;
    wait_until("the client drew them", Duration::from_secs(15), || picture_calls(&frames) >= 5).await;
    let stats = host.stats();
    assert_eq!(stats.hevc_frames, 0);
    assert_eq!(stats.hevc_profile, None);
    assert!(requests.lock().unwrap().iter().all(|r| r.hevc.is_none()));
    let _ = handle.send(SessionCommand::Close);
    tokio::time::sleep(Duration::from_millis(300)).await;
}
