//! M6-1 Red: the SessionManager maps windows to session actors, routes events only to their
//! own window, closes gracefully and shuts down within the 2 s cap without leaking handles.
//!
//! Actors are fakes built from `SessionHandle::from_sender` (ADR M0-5); the host is a
//! recorder standing in for Tauri/AppKit.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_app::connections::ConnectionStatus;
use drift_app::manager::{CLOSE_CAP, ConnectPlan, Reconnect, SessionHost, SessionManager, ShutdownReport};
use drift_app::profiles::CommandError;
use drift_app::view::{Screen, SessionView};
use drift_core::{
    CertFingerprint, ConnectMode, ConnectStage, ConnectionProfile, DesktopSize, DisconnectReason,
    SessionState,
};
use drift_rdp::{
    CertificateRole, CursorUpdate, SessionCommand, SessionEvent, SessionEvents, SessionHandle, SessionStats,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    /// Exits (after `Disconnected{UserClosed}`) when told to `Close`.
    Graceful,
    /// Ignores `Close`; only exits once every handle is dropped.
    Stuck,
}

/// The test's end of one fake actor.
struct FakeActor {
    commands: Arc<Mutex<Vec<SessionCommand>>>,
    events: mpsc::WeakUnboundedSender<SessionEvent>,
    handle: mpsc::WeakUnboundedSender<SessionCommand>,
    kill: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl FakeActor {
    fn emit(&self, ev: SessionEvent) {
        self.events.upgrade().expect("actor still running").send(ev).unwrap();
    }
    fn commands(&self) -> Vec<SessionCommand> {
        self.commands.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct FakeHost {
    behaviour: Mutex<HashMap<String, Behaviour>>,
    actors: Mutex<HashMap<String, Vec<FakeActor>>>,
    views: Mutex<Vec<(String, SessionView)>>,
    events: Mutex<Vec<(String, SessionEvent)>>,
    pins: Mutex<Vec<(Uuid, CertFingerprint)>>,
    ended: Mutex<Vec<String>>,
    fail_spawn: Mutex<bool>,
}

impl FakeHost {
    fn set_behaviour(&self, window: &str, b: Behaviour) {
        self.behaviour.lock().unwrap().insert(window.into(), b);
    }
    fn with_actor<R>(&self, window: &str, nth: usize, f: impl FnOnce(&mut FakeActor) -> R) -> R {
        let mut actors = self.actors.lock().unwrap();
        f(&mut actors.get_mut(window).expect("window has actors")[nth])
    }
    fn actor_commands(&self, window: &str, nth: usize) -> Vec<SessionCommand> {
        self.with_actor(window, nth, |a| a.commands())
    }
    fn emit(&self, window: &str, nth: usize, ev: SessionEvent) {
        self.with_actor(window, nth, |a| a.emit(ev));
    }
    fn views_for(&self, window: &str) -> Vec<SessionView> {
        self.views.lock().unwrap().iter().filter(|(w, _)| w == window).map(|(_, v)| v.clone()).collect()
    }
    fn view_windows(&self) -> Vec<String> {
        self.views.lock().unwrap().iter().map(|(w, _)| w.clone()).collect()
    }
    fn ended(&self) -> Vec<String> {
        self.ended.lock().unwrap().clone()
    }
    /// Ends every fake actor that is still running (wedged ones).
    fn kill_all(&self) {
        for actor in self.actors.lock().unwrap().values_mut().flatten() {
            if let Some(kill) = actor.kill.take() {
                let _ = kill.send(());
            }
        }
    }
    fn all_actors_gone(&self) -> bool {
        self.actors
            .lock()
            .unwrap()
            .values()
            .flatten()
            .all(|a| a.task.is_finished() && a.handle.upgrade().is_none() && a.events.upgrade().is_none())
    }
}

impl SessionHost for FakeHost {
    fn spawn(
        &self,
        window: &str,
        _profile: &ConnectionProfile,
    ) -> Result<(SessionHandle, SessionEvents), CommandError> {
        if *self.fail_spawn.lock().unwrap() {
            return Err(CommandError::Storage { message: "spawn failed".into() });
        }
        let behaviour = self.behaviour.lock().unwrap().get(window).copied().unwrap_or(Behaviour::Graceful);
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel::<SessionEvent>();
        let (kill_tx, mut kill_rx) = oneshot::channel::<()>();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let log = commands.clone();
        let weak_events = ev_tx.downgrade();
        let task = tokio::spawn(async move {
            let _ = ev_tx
                .send(SessionEvent::State(SessionState::Connecting { leg: 1, stage: ConnectStage::Tcp }));
            loop {
                tokio::select! {
                    _ = &mut kill_rx => return,
                    cmd = cmd_rx.recv() => {
                        let Some(cmd) = cmd else {
                            if behaviour == Behaviour::Stuck {
                                // A wedged actor: keeps running until the test kills it.
                                let _ = (&mut kill_rx).await;
                            }
                            return;
                        };
                        log.lock().unwrap().push(cmd.clone());
                        if cmd == SessionCommand::Close && behaviour == Behaviour::Graceful {
                            let _ = ev_tx.send(SessionEvent::State(SessionState::Disconnected {
                                reason: DisconnectReason::UserClosed,
                            }));
                            return;
                        }
                    }
                }
            }
        });
        let actor = FakeActor {
            commands,
            events: weak_events,
            handle: cmd_tx.downgrade(),
            kill: Some(kill_tx),
            task,
        };
        self.actors.lock().unwrap().entry(window.into()).or_default().push(actor);
        Ok((SessionHandle::from_sender(cmd_tx), ev_rx))
    }

    fn view_changed(&self, window: &str, view: &SessionView) {
        self.views.lock().unwrap().push((window.into(), view.clone()));
    }

    fn session_event(&self, window: &str, event: &SessionEvent) {
        self.events.lock().unwrap().push((window.into(), event.clone()));
    }

    fn certificate_pinned(&self, profile: Uuid, fingerprint: CertFingerprint) {
        self.pins.lock().unwrap().push((profile, fingerprint));
    }

    fn session_ended(&self, window: &str) {
        self.ended.lock().unwrap().push(window.into());
    }
}

fn profile(name: &str) -> ConnectionProfile {
    let mut p = ConnectionProfile::new(name, "10.1.2.40", ConnectMode::Headless);
    p.rdp_username = "fake-user".into();
    p
}

fn setup() -> (Arc<FakeHost>, SessionManager) {
    let host = Arc::new(FakeHost::default());
    let manager = SessionManager::new(host.clone(), tokio::runtime::Handle::current());
    (host, manager)
}

/// Polls `cond` (yielding to the pumps) until it holds; panics after ~2 s of real time.
async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..2000 {
        if cond() {
            return;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("timed out waiting for: {what}");
}

fn connected() -> SessionEvent {
    SessionEvent::State(SessionState::Connected { desktop: DesktopSize::new(1280, 800), scale: 100 })
}

/// Opens a session and waits for the actor's first event, so later `emit`s cannot overtake it.
async fn open_session(host: &FakeHost, manager: &SessionManager, window: &str, profile: ConnectionProfile) {
    let before = host.views_for(window).len();
    manager.open(window, profile).unwrap();
    eventually("the first view", || host.views_for(window).len() > before).await;
}

async fn open_three(host: &FakeHost, manager: &SessionManager) {
    for (w, name) in [("w1", "Alpha"), ("w2", "Bravo"), ("w3", "Charlie")] {
        manager.open(w, profile(name)).unwrap();
    }
    eventually("three connecting views", || ["w1", "w2", "w3"].iter().all(|w| !host.views_for(w).is_empty()))
        .await;
}

#[tokio::test]
async fn open_three_close_the_middle_one() {
    let (host, manager) = setup();
    open_three(&host, &manager).await;
    assert_eq!(manager.live_sessions(), 3);
    assert_eq!(manager.windows(), ["w1", "w2", "w3"]);

    assert!(manager.close("w2").await, "a graceful actor closes within the cap");

    assert_eq!(manager.live_sessions(), 2);
    assert_eq!(manager.windows(), ["w1", "w3"]);
    assert_eq!(host.ended(), ["w2"]);
    assert_eq!(host.actor_commands("w2", 0), [SessionCommand::Close]);
    assert!(host.actor_commands("w1", 0).is_empty());
    assert!(host.actor_commands("w3", 0).is_empty());
    assert!(manager.view("w2").is_none());
    assert_eq!(manager.view("w1").unwrap().profile_name, "Alpha");
    assert_eq!(manager.view("w3").unwrap().screen, Screen::Connecting);
}

#[tokio::test]
async fn events_reach_only_their_own_window() {
    let (host, manager) = setup();
    open_three(&host, &manager).await;
    host.views.lock().unwrap().clear();

    host.emit("w2", 0, connected());
    host.emit("w2", 0, SessionEvent::Cursor(CursorUpdate::Hidden));
    eventually("w2 live", || host.views_for("w2").iter().any(|v| v.screen == Screen::Live)).await;
    eventually("cursor", || !host.events.lock().unwrap().is_empty()).await;

    assert_eq!(host.view_windows(), ["w2"], "only w2's view changed");
    let events = host.events.lock().unwrap().clone();
    assert_eq!(events, [("w2".to_owned(), SessionEvent::Cursor(CursorUpdate::Hidden))]);
    assert_eq!(manager.view("w2").unwrap().screen, Screen::Live);
    assert_eq!(manager.view("w1").unwrap().screen, Screen::Connecting);
    assert_eq!(manager.view("w3").unwrap().screen, Screen::Connecting);
}

#[tokio::test]
async fn unchanged_views_are_not_re_emitted() {
    let (host, manager) = setup();
    open_session(&host, &manager, "w1", profile("Alpha")).await;
    host.emit("w1", 0, connected());
    host.emit("w1", 0, SessionEvent::Capabilities(Default::default()));
    eventually("live", || host.views_for("w1").len() == 2).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(host.views_for("w1").len(), 2, "capabilities do not change the view");
}

#[tokio::test]
async fn statistics_reach_the_window_and_the_hud_can_be_toggled() {
    // Plan M1 "Done (manual M1)": the fps number must reach the window that shows the picture.
    let (host, manager) = setup();
    open_session(&host, &manager, "w1", profile("Alpha")).await;
    open_session(&host, &manager, "w2", profile("Bravo")).await;
    host.emit("w1", 0, connected());
    let sample = SessionStats { fps: 58.9, bitrate_bps: 1_200_000, ..Default::default() };
    host.emit("w1", 0, SessionEvent::Stats(sample));
    eventually("a view carrying the sample", || {
        host.views_for("w1").last().is_some_and(|v| v.stats.is_some())
    })
    .await;
    let v = host.views_for("w1").pop().unwrap();
    assert_eq!(v.stats.unwrap().fps_tenths, 589);
    assert!(!v.show_stats, "the HUD is off until the user asks for it");
    assert!(host.views_for("w2").iter().all(|v| v.stats.is_none()), "only its own window");

    manager.toggle_stats("w1").unwrap();
    assert!(host.views_for("w1").pop().unwrap().show_stats, "Session \u{25b8} Show Statistics");
    manager.toggle_stats("w1").unwrap();
    assert!(!host.views_for("w1").pop().unwrap().show_stats);
    assert!(matches!(manager.toggle_stats("nope"), Err(CommandError::NoSession)));
}

#[tokio::test]
async fn certificate_answers_are_forwarded_and_pins_persisted() {
    let (host, manager) = setup();
    let p = profile("Alpha");
    let id = p.id;
    open_session(&host, &manager, "w1", p).await;
    let fp = CertFingerprint::from_bytes([0xf3; 32]);
    host.emit(
        "w1",
        0,
        SessionEvent::CertificatePrompt {
            host: "10.1.2.40".into(),
            port: 3392,
            fingerprint: fp,
            role: CertificateRole::Server,
        },
    );
    eventually("prompt", || manager.view("w1").is_some_and(|v| v.screen == Screen::Certificate)).await;

    manager.accept_certificate("w1", fp, true).unwrap();
    eventually("accepted", || !host.actor_commands("w1", 0).is_empty()).await;
    assert_eq!(
        host.actor_commands("w1", 0),
        [SessionCommand::AcceptCertificate { fingerprint: fp, pin: true }]
    );
    let last = host.views_for("w1").pop().unwrap();
    assert_eq!(last.screen, Screen::Connecting, "the prompt is dismissed immediately");
    assert!(last.certificate.is_none());

    host.emit("w1", 0, SessionEvent::CertificatePinned(fp));
    eventually("pinned", || !host.pins.lock().unwrap().is_empty()).await;
    assert_eq!(*host.pins.lock().unwrap(), [(id, fp)]);
}

#[tokio::test]
async fn reject_certificate_is_forwarded() {
    let (host, manager) = setup();
    manager.open("w1", profile("Alpha")).unwrap();
    manager.reject_certificate("w1").unwrap();
    eventually("rejected", || !host.actor_commands("w1", 0).is_empty()).await;
    assert_eq!(host.actor_commands("w1", 0), [SessionCommand::RejectCertificate]);
}

#[tokio::test]
async fn intents_are_forwarded_to_the_right_actor() {
    let (host, manager) = setup();
    open_three(&host, &manager).await;
    assert_eq!(manager.reconnect_now("w3").unwrap(), Reconnect::Sent);
    manager.send("w1", SessionCommand::SetVisible(false)).unwrap();
    manager.send("w3", SessionCommand::Cancel).unwrap();
    eventually("delivered", || host.actor_commands("w3", 0).len() == 2).await;
    eventually("delivered", || host.actor_commands("w1", 0).len() == 1).await;
    assert_eq!(host.actor_commands("w3", 0), [SessionCommand::ReconnectNow, SessionCommand::Cancel]);
    assert_eq!(host.actor_commands("w1", 0), [SessionCommand::SetVisible(false)]);
    assert!(host.actor_commands("w2", 0).is_empty());
}

#[tokio::test]
async fn intents_without_a_session_fail() {
    let (_host, manager) = setup();
    assert_eq!(manager.send("nope", SessionCommand::ReconnectNow), Err(CommandError::NoSession));
    assert_eq!(manager.reconnect_now("nope"), Err(CommandError::NoSession));
    assert_eq!(manager.reject_certificate("nope"), Err(CommandError::NoSession));
    assert!(manager.close("nope").await, "closing an unknown window is a no-op");
}

#[tokio::test]
async fn spawn_failure_leaves_no_session() {
    let (host, manager) = setup();
    *host.fail_spawn.lock().unwrap() = true;
    assert!(matches!(manager.open("w1", profile("Alpha")), Err(CommandError::Storage { .. })));
    assert_eq!(manager.live_sessions(), 0);
    assert!(manager.windows().is_empty());
}

#[tokio::test]
async fn an_ended_session_keeps_its_window_and_can_be_reopened() {
    let (host, manager) = setup();
    let p = profile("Alpha");
    let id = p.id;
    open_session(&host, &manager, "w1", p.clone()).await;
    host.emit("w1", 0, SessionEvent::State(SessionState::Failed { reason: DisconnectReason::AuthFailed }));
    host.with_actor("w1", 0, |a| a.kill.take().unwrap().send(()).unwrap());

    eventually("ended", || host.ended() == ["w1"]).await;
    assert_eq!(manager.live_sessions(), 0);
    assert_eq!(manager.windows(), ["w1"]);
    assert_eq!(manager.view("w1").unwrap().screen, Screen::Error);
    assert_eq!(manager.profile_id("w1"), Some(id));
    assert_eq!(manager.reconnect_now("w1"), Ok(Reconnect::Reopen(id)));
    assert_eq!(manager.send("w1", SessionCommand::ReconnectNow), Err(CommandError::NoSession));

    manager.open("w1", p).unwrap();
    eventually("reconnecting view", || manager.view("w1").is_some_and(|v| v.screen == Screen::Connecting))
        .await;
    assert_eq!(manager.live_sessions(), 1);
    assert_eq!(host.ended(), ["w1"], "session_ended once per session");
}

#[tokio::test]
async fn open_replaces_a_live_session() {
    let (host, manager) = setup();
    manager.open("w1", profile("Alpha")).unwrap();
    manager.open("w1", profile("Bravo")).unwrap();
    eventually("old closed", || host.actor_commands("w1", 0) == [SessionCommand::Close]).await;
    eventually("old ended", || host.ended() == ["w1"]).await;
    assert_eq!(manager.live_sessions(), 1);
    assert_eq!(manager.view("w1").unwrap().profile_name, "Bravo");
    // Late events of the replaced actor never reach the window.
    let n = host.views_for("w1").len();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(host.views_for("w1").len(), n);
    assert!(
        host.views_for("w1").iter().all(|v| v.screen != Screen::Error),
        "no UserClosed from the old actor"
    );
}

/// UI-windows decision 3: connecting a profile that already has a session window (connecting,
/// live, reconnecting, or failed and not dismissed) focuses that window; otherwise a new window
/// is opened.
#[tokio::test]
async fn connecting_an_open_profile_focuses_its_window() {
    let (host, manager) = setup();
    let alpha = profile("Alpha");
    let bravo = profile("Bravo");
    assert_eq!(manager.connect_plan(alpha.id), ConnectPlan::Open, "nothing open yet");

    open_session(&host, &manager, "session-0", alpha.clone()).await;
    assert_eq!(manager.window_for(alpha.id).as_deref(), Some("session-0"));
    assert_eq!(manager.connect_plan(alpha.id), ConnectPlan::Focus("session-0".into()), "connecting");
    assert_eq!(manager.connect_plan(bravo.id), ConnectPlan::Open);

    host.emit("session-0", 0, connected());
    eventually("live", || manager.view("session-0").is_some_and(|v| v.screen == Screen::Live)).await;
    assert_eq!(manager.connect_plan(alpha.id), ConnectPlan::Focus("session-0".into()), "live");

    // A failed window stays until the user dismisses it: still focused, not duplicated.
    host.emit(
        "session-0",
        0,
        SessionEvent::State(SessionState::Failed { reason: DisconnectReason::AuthFailed }),
    );
    host.with_actor("session-0", 0, |a| a.kill.take().unwrap().send(()).unwrap());
    eventually("ended", || host.ended() == ["session-0"]).await;
    assert_eq!(manager.connect_plan(alpha.id), ConnectPlan::Focus("session-0".into()), "failed");

    // Closing the window (dismissing it) frees the profile.
    assert!(manager.close("session-0").await);
    assert_eq!(manager.window_for(alpha.id), None);
    assert_eq!(manager.connect_plan(alpha.id), ConnectPlan::Open);
}

/// UI-windows decision 9 / 16: the Window menu, the Dock menu and the gallery list session
/// windows in the order they were opened; reconnecting in a window keeps its place.
#[tokio::test]
async fn sessions_are_listed_in_opening_order() {
    let (host, manager) = setup();
    let alpha = profile("Alpha");
    open_session(&host, &manager, "session-7", alpha.clone()).await;
    open_session(&host, &manager, "session-10", profile("Bravo")).await;
    open_session(&host, &manager, "session-2", profile("Charlie")).await;
    let order = |m: &SessionManager| m.sessions().into_iter().map(|s| s.window).collect::<Vec<_>>();
    assert_eq!(order(&manager), ["session-7", "session-10", "session-2"]);

    manager.open("session-7", alpha.clone()).unwrap(); // Reconnect in place
    assert_eq!(order(&manager), ["session-7", "session-10", "session-2"]);
    let first = &manager.sessions()[0];
    assert_eq!((first.profile.id, first.view.profile_name.as_str()), (alpha.id, "Alpha"));
    assert!(manager.close("session-10").await);
    assert_eq!(order(&manager), ["session-7", "session-2"]);
}

/// UI-windows IPC contract: `connections()` is the gallery's "Open" section.
#[tokio::test]
async fn connections_list_open_windows_with_their_status() {
    let (host, manager) = setup();
    let alpha = profile("Alpha");
    let bravo = profile("Bravo");
    open_session(&host, &manager, "session-0", alpha.clone()).await;
    open_session(&host, &manager, "session-1", bravo.clone()).await;
    host.emit("session-1", 0, connected());
    eventually("live", || manager.view("session-1").is_some_and(|v| v.screen == Screen::Live)).await;

    let open = manager.connections().open;
    assert_eq!(open.len(), 2);
    assert_eq!((open[0].profile_id, open[0].window.as_str()), (alpha.id, "session-0"));
    assert_eq!(open[0].status, ConnectionStatus::Connecting);
    assert_eq!(open[0].live_secs, None);
    assert_eq!((open[1].profile_id, open[1].status), (bravo.id, ConnectionStatus::Live));
    assert!(open[1].live_secs.is_some(), "uptime while live");

    host.emit(
        "session-1",
        0,
        SessionEvent::State(SessionState::Reconnecting {
            attempt: 2,
            next_in: Duration::from_secs(4),
            reason: DisconnectReason::Network,
        }),
    );
    eventually("reconnecting", || {
        manager.view("session-1").is_some_and(|v| v.screen == Screen::Reconnecting)
    })
    .await;
    let open = manager.connections().open;
    assert_eq!(open[1].status, ConnectionStatus::Reconnecting);
    assert_eq!((open[1].reconnect_in_secs, open[1].attempt, open[1].live_secs), (Some(4), Some(2), None));
    assert!(manager.close("session-0").await);
    assert_eq!(manager.connections().open.iter().map(|o| o.profile_id).collect::<Vec<_>>(), [bravo.id]);
}

#[tokio::test(start_paused = true)]
async fn closing_a_stuck_actor_is_capped() {
    let (host, manager) = setup();
    host.set_behaviour("w1", Behaviour::Stuck);
    manager.open("w1", profile("Alpha")).unwrap();
    let t0 = tokio::time::Instant::now();
    assert!(!manager.close("w1").await, "the stuck actor is abandoned");
    let took = t0.elapsed();
    assert!(took >= CLOSE_CAP && took <= CLOSE_CAP + Duration::from_millis(100), "{took:?}");
    assert_eq!(host.ended(), ["w1"]);
    assert_eq!(manager.live_sessions(), 0);
}

#[tokio::test(start_paused = true)]
async fn quit_completes_within_the_cap() {
    let (host, manager) = setup();
    host.set_behaviour("w2", Behaviour::Stuck);
    open_three(&host, &manager).await;
    let t0 = tokio::time::Instant::now();
    let report = manager.shutdown().await;
    let took = t0.elapsed();
    assert!(took <= CLOSE_CAP + Duration::from_millis(100), "quit took {took:?}");
    assert_eq!(report, ShutdownReport { graceful: 2, abandoned: 1 });
    assert_eq!(manager.live_sessions(), 0);
    assert!(manager.windows().is_empty());
    let mut ended = host.ended();
    ended.sort();
    assert_eq!(ended, ["w1", "w2", "w3"]);
    for w in ["w1", "w2", "w3"] {
        assert_eq!(host.actor_commands(w, 0), [SessionCommand::Close]);
    }
}

#[tokio::test]
async fn quit_without_stuck_actors_is_fast_and_graceful() {
    let (host, manager) = setup();
    open_three(&host, &manager).await;
    let t0 = std::time::Instant::now();
    let report = manager.shutdown().await;
    assert!(t0.elapsed() < Duration::from_millis(500));
    assert_eq!(report, ShutdownReport { graceful: 3, abandoned: 0 });
}

#[tokio::test(start_paused = true)]
async fn no_handles_leak_after_close_and_quit() {
    let (host, manager) = setup();
    host.set_behaviour("w3", Behaviour::Stuck);
    open_three(&host, &manager).await;
    manager.open("w1", profile("Alpha again")).unwrap(); // replaces w1's first actor
    assert!(manager.close("w2").await);
    let _ = manager.shutdown().await;
    host.kill_all(); // the wedged actor would run forever; nothing of Drift's is left in it
    eventually("all fake actors exited and every sender dropped", || host.all_actors_gone()).await;
    assert_eq!(manager.live_sessions(), 0);
    let mut ended = host.ended();
    ended.sort();
    assert_eq!(ended, ["w1", "w1", "w2", "w3"], "session_ended exactly once per session");
}
