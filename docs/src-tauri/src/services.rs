//! App-lifetime platform services (tasks **M5-3**, **M5-2** and **M7-2**, app side).
//!
//! Two OS observers must run for as long as Drift does, independently of any one session:
//!
//! | Service | Task | What it does |
//! |---|---|---|
//! | [`ClipboardPump`] on a [`PollTimer`] | M5-3 / M5-2 | polls `NSPasteboard.changeCount` every [`CLIPBOARD_POLL_INTERVAL`] on the main thread and hands each local copy to every live session as [`SessionCommand::ClipboardLocalChanged`] |
//! | `ReconnectTriggers` | M7-2 | turns `NWPathMonitor` and `NSWorkspace.didWakeNotification` into [`SessionCommand::NetworkReachable`] and [`SessionCommand::ReconnectNow`] for every live session |
//!
//! [`PlatformServices::start`] owns both for the app's lifetime; dropping it stops them.
//!
//! Everything that decides anything lives behind two small ports — [`PasteboardPort`] (the
//! pasteboard) and [`SessionFanout`] (the sessions) — so the whole service is unit tested with
//! fakes in `tests/platform_services.rs`, without AppKit, Tauri or a real actor. The only
//! untested glue left is `PlatformServices::start` itself: three constructor calls.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::Duration;

use drift_clipboard::poll::{LocalChange, PasteboardPort, PasteboardWatcher};
use drift_core::{ClipboardPrefs, Clock, SystemClock, TriggerAction};
use drift_macos::ReconnectTriggers;
use drift_rdp::SessionCommand;
use tauri::{AppHandle, Runtime};

use crate::manager::SessionManager;

/// How often the main thread polls the pasteboard (`drift_clipboard::poll::POLL_INTERVAL`).
pub const CLIPBOARD_POLL_INTERVAL: Duration = drift_clipboard::poll::POLL_INTERVAL;

// ---- the session port ------------------------------------------------------------------------

/// One live session, as the platform services see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    /// The session window's Tauri label.
    pub window: String,
    /// That session's profile clipboard preference.
    pub clipboard: ClipboardPrefs,
    /// The session is in [`drift_core::SessionState::Reconnecting`], i.e. waiting out a
    /// backoff delay that a trigger may cut short.
    pub reconnecting: bool,
    /// Identifies the *actor*, not the window: reconnecting a window gives a new generation, so
    /// the clipboard pump knows the replacement has to be seeded again.
    pub generation: u64,
}

impl LiveSession {
    /// A session that is not reconnecting, in generation 0.
    pub fn new(window: impl Into<String>, clipboard: ClipboardPrefs) -> Self {
        Self { window: window.into(), clipboard, reconnecting: false, generation: 0 }
    }

    /// Sets [`Self::reconnecting`].
    #[must_use]
    pub fn reconnecting(mut self, reconnecting: bool) -> Self {
        self.reconnecting = reconnecting;
        self
    }

    /// Sets [`Self::generation`].
    #[must_use]
    pub fn generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    /// The key the clipboard pump remembers a seeded session by.
    fn key(&self) -> (String, u64) {
        (self.window.clone(), self.generation)
    }
}

/// The sessions a platform service fans out to (production: [`SessionManager`]).
pub trait SessionFanout: Send + Sync + 'static {
    /// Every window with a live session actor, in an arbitrary order.
    fn live_sessions(&self) -> Vec<LiveSession>;

    /// Sends `cmd` to `window`'s live session; `false` if there is none (it just ended).
    fn send(&self, window: &str, cmd: SessionCommand) -> bool;
}

/// Note: [`SessionManager`] also has an inherent `live_sessions()` returning the *count*, which
/// shadows this one; call the trait method as `SessionFanout::live_sessions(&manager)`.
impl SessionFanout for SessionManager {
    fn live_sessions(&self) -> Vec<LiveSession> {
        self.live_session_states()
            .into_iter()
            .map(|(window, clipboard, reconnecting, generation)| LiveSession {
                window,
                clipboard,
                reconnecting,
                generation,
            })
            .collect()
    }

    fn send(&self, window: &str, cmd: SessionCommand) -> bool {
        SessionManager::send(self, window, cmd).is_ok()
    }
}

// ---- M5-3 / M5-2: local → remote clipboard ---------------------------------------------------

/// The level a single pasteboard read must satisfy for every live session: the most permissive
/// preference among them, so images are read only when at least one session wants them.
pub fn most_permissive(levels: impl IntoIterator<Item = ClipboardPrefs>) -> ClipboardPrefs {
    levels.into_iter().max_by_key(|level| rank(*level)).unwrap_or(ClipboardPrefs::Off)
}

