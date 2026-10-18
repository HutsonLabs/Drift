//! M5-3 / M5-2 / M7-2 Red (app side): the two app-lifetime platform services.
//!
//! Both services are implemented and unit-tested in their own crates, but nothing in the
//! shipped app ever started them. These tests drive `drift_app::services` with fake ports and
//! a fake session fanout, so the production path — main-thread poll timer → watcher → every
//! live session, and `TriggerFeed` → `NetworkReachable`/`ReconnectNow` — is covered without
//! AppKit or a real actor.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use drift_app::manager::{SessionHost, SessionManager};
use drift_app::profiles::CommandError;
use drift_app::services::{
    ClipboardPump, LiveSession, PollTimer, SessionFanout, Tick, most_permissive, trigger_commands,
};
use drift_app::view::SessionView;
use drift_clipboard::poll::{POLL_INTERVAL, PasteboardPort};
use drift_clipboard::{ClipboardContents, ClipboardItem};
use drift_core::{
    CertFingerprint, ClipboardPrefs, Clock, ConnectMode, ConnectionProfile, DisconnectReason, SessionState,
    Trigger, TriggerAction,
};
use drift_macos::TriggerFeed;
use drift_rdp::{SessionCommand, SessionEvent, SessionEvents, SessionHandle};
use tokio::sync::mpsc;
use uuid::Uuid;

// ---- fakes ---------------------------------------------------------------------------------

/// A pasteboard whose `changeCount` only moves when the test says so.
#[derive(Default)]
struct FakePasteboard {
    state: Mutex<(i64, ClipboardContents)>,
    reads: Mutex<Vec<ClipboardPrefs>>,
}

impl FakePasteboard {
    fn user_copies(&self, contents: ClipboardContents) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 += 1;
        state.1 = contents;
    }

    fn reads(&self) -> Vec<ClipboardPrefs> {
        self.reads.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl PasteboardPort for FakePasteboard {
    fn change_count(&self) -> i64 {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).0
    }

    fn read(&self, level: ClipboardPrefs) -> ClipboardContents {
        self.reads.lock().unwrap_or_else(PoisonError::into_inner).push(level);
        let mut contents = self.state.lock().unwrap_or_else(PoisonError::into_inner).1.clone();
        if level != ClipboardPrefs::TextAndImages {
            contents.items.retain(|i| matches!(i, ClipboardItem::Text(_)));
        }
        contents
    }

    fn write(&self, contents: &ClipboardContents) -> i64 {
        self.user_copies(contents.clone());
        self.change_count()
    }
}

/// Stands in for the `SessionManager`: a list of live sessions and a command log.
#[derive(Default)]
struct FakeSessions {
    live: Mutex<Vec<LiveSession>>,
    sent: Mutex<Vec<(String, SessionCommand)>>,
}

impl FakeSessions {
    fn with(sessions: &[(&str, ClipboardPrefs)]) -> Arc<Self> {
        let me = Arc::new(Self::default());
        me.set(sessions);
        me
    }

    fn with_live(sessions: &[LiveSession]) -> Arc<Self> {
        let me = Arc::new(Self::default());
        me.replace(sessions);
        me
    }

    fn replace(&self, sessions: &[LiveSession]) {
        *self.live.lock().unwrap_or_else(PoisonError::into_inner) = sessions.to_vec();
    }

    fn set(&self, sessions: &[(&str, ClipboardPrefs)]) {
        *self.live.lock().unwrap_or_else(PoisonError::into_inner) =
            sessions.iter().map(|(window, clipboard)| LiveSession::new(*window, *clipboard)).collect();
    }

    fn take(&self) -> Vec<(String, SessionCommand)> {
        std::mem::take(&mut *self.sent.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl SessionFanout for FakeSessions {
    fn live_sessions(&self) -> Vec<LiveSession> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn send(&self, window: &str, cmd: SessionCommand) -> bool {
        let live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        if !live.iter().any(|s| s.window == window) {
            return false;
        }
        drop(live);
        self.sent.lock().unwrap_or_else(PoisonError::into_inner).push((window.to_owned(), cmd));
        true
    }
}

/// A clock the test moves by hand (the debounce window in `TriggerMerger` is 2 s).
#[derive(Debug, Clone)]
struct ManualClock(Arc<Mutex<Instant>>);

impl ManualClock {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Instant::now())))
    }
    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn text(s: &str) -> ClipboardContents {
    ClipboardContents { items: vec![ClipboardItem::Text(s.into())] }
}

