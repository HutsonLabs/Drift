//! HOST-8 + HOST-11 Red: with pairing on, a connection that has not paired or authenticated
//! gets neither the Mac's clipboard nor its sound, and cannot write the Mac's pasteboard; once a
//! device pairs, both flow. (Picture and input were already gated by HOST-8.)
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_apple::pairing::DeviceKey;
use drift_clipboard::poll::PasteboardPort;
use drift_clipboard::{ClipboardContents, ClipboardItem};
use drift_core::pairing::ShortCode;
use drift_core::{ClipboardPrefs, ConnectMode, ConnectionProfile, SessionState, Size, SystemClock};
use drift_ext::pairing::{ClientPairing, PairingOutcome};
use drift_host::audio::{AudioSource, ToneSource};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::pairing::{DeviceStore, Pairing};
use drift_host::server::{Host, HostOptions};
use drift_rdp::audio::{AudioOutput, AudioSink};
use drift_rdp::{SessionCommand, SessionEvent, SessionEvents, SessionOptions, SessionSecrets};
use drift_testkit::{PresentMode, RecordingFrameSink};

const USER: &str = "drift-host-test";
const PASS: &str = "fake-gate-password";

#[derive(Debug, Default)]
struct Board {
    change_count: i64,
    contents: ClipboardContents,
    writes: usize,
}

#[derive(Clone, Default)]
struct FakePasteboard(Arc<Mutex<Board>>);

impl PasteboardPort for FakePasteboard {
    fn change_count(&self) -> i64 {
        self.0.lock().unwrap().change_count
    }
    fn read(&self, _level: ClipboardPrefs) -> ClipboardContents {
        self.0.lock().unwrap().contents.clone()
    }
    fn write(&self, _contents: &ClipboardContents) -> i64 {
        let mut b = self.0.lock().unwrap();
        b.writes += 1;
        b.change_count += 1;
        b.change_count
    }
}

/// Counts the sound bytes the client plays.
#[derive(Clone, Default)]
struct Heard(Arc<Mutex<usize>>);

impl AudioSink for Heard {
    fn start(&self, _format: drift_core::audio::PcmFormat) {}
    fn play(&self, pcm: &[u8]) {
        *self.0.lock().unwrap() += pcm.len();
    }
    fn stop(&self) {}
}

fn connect(
    host: &Host,
    identity: &HostIdentity,
    pairing: Option<ClientPairing>,
    heard: &Heard,
) -> (drift_rdp::SessionHandle, SessionEvents) {
    let mut p = ConnectionProfile::new("Drift Host", "127.0.0.1", ConnectMode::Headless);
    p.port = host.port();
    USER.clone_into(&mut p.rdp_username);
    p.cert_pin = Some(identity.fingerprint());
    drift_rdp::spawn_session(
        p,
        SessionSecrets::new(PASS),
        Box::new(RecordingFrameSink::new(PresentMode::Immediate).0),
        Arc::new(SystemClock),
        SessionOptions {
            connect_timeout: Duration::from_secs(15),
            reconnect_seed: Some(5),
            drift_mac: Some(drift_ext::channel::Link::new()),
            pairing,
            audio: Some(AudioOutput::new(heard.clone())),
            ..SessionOptions::default()
        },
    )
}

/// Every event until `within` passes.
async fn drain(events: &mut SessionEvents, within: Duration) -> Vec<SessionEvent> {
    let deadline = tokio::time::Instant::now() + within;
    let mut out = Vec::new();
    while let Ok(Some(e)) = tokio::time::timeout_at(deadline, events.recv()).await {
        out.push(e);
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn clipboard_and_sound_wait_for_the_device_to_be_authorized() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let pin = identity.fingerprint();
    let pairing =
        Pairing::new(DeviceStore::memory(), Arc::new(SystemClock)).with_auth_timeout(Duration::from_secs(3));
    let code = pairing.open_window_with(ShortCode::from_body(4_242_424).unwrap());
    let board = FakePasteboard::default();
    // The host announces its clipboard when it changes (HOST-11).
    let copy_on_host = |text: &str| {
        let mut b = board.0.lock().unwrap();
        b.change_count += 1;
        b.contents = ClipboardContents { items: vec![ClipboardItem::Text(text.into())] };
    };
    let port = board.clone();
    let host = Host::start(HostOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        identity: identity.clone(),
        username: USER.into(),
        password: zeroize::Zeroizing::new(PASS.to_owned()),
        desktop: Size::new(1280, 800),
        frames: Box::new(|| Box::new(TestPattern::new())),
        live: None,
        input: Box::new(RecordingInput::default()),
        clipboard: Some(Box::new(move || Box::new(port.clone()) as Box<dyn PasteboardPort>)),
        audio: Some(Box::new(|| Box::new(ToneSource::new()) as Box<dyn AudioSource>)),
        displays: Box::new(drift_host::display::PhysicalOnly),
        pairing: Some(pairing.clone()),
    })
    .unwrap();

    // A stranger: passes NLA, never pairs. It gets no clipboard and no sound before it is dropped.
    let stranger_heard = Heard::default();
    let (handle, mut events) = connect(&host, &identity, None, &stranger_heard);
    let _ = handle.send(SessionCommand::Focus(true));
    let _ = handle.send(SessionCommand::ClipboardLocalChanged(ClipboardContents {
        items: vec![ClipboardItem::Text("from a stranger".into())],
    }));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    copy_on_host("secret on the host");
    let seen = drain(&mut events, Duration::from_secs(3)).await;
    assert!(
        !seen.iter().any(|e| matches!(e, SessionEvent::ClipboardRemote(_))),
        "the host's clipboard reached an unauthorized device"
    );
    assert_eq!(host.stats().audio_waves, 0, "sound reached an unauthorized device: {:?}", host.stats());
    assert_eq!(*stranger_heard.0.lock().unwrap(), 0);
    assert_eq!(board.0.lock().unwrap().writes, 0, "an unauthorized device wrote the Mac's pasteboard");
    // It was turned away (refused over Drift::Mac::v1, or dropped for not authenticating).
    let stats = host.stats();
    assert!(stats.auth_refused + stats.auth_timeouts >= 1 && stats.authorized == 0, "{stats:?}");
    let _ = handle.send(SessionCommand::Close);

    // A device that pairs gets both.
    let device = Arc::new(DeviceKey::generate().unwrap());
    let first = ClientPairing::new(device, *pin.as_bytes(), "Test iPad").with_code(code.as_str());
    let heard = Heard::default();
    let (handle, mut events) = connect(&host, &identity, Some(first.clone()), &heard);
    let _ = handle.send(SessionCommand::Focus(true));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while first.outcome() != Some(PairingOutcome::Paired) {
        assert!(tokio::time::Instant::now() < deadline, "not paired");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Past the echo window of the client's initial (empty) clipboard, the Mac copies.
    tokio::time::sleep(drift_clipboard::sync::ECHO_WINDOW + Duration::from_millis(500)).await;
    copy_on_host("copied after pairing");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut got_clipboard = false;
    while !got_clipboard {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SessionEvent::ClipboardRemote(c))) => {
                got_clipboard = c.text() == Some("secret on the host")
            }
            Ok(Some(SessionEvent::State(SessionState::Failed { reason }))) => panic!("failed: {reason:?}"),
            Ok(Some(_)) => {}
            other => panic!("no clipboard after pairing: {other:?}"),
        }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while host.stats().audio_waves == 0 || *heard.0.lock().unwrap() == 0 {
        assert!(tokio::time::Instant::now() < deadline, "no sound after pairing: {:?}", host.stats());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = handle.send(SessionCommand::Close);
}