const fn rank(level: ClipboardPrefs) -> u8 {
    match level {
        ClipboardPrefs::Off => 0,
        ClipboardPrefs::Text => 1,
        ClipboardPrefs::TextAndImages => 2,
    }
}

/// Drives one [`PasteboardWatcher`] and fans local copies out to every live session (M5-2:
/// each session's `ClipboardSync` then decides — only the focused window advertises, and a
/// session ignores the change it caused itself).
///
/// A session that opens later is *seeded* with the current pasteboard, so its very first
/// format list already offers what the user copied before connecting. That is what
/// [`PasteboardWatcher::snapshot`] exists for.
#[derive(Debug)]
pub struct ClipboardPump<P> {
    port: P,
    watcher: PasteboardWatcher,
    seeded: BTreeSet<(String, u64)>,
}

impl<P: PasteboardPort> ClipboardPump<P> {
    /// A pump over `port`. The pasteboard's state at start-up is not a change; the sessions
    /// that open afterwards are seeded with it.
    pub fn new(port: P) -> Self {
        let watcher = PasteboardWatcher::new(port.change_count());
        Self { port, watcher, seeded: BTreeSet::new() }
    }

    /// The pasteboard this pump polls.
    pub fn port(&self) -> &P {
        &self.port
    }

    /// One poll tick. Returns how many sessions were told about the clipboard.
    ///
    /// The pasteboard is read at most once per tick, and only when something changed (or a new
    /// session needs seeding). Sessions whose clipboard preference is `Off` are skipped.
    pub fn tick(&mut self, sessions: &dyn SessionFanout) -> usize {
        let live = sessions.live_sessions();
        let level = most_permissive(live.iter().map(|s| s.clipboard));
        let syncing = || live.iter().filter(|s| s.clipboard != ClipboardPrefs::Off);

        if let Some(LocalChange { contents, .. }) = self.watcher.poll(&self.port, level) {
            // Everyone hears about a real change, so nobody needs seeding afterwards.
            self.seeded = live.iter().map(LiveSession::key).collect();
            let cmd = SessionCommand::ClipboardLocalChanged(contents);
            return syncing().filter(|s| sessions.send(&s.window, cmd.clone())).count();
        }

        let fresh: Vec<&LiveSession> = syncing().filter(|s| !self.seeded.contains(&s.key())).collect();
        // Sessions that ended (and generations that were replaced) are forgotten here.
        self.seeded = live.iter().map(LiveSession::key).collect();
        if fresh.is_empty() {
            return 0;
        }
        let snapshot = PasteboardWatcher::snapshot(&self.port, level);
        if snapshot.contents.is_empty() {
            return 0;
        }
        let cmd = SessionCommand::ClipboardLocalChanged(snapshot.contents);
        fresh.iter().filter(|s| sessions.send(&s.window, cmd.clone())).count()
    }
}

// ---- M7-2: reconnect triggers ----------------------------------------------------------------

/// The commands one merged trigger action turns into for a session that is (or is not)
/// waiting out a reconnect backoff.
///
/// `RetryNow` restores reachability *before* asking for the reconnect, because the actor
/// ignores `ReconnectNow` while it believes the network is down (`drift_rdp::actor::backoff`).
///
/// Only a *reconnecting* session is told to skip its delay. `drift_rdp::actor::idle` reconnects
/// on `ReconnectNow`, so sending it to every session would restart one the user cancelled on
/// the reconnect overlay (M7-3) and would retry an `AuthFailed` session after every wake. A
/// session in backoff does not need it either — `NetworkReachable(true)` already resumes it —
/// but it is what plan §3 specifies for `RetryNow`, and it closes the window between the
/// actor's states.
pub fn trigger_commands(action: TriggerAction, reconnecting: bool) -> Vec<SessionCommand> {
    match action {
        TriggerAction::PauseReconnect => vec![SessionCommand::NetworkReachable(false)],
        TriggerAction::RetryNow if reconnecting => {
            vec![SessionCommand::NetworkReachable(true), SessionCommand::ReconnectNow]
        }
        TriggerAction::RetryNow => vec![SessionCommand::NetworkReachable(true)],
    }
}

/// Applies one merged trigger action to every live session; returns how many commands landed.
pub fn apply_trigger(sessions: &dyn SessionFanout, action: TriggerAction) -> usize {
    sessions
        .live_sessions()
        .iter()
        .map(|s| {
            trigger_commands(action, s.reconnecting)
                .into_iter()
                .filter(|cmd| sessions.send(&s.window, cmd.clone()))
                .count()
        })
        .sum()
}