fn text_and_png(s: &str) -> ClipboardContents {
    ClipboardContents {
        items: vec![ClipboardItem::Text(s.into()), ClipboardItem::Png(vec![0x89, b'P', b'N', b'G'])],
    }
}

fn local(contents: &ClipboardContents) -> SessionCommand {
    SessionCommand::ClipboardLocalChanged(contents.clone())
}

// ---- M5-3: the poll timer ------------------------------------------------------------------

#[test]
fn poll_timer_ticks_until_it_is_dropped() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let counter = ticks.clone();
    let timer = PollTimer::start(Duration::from_millis(20), move || {
        counter.fetch_add(1, Ordering::SeqCst);
        Tick::Continue
    });
    std::thread::sleep(Duration::from_millis(250));
    let while_running = ticks.load(Ordering::SeqCst);
    assert!(while_running >= 3, "expected several ticks in 250 ms, got {while_running}");
    drop(timer);
    std::thread::sleep(Duration::from_millis(60));
    let after_drop = ticks.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(ticks.load(Ordering::SeqCst), after_drop, "the timer thread outlived its PollTimer");
}

#[test]
fn poll_timer_stops_when_a_tick_asks_it_to() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let counter = ticks.clone();
    let _timer = PollTimer::start(Duration::from_millis(10), move || {
        if counter.fetch_add(1, Ordering::SeqCst) >= 1 { Tick::Stop } else { Tick::Continue }
    });
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(ticks.load(Ordering::SeqCst), 2, "the timer ignored Tick::Stop");
}

#[test]
fn the_app_polls_at_the_clipboard_poll_interval() {
    assert_eq!(drift_app::services::CLIPBOARD_POLL_INTERVAL, POLL_INTERVAL);
    assert_eq!(POLL_INTERVAL, Duration::from_millis(250));
}

// ---- M5-3/M5-2: local → remote fanout -------------------------------------------------------

#[test]
fn most_permissive_level_wins_across_live_sessions() {
    use ClipboardPrefs::{Off, Text, TextAndImages};
    assert_eq!(most_permissive(std::iter::empty()), Off);
    assert_eq!(most_permissive([Off, Off]), Off);
    assert_eq!(most_permissive([Off, Text]), Text);
    assert_eq!(most_permissive([Text, Off]), Text);
    assert_eq!(most_permissive([Text, TextAndImages, Off]), TextAndImages);
    assert_eq!(most_permissive([TextAndImages]), TextAndImages);
}

#[test]
fn a_local_copy_reaches_every_live_session() {
    let sessions = FakeSessions::with(&[
        ("session-0", ClipboardPrefs::TextAndImages),
        ("session-1", ClipboardPrefs::Text),
        ("session-2", ClipboardPrefs::Off),
    ]);
    let mut pump = ClipboardPump::new(FakePasteboard::default());
    // Nothing on the pasteboard: the seeding tick reads it once and has nothing to advertise.
    assert_eq!(pump.tick(&*sessions), 0);
    assert_eq!(sessions.take(), vec![]);
    assert_eq!(pump.port().reads(), vec![ClipboardPrefs::TextAndImages], "one seeding read");

    pump.port().user_copies(text_and_png("copy-me"));
    assert_eq!(pump.tick(&*sessions), 2, "both syncing sessions must be told");
    assert_eq!(
        sessions.take(),
        vec![
            ("session-0".to_owned(), local(&text_and_png("copy-me"))),
            ("session-1".to_owned(), local(&text_and_png("copy-me"))),
        ],
        "sessions whose clipboard is Off get nothing; the rest filter per-session"
    );
    // The pasteboard is read once per change, at the most permissive level.
    assert_eq!(pump.port().reads(), vec![ClipboardPrefs::TextAndImages; 2]);

    // No further change and nothing new to seed: no read, no command.
    assert_eq!(pump.tick(&*sessions), 0);
    assert_eq!(sessions.take(), vec![]);
    assert_eq!(pump.port().reads().len(), 2);
}

#[test]
fn images_are_not_read_when_no_live_session_wants_them() {
    let sessions = FakeSessions::with(&[("session-0", ClipboardPrefs::Text)]);
    let mut pump = ClipboardPump::new(FakePasteboard::default());
    assert_eq!(pump.tick(&*sessions), 0);
    pump.port().user_copies(text_and_png("text only"));
    assert_eq!(pump.tick(&*sessions), 1);
    assert_eq!(pump.port().reads(), vec![ClipboardPrefs::Text; 2], "never at TextAndImages");
    assert_eq!(sessions.take(), vec![("session-0".to_owned(), local(&text("text only")))]);
}

