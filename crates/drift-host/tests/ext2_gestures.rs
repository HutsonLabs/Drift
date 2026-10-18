//! EXT-2 Red: precise scrolling and gestures from a Drift client reach the host over
//! `Drift::Mac::v1`; a client without the channel sends today's wheel events instead.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use drift_core::{
    ConnectMode, ConnectionProfile, InputEvent, MomentumPhase, ScrollPhase, SessionState, Size,
    SwipeDirection, SystemClock,
};
use drift_ext::gestures::Gesture;
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::server::{Host, HostOptions};
use drift_rdp::{SessionCommand, SessionEvent, SessionOptions, SessionSecrets};
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
        menus: None,
    })
    .unwrap()
}

fn scroll(dy: i32, phase: ScrollPhase) -> InputEvent {
    InputEvent::PreciseScroll {
        dx: 0,
        dy,
        phase,
        momentum: MomentumPhase::None,
        wheel_v: dy / 50,
        wheel_h: 0,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn precise_scroll_and_swipes_ride_the_channel_to_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let input = RecordingInput::default();
    let host = start_host(&input, &identity);
    let link = drift_ext::channel::Link::new();
    let (handle, mut events) = drift_rdp::spawn_session(
        profile(host.port(), &identity),
        SessionSecrets::new(PASS),
        Box::new(RecordingFrameSink::new(PresentMode::Immediate).0),
        Arc::new(SystemClock),
        SessionOptions { drift_mac: Some(link.clone()), ..options() },
    );
    wait_state(&mut events, "connected", |s| matches!(s, SessionState::Connected { .. })).await;
    wait_until("Drift::Mac::v1 negotiated", Duration::from_secs(10), || {
        link.negotiated().is_some_and(|n| {
            n.capabilities
                .contains(drift_ext::wire::Capabilities::SCROLL | drift_ext::wire::Capabilities::GESTURES)
        })
    })
    .await;

    for event in [
        scroll(500, ScrollPhase::Began),
        scroll(700, ScrollPhase::Changed),
        scroll(0, ScrollPhase::Ended),
        InputEvent::Swipe { direction: SwipeDirection::Up, fingers: 3 },
    ] {
        handle.send(SessionCommand::Input(event)).unwrap();
    }
    wait_until("the gestures at the host", Duration::from_secs(10), || input.gestures().len() >= 4).await;
    assert_eq!(
        input.gestures(),
        vec![
            Gesture::Scroll { dx: 0, dy: 500, phase: ScrollPhase::Began, momentum: MomentumPhase::None },
            Gesture::Scroll { dx: 0, dy: 700, phase: ScrollPhase::Changed, momentum: MomentumPhase::None },
            Gesture::Scroll { dx: 0, dy: 0, phase: ScrollPhase::Ended, momentum: MomentumPhase::None },
            Gesture::Swipe { direction: SwipeDirection::Up, fingers: 3 },
        ]
    );
    assert_eq!(input.mouse_events(), 0, "no wheel events as well: the host would scroll twice");
    let _ = handle.send(SessionCommand::Close);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_the_channel_the_wheel_fallback_goes_and_gestures_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let input = RecordingInput::default();
    let host = start_host(&input, &identity);
    let (handle, mut events) = drift_rdp::spawn_session(
        profile(host.port(), &identity),
        SessionSecrets::new(PASS),
        Box::new(RecordingFrameSink::new(PresentMode::Immediate).0),
        Arc::new(SystemClock),
        options(),
    );
    wait_state(&mut events, "connected", |s| matches!(s, SessionState::Connected { .. })).await;
    handle
        .send(SessionCommand::Input(InputEvent::Swipe { direction: SwipeDirection::Up, fingers: 3 }))
        .unwrap();
    // 600 hundredths → 12 wheel units in one vertical wheel event.
    handle.send(SessionCommand::Input(scroll(600, ScrollPhase::Changed))).unwrap();
    wait_until("the wheel at the host", Duration::from_secs(10), || input.mouse_events() >= 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(input.mouse_events(), 1, "one wheel event, nothing for the swipe");
    assert!(input.gestures().is_empty());
    let _ = handle.send(SessionCommand::Close);
}

#[test]
fn the_injecting_sink_posts_shortcuts_and_pixel_scrolls_and_never_leaks_a_pinch() {
    use std::sync::Mutex;

    use drift_host::inject::InjectingInput;
    use drift_host::input::InputSink;
    use drift_inject::plan::{DisplayMap, InjectEvent};

    let posted: Arc<Mutex<Vec<InjectEvent>>> = Arc::default();
    let sink_posted = Arc::clone(&posted);
    let map = DisplayMap { origin: (0.0, 0.0), desktop: (1280, 800), points_per_pixel: 1.0 };
    let mut sink =
        InjectingInput::new(map, move |e: &InjectEvent| sink_posted.lock().unwrap().push(e.clone()));
    sink.gesture(Gesture::Swipe { direction: SwipeDirection::Up, fingers: 3 });
    sink.gesture(Gesture::Scroll {
        dx: 0,
        dy: 250,
        phase: ScrollPhase::Changed,
        momentum: MomentumPhase::None,
    });
    sink.gesture(Gesture::Pinch { magnification: 200, phase: ScrollPhase::Began });
    sink.session_ended();
    // A new session's pinch starts from zero: 200 left over must not add up with this one.
    sink.gesture(Gesture::Pinch { magnification: 100, phase: ScrollPhase::Changed });
    let posted = posted.lock().unwrap();
    assert_eq!(posted.len(), 5, "Control-Up as four key events, then one scroll: {posted:?}");
    assert!(matches!(posted[4], InjectEvent::Scroll { dy, .. } if (dy - 2.5).abs() < 1e-9));
}