/// The sink to hand to `drift_macos::ReconnectTriggers::start` (or a `TriggerFeed` in tests).
///
/// It runs on the path monitor's dispatch queue or on the main thread, so it only pushes into
/// the actors' command channels.
pub fn trigger_sink(sessions: Arc<dyn SessionFanout>) -> impl Fn(TriggerAction) + Send + Sync + 'static {
    move |action| {
        let reached = apply_trigger(&*sessions, action);
        tracing::debug!(?action, reached, "reconnect trigger");
    }
}

// ---- the main-thread timer -------------------------------------------------------------------

/// What a [`PollTimer`] tick asks the timer to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// Keep polling.
    Continue,
    /// Stop the timer (the app is going away).
    Stop,
}

/// A repeating timer on its own thread. Stops when it is dropped or when a tick returns
/// [`Tick::Stop`]; the thread is never joined, so dropping it can never block the main thread.
#[derive(Debug)]
pub struct PollTimer {
    /// Dropping this wakes the timer thread out of its wait at once.
    _stop: Sender<()>,
}

impl PollTimer {
    /// Starts a timer calling `tick` every `interval` (the first call is one `interval` in).
    pub fn start(interval: Duration, mut tick: impl FnMut() -> Tick + Send + 'static) -> Self {
        let (stop, wait) = channel::<()>();
        let spawned = std::thread::Builder::new()
            .name("drift-poll-timer".into())
            .spawn(move || {
                while matches!(wait.recv_timeout(interval), Err(RecvTimeoutError::Timeout)) {
                    if tick() == Tick::Stop {
                        break;
                    }
                }
            })
            .is_ok();
        if !spawned {
            tracing::error!("could not start the pasteboard poll timer");
        }
        Self { _stop: stop }
    }
}

// ---- production wiring -------------------------------------------------------------------------

thread_local! {
    /// The main thread's pasteboard pump. `NsPasteboard` is not `Send`, and plan M5-3 requires
    /// the poll to happen on the main thread, so the pump lives here and the timer thread only
    /// dispatches a tick to it.
    static PUMP: RefCell<Option<ClipboardPump<drift_clipboard::pasteboard::NsPasteboard>>> =
        const { RefCell::new(None) };
}

/// Polls the general pasteboard once. Must be called on the main thread.
fn pump_once(sessions: &dyn SessionFanout) -> usize {
    let told = PUMP.with_borrow_mut(|pump| {
        pump.get_or_insert_with(|| ClipboardPump::new(drift_clipboard::pasteboard::NsPasteboard::general()))
            .tick(sessions)
    });
    // Per tick, so `RUST_LOG=drift_app=trace` shows the main-thread timer is alive; never the
    // contents, which may be anything the user copied.
    tracing::trace!(told, "pasteboard poll");
    told
}

/// The app's platform services. Dropping it stops both.
#[derive(Debug)]
pub struct PlatformServices {
    _clipboard: PollTimer,
    _triggers: ReconnectTriggers,
}

impl PlatformServices {
    /// Starts the pasteboard poll timer and the reconnect triggers for `sessions`.
    ///
    /// Call from `run_with`'s `setup` (main thread) and keep the value alive for the app's
    /// lifetime (Tauri managed state).
    pub fn start<R: Runtime>(app: &AppHandle<R>, sessions: SessionManager) -> Self {
        Self::start_with(app, Arc::new(sessions), Arc::new(SystemClock))
    }

    /// [`Self::start`] with an explicit fanout and clock.
    pub fn start_with<R: Runtime>(
        app: &AppHandle<R>,
        sessions: Arc<dyn SessionFanout>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let triggers = ReconnectTriggers::start(clock, trigger_sink(sessions.clone()));
        let app = app.clone();
        let clipboard = PollTimer::start(CLIPBOARD_POLL_INTERVAL, move || {
            let sessions = sessions.clone();
            // `on_main` blocks until the main thread has polled, so ticks cannot pile up.
            match crate::windows::on_main(&app, move |_| pump_once(&*sessions)) {
                Ok(_) => Tick::Continue,
                // The main thread is gone (the app is quitting).
                Err(_) => Tick::Stop,
            }
        });
        tracing::debug!(
            poll_interval = ?CLIPBOARD_POLL_INTERVAL,
            online = triggers.feed().is_online(),
            "platform services started"
        );
        Self { _clipboard: clipboard, _triggers: triggers }
    }
}