#[test]
fn a_new_session_is_seeded_with_the_current_pasteboard() {
    let sessions = FakeSessions::with(&[]);
    let mut pump = ClipboardPump::new(FakePasteboard::default());
    // A copy made while no session is live is consumed without a read …
    pump.port().user_copies(text("copied-before-connecting"));
    assert_eq!(pump.tick(&*sessions), 0);
    assert_eq!(pump.port().reads(), vec![], "nothing to sync: no read");

    // … but the session that opens afterwards still starts with it.
    sessions.set(&[("session-0", ClipboardPrefs::TextAndImages)]);
    assert_eq!(pump.tick(&*sessions), 1);
    assert_eq!(sessions.take(), vec![("session-0".to_owned(), local(&text("copied-before-connecting")))]);
    // Seeding happens once per session.
    assert_eq!(pump.tick(&*sessions), 0);
    assert_eq!(sessions.take(), vec![]);
}

#[test]
fn a_session_that_opens_later_is_seeded_but_the_others_are_not_told_again() {
    let sessions = FakeSessions::with(&[("session-0", ClipboardPrefs::TextAndImages)]);
    let mut pump = ClipboardPump::new(FakePasteboard::default());
    pump.port().user_copies(text("shared"));
    assert_eq!(pump.tick(&*sessions), 1);
    assert_eq!(sessions.take(), vec![("session-0".to_owned(), local(&text("shared")))]);

    sessions
        .set(&[("session-0", ClipboardPrefs::TextAndImages), ("session-1", ClipboardPrefs::TextAndImages)]);
    assert_eq!(pump.tick(&*sessions), 1);
    assert_eq!(sessions.take(), vec![("session-1".to_owned(), local(&text("shared")))]);
}

#[test]
fn a_session_replaced_in_the_same_window_is_seeded_again() {
    // "Disconnect" then "Connect" in the same tab within one 250 ms tick: the window never
    // leaves the fanout, but the actor behind it is new and knows nothing about the clipboard.
    let sessions = FakeSessions::with_live(&[LiveSession::new("tab-0", ClipboardPrefs::Text).generation(1)]);
    let mut pump = ClipboardPump::new(FakePasteboard::default());
    pump.port().user_copies(text("one"));
    assert_eq!(pump.tick(&*sessions), 1);
    assert_eq!(sessions.take(), vec![("tab-0".to_owned(), local(&text("one")))]);

    sessions.replace(&[LiveSession::new("tab-0", ClipboardPrefs::Text).generation(2)]);
    assert_eq!(pump.tick(&*sessions), 1, "the replacement actor must be seeded too");
    assert_eq!(sessions.take(), vec![("tab-0".to_owned(), local(&text("one")))]);
    assert_eq!(pump.tick(&*sessions), 0, "…but only once");
}

#[test]
fn a_session_that_ends_and_returns_is_seeded_again() {
    let sessions = FakeSessions::with(&[("session-0", ClipboardPrefs::Text)]);
    let mut pump = ClipboardPump::new(FakePasteboard::default());
    pump.port().user_copies(text("one"));
    assert_eq!(pump.tick(&*sessions), 1);
    let _ = sessions.take();

    sessions.set(&[]);
    assert_eq!(pump.tick(&*sessions), 0);
    // The same window reconnects: its fresh actor knows nothing, so it is seeded again.
    sessions.set(&[("session-0", ClipboardPrefs::Text)]);
    assert_eq!(pump.tick(&*sessions), 1);
    assert_eq!(sessions.take(), vec![("session-0".to_owned(), local(&text("one")))]);
}

// ---- M7-2: reconnect triggers ---------------------------------------------------------------

#[test]
fn trigger_actions_map_to_session_commands() {
    assert_eq!(
        trigger_commands(TriggerAction::PauseReconnect, false),
        vec![SessionCommand::NetworkReachable(false)]
    );
    assert_eq!(
        trigger_commands(TriggerAction::PauseReconnect, true),
        vec![SessionCommand::NetworkReachable(false)]
    );
    // A session that is waiting out a backoff is also told to skip it.
    assert_eq!(
        trigger_commands(TriggerAction::RetryNow, true),
        vec![SessionCommand::NetworkReachable(true), SessionCommand::ReconnectNow],
        "reachability must be restored before the actor is asked to skip its backoff"
    );
    // Any other session only learns that the network is back: `ReconnectNow` would restart a
    // session the user cancelled or one that ended in `Failed` (drift_rdp::actor::idle).
    assert_eq!(
        trigger_commands(TriggerAction::RetryNow, false),
        vec![SessionCommand::NetworkReachable(true)]
    );
}

#[test]
fn network_and_wake_triggers_reach_every_session() {
    let sessions = FakeSessions::with_live(&[
        LiveSession::new("session-0", ClipboardPrefs::TextAndImages).reconnecting(true),
        LiveSession::new("session-1", ClipboardPrefs::Off),
    ]);
    let clock = ManualClock::new();
    let fanout: Arc<dyn SessionFanout> = sessions.clone();
    let feed = TriggerFeed::new(true, Arc::new(clock.clone()), drift_app::services::trigger_sink(fanout));

    // Wi-Fi off: every session pauses its backoff (clipboard prefs are irrelevant here).
    feed.trigger(Trigger::NetworkOffline);
    assert_eq!(
        sessions.take(),
        vec![
            ("session-0".to_owned(), SessionCommand::NetworkReachable(false)),
            ("session-1".to_owned(), SessionCommand::NetworkReachable(false)),
        ]
    );
    // A duplicate path update changes nothing.
    feed.trigger(Trigger::NetworkOffline);
    assert_eq!(sessions.take(), vec![]);

    // Wi-Fi on: the reconnecting session skips its backoff instead of waiting it out.
    feed.trigger(Trigger::NetworkOnline);
    assert_eq!(
        sessions.take(),
        vec![
            ("session-0".to_owned(), SessionCommand::NetworkReachable(true)),
            ("session-0".to_owned(), SessionCommand::ReconnectNow),
            ("session-1".to_owned(), SessionCommand::NetworkReachable(true)),
        ]
    );
    // The wake that follows a wake-up reconnect is debounced.
    clock.advance(Duration::from_millis(500));
    feed.trigger(Trigger::Wake);
    assert_eq!(sessions.take(), vec![]);

    // A later wake retries again.
    clock.advance(Duration::from_secs(3));
    feed.trigger(Trigger::Wake);
    assert_eq!(
        sessions.take(),
        vec![
            ("session-0".to_owned(), SessionCommand::NetworkReachable(true)),
            ("session-0".to_owned(), SessionCommand::ReconnectNow),
            ("session-1".to_owned(), SessionCommand::NetworkReachable(true)),
        ]
    );
}

#[test]
fn a_cancelled_session_is_never_resurrected_by_a_wake() {
    // "Cancel" on the reconnect overlay leaves the actor in `idle`, where `ReconnectNow` would
    // reconnect it (drift_rdp::actor). The trigger service must not undo the user's decision.
    let sessions = FakeSessions::with(&[("session-0", ClipboardPrefs::Text)]);
    let clock = ManualClock::new();
    let fanout: Arc<dyn SessionFanout> = sessions.clone();
    let feed = TriggerFeed::new(false, Arc::new(clock), drift_app::services::trigger_sink(fanout));
    feed.trigger(Trigger::NetworkOnline);
    assert_eq!(sessions.take(), vec![("session-0".to_owned(), SessionCommand::NetworkReachable(true))]);
}

// ---- the real SessionManager is the production fanout ---------------------------------------

/// The minimum `SessionHost` needed to open live sessions: one channel-backed fake actor per
/// window that records what it is told.
#[derive(Default)]
struct FakeHost {
    actors: Mutex<HashMap<String, FakeActor>>,
}

struct FakeActor {
    commands: Arc<Mutex<Vec<SessionCommand>>>,
    /// Weak, so the host never keeps an ended actor's event stream open (the manager waits
    /// for it to close).
    events: mpsc::WeakUnboundedSender<SessionEvent>,
}

impl FakeHost {
    fn commands(&self, window: &str) -> Vec<SessionCommand> {
        let actors = self.actors.lock().unwrap_or_else(PoisonError::into_inner);
        actors
            .get(window)
            .map(|a| a.commands.lock().unwrap_or_else(PoisonError::into_inner).clone())
            .unwrap_or_default()
    }

    fn emit(&self, window: &str, event: SessionEvent) {
        let actors = self.actors.lock().unwrap_or_else(PoisonError::into_inner);
        let events = actors.get(window).expect("window has an actor").events.upgrade();
        events.expect("actor still running").send(event).unwrap();
    }
}

impl SessionHost for FakeHost {
    fn spawn(
        &self,
        window: &str,
        _profile: &ConnectionProfile,
    ) -> Result<(SessionHandle, SessionEvents), CommandError> {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SessionCommand>();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel::<SessionEvent>();
        let log = Arc::new(Mutex::new(Vec::new()));
        self.actors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(window.into(), FakeActor { commands: log.clone(), events: ev_tx.downgrade() });
        tokio::spawn(async move {
            let _keep_alive = ev_tx;
            while let Some(cmd) = cmd_rx.recv().await {
                let close = cmd == SessionCommand::Close;
                log.lock().unwrap_or_else(PoisonError::into_inner).push(cmd);
                if close {
                    return;
                }
            }
        });
        Ok((SessionHandle::from_sender(cmd_tx), ev_rx))
    }

    fn view_changed(&self, _window: &str, _view: &SessionView) {}
    fn session_event(&self, _window: &str, _event: &SessionEvent) {}
    fn certificate_pinned(&self, _profile: Uuid, _fingerprint: CertFingerprint) {}
    fn session_ended(&self, _window: &str) {}
}

fn profile(name: &str, clipboard: ClipboardPrefs) -> ConnectionProfile {
    let mut p = ConnectionProfile::new(name, "10.1.2.40", ConnectMode::Headless);
    p.rdp_username = "fake-user".into();
    p.clipboard = clipboard;
    p
}

#[tokio::test]
async fn the_session_manager_reports_and_reaches_live_sessions() {
    let host = Arc::new(FakeHost::default());
    let manager = SessionManager::new(host.clone(), tokio::runtime::Handle::current());
    manager.open("tab-0", profile("headless", ClipboardPrefs::Text)).unwrap();
    manager.open("tab-1", profile("images", ClipboardPrefs::TextAndImages)).unwrap();

    assert_eq!(
        fanout_of(&manager),
        vec![
            ("tab-0".to_owned(), ClipboardPrefs::Text, false),
            ("tab-1".to_owned(), ClipboardPrefs::TextAndImages, false),
        ],
        "the fanout must see each window's per-profile clipboard preference"
    );

    // A session waiting out a backoff is flagged, so the trigger service may skip its delay.
    host.emit(
        "tab-0",
        SessionEvent::State(SessionState::Reconnecting {
            attempt: 3,
            next_in: Duration::from_secs(8),
            reason: DisconnectReason::Network,
        }),
    );
    eventually(|| {
        SessionFanout::live_sessions(&manager).iter().any(|s| s.window == "tab-0" && s.reconnecting)
    })
    .await;
    assert!(
        !SessionFanout::live_sessions(&manager).iter().any(|s| s.window == "tab-1" && s.reconnecting),
        "a connecting session is not waiting out a backoff"
    );

    assert!(SessionFanout::send(&manager, "tab-0", local(&text("hi"))));
    assert!(!SessionFanout::send(&manager, "tab-9", local(&text("hi"))), "an unknown window is not live");

    // Closing a tab removes it from the fanout.
    assert!(manager.close("tab-1").await);
    assert_eq!(fanout_of(&manager), vec![("tab-0".to_owned(), ClipboardPrefs::Text, true)]);
    assert!(!SessionFanout::send(&manager, "tab-1", local(&text("gone"))));

    eventually(|| host.commands("tab-0").contains(&local(&text("hi")))).await;
    assert_eq!(host.commands("tab-0"), vec![local(&text("hi"))]);

    // Reconnecting the same window gives a new actor, and a new generation with it.
    let before = generation_of(&manager, "tab-0");
    manager.open("tab-0", profile("headless", ClipboardPrefs::Text)).unwrap();
    assert_ne!(generation_of(&manager, "tab-0"), before, "a replaced actor is a new session");
}

/// `(window, clipboard preference, reconnecting)` for every live session, sorted by window.
fn fanout_of(manager: &SessionManager) -> Vec<(String, ClipboardPrefs, bool)> {
    let mut live: Vec<_> = SessionFanout::live_sessions(manager)
        .into_iter()
        .map(|s| (s.window, s.clipboard, s.reconnecting))
        .collect();
    live.sort_by(|a, b| a.0.cmp(&b.0));
    live
}

fn generation_of(manager: &SessionManager, window: &str) -> u64 {
    SessionFanout::live_sessions(manager)
        .into_iter()
        .find(|s| s.window == window)
        .expect("window is live")
        .generation
}

/// Polls `cond` (yielding to the manager's pumps) until it holds; panics after ~2 s.
async fn eventually(mut cond: impl FnMut() -> bool) {
    for _ in 0..2000 {
        if cond() {
            return;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("condition never became true");
}
