//! The per-window view model sent to the webview (M1-6, M3-2, M7-3 overlays).
//!
//! The webview is a thin renderer: Rust folds [`SessionEvent`]s into a [`SessionView`] snapshot
//! (including which [`Screen`] to show) and emits it as [`SessionViewChanged`]; TypeScript only
//! switches on `screen` and renders. The `SessionManager` owns one `SessionView` per window.

use drift_core::reconnect::ReconnectConfig;
use drift_core::{
    CertFingerprint, ConnectMode, ConnectionProfile, ErrorExplanation, SessionState, explain_disconnect,
};
use drift_rdp::{CertificateRole, SessionEvent, SessionStats};
use serde::{Deserialize, Serialize};

/// Which screen the webview shows for a session window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "kebab-case")]
pub enum Screen {
    /// No session: the connect / profiles form.
    Profiles,
    /// Connecting progress.
    Connecting,
    /// An unknown certificate needs a decision.
    Certificate,
    /// Remote Login greeter is live: a non-modal hint banner over the picture.
    GreeterHint,
    /// Backoff overlay over the dimmed last frame: "Reconnecting in N s… [Now] [Cancel]".
    Reconnecting,
    /// Ended with an explanation and next steps.
    Error,
    /// Live desktop: the webview is hidden, unless the statistics HUD is switched on.
    Live,
}

/// Which certificate a prompt is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "kebab-case")]
pub enum CertificateSubject {
    /// The host in the profile.
    Server,
    /// A Server Redirection target (Remote Login legs ≥ 2).
    RedirectTarget,
}

impl From<CertificateRole> for CertificateSubject {
    fn from(role: CertificateRole) -> Self {
        match role {
            CertificateRole::Server => Self::Server,
            CertificateRole::RedirectTarget => Self::RedirectTarget,
        }
    }
}

/// A pending certificate decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct CertificatePrompt {
    /// Host as shown to the user.
    pub host: String,
    /// Port.
    pub port: u16,
    /// SHA-256 of the leaf certificate, in `grdctl status` format.
    pub fingerprint: CertFingerprint,
    /// Which certificate.
    pub subject: CertificateSubject,
    /// The command that prints the same fingerprint on the host, for this mode.
    pub grdctl_command: String,
}

/// One statistics sample, quantised to what the HUD actually prints.
///
/// [`drift_rdp::SessionStats`] is sampled about once per second; the numbers are rounded to one
/// decimal here so that two samples that would draw the same line compare equal and the view is
/// not re-emitted (plan M1 "Done (manual M1)", M9-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
pub struct StatsView {
    /// Presented frames per second, in tenths (`589` prints as `58.9 fps`).
    pub fps_tenths: u32,
    /// Received payload bit rate, in tenths of a Mbit/s.
    pub mbit_tenths: u32,
    /// Decode + present latency, 95th percentile, in tenths of a millisecond.
    pub latency_p95_tenths_ms: u32,
    /// Frames awaiting acknowledgement.
    pub unacked_frames: u32,
}

/// One decimal as a whole number of tenths; negative or non-finite input becomes `0`.
///
/// Integers keep the type honest on the TypeScript side: specta maps `f32` to `number | null`,
/// because `serde_json` writes a non-finite float as `null`.
fn tenths(value: f32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped above")]
    let tenths = (f64::from(value) * 10.0).round().min(f64::from(u32::MAX)) as u32;
    tenths
}

impl StatsView {
    /// The displayed form of one actor sample.
    pub fn sample(stats: &SessionStats) -> Self {
        #[expect(clippy::cast_precision_loss, reason = "a bit rate is a display value")]
        let mbit = stats.bitrate_bps as f32 / 1_000_000.0;
        Self {
            fps_tenths: tenths(stats.fps),
            mbit_tenths: tenths(mbit),
            latency_p95_tenths_ms: tenths(stats.frame_latency_p95_ms),
            unacked_frames: stats.unacked_frames,
        }
    }
}

/// Everything a session window's webview needs to render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SessionView {
    /// Profile display name.
    pub profile_name: String,
    /// Profile mode.
    pub mode: ConnectMode,
    /// Remote Login: the Linux user to log in as (greeter hint).
    pub linux_username: Option<String>,
    /// Current session state.
    pub state: SessionState,
    /// What to render.
    pub screen: Screen,
    /// Set while a certificate prompt is pending.
    pub certificate: Option<CertificatePrompt>,
    /// Set for `Disconnected` / `Failed` states.
    pub explanation: Option<ErrorExplanation>,
    /// Remote Login: the greeter appeared after this window had already shown a desktop, so the
    /// session is still running ("log in to resume").
    pub resuming: bool,
    /// Reconnect attempt budget shown in the overlay (`None` = unlimited).
    pub max_attempts: Option<u32>,
    /// The statistics HUD is switched on for this window (Session ▸ Show Statistics). It is a
    /// per-window user choice, so no session event ever changes it.
    pub show_stats: bool,
    /// Latest statistics sample, or `None` before the first one and once the picture is gone.
    pub stats: Option<StatsView>,
}

/// Command that prints the TLS fingerprint on the host for `mode`.
pub fn grdctl_status_command(mode: ConnectMode) -> &'static str {
    match mode {
        ConnectMode::RemoteLogin => "sudo grdctl --system status",
        ConnectMode::Headless => "grdctl --headless status",
        ConnectMode::DesktopSharing => "grdctl status",
    }
}

impl SessionView {
    /// Initial view for a window that is about to connect `profile`.
    pub fn new(profile: &ConnectionProfile) -> Self {
        let mut view = Self {
            profile_name: profile.name.clone(),
            mode: profile.mode,
            linux_username: profile.linux_username.clone(),
            state: SessionState::Idle,
            screen: Screen::Profiles,
            certificate: None,
            explanation: None,
            resuming: false,
            max_attempts: ReconnectConfig::default().max_attempts,
            show_stats: false,
            stats: None,
        };
        view.refresh();
        view
    }

    /// Folds one actor event into the view. Returns `true` if the view changed (emit it).
    pub fn apply(&mut self, event: &SessionEvent) -> bool {
        let before = self.clone();
        match event {
            SessionEvent::State(state) => {
                if matches!(state, SessionState::Connected { .. }) {
                    self.resuming = true;
                }
                if matches!(state, SessionState::Idle) {
                    self.resuming = false;
                }
                // Stale numbers must not linger under the reconnect overlay or the error screen.
                if !matches!(state, SessionState::Connected { .. } | SessionState::AwaitingGreeterLogin) {
                    self.stats = None;
                }
                self.state = state.clone();
                self.certificate = None;
            }
            SessionEvent::CertificatePrompt { host, port, fingerprint, role } => {
                self.certificate = Some(CertificatePrompt {
                    host: host.clone(),
                    port: *port,
                    fingerprint: *fingerprint,
                    subject: (*role).into(),
                    grdctl_command: grdctl_status_command(self.mode).into(),
                });
            }
            SessionEvent::Stats(stats) => self.stats = Some(StatsView::sample(stats)),
            _ => {}
        }
        self.refresh();
        *self != before
    }

    /// The user answered the certificate prompt (the actor continues or fails).
    pub fn clear_certificate(&mut self) {
        self.certificate = None;
        self.refresh();
    }

    fn refresh(&mut self) {
        self.explanation = match &self.state {
            SessionState::Disconnected { reason } | SessionState::Failed { reason } => {
                Some(explain_disconnect(reason, self.mode))
            }
            _ => None,
        };
        self.screen = if self.certificate.is_some() {
            Screen::Certificate
        } else {
            match self.state {
                SessionState::Idle => Screen::Profiles,
                SessionState::Connecting { .. } => Screen::Connecting,
                SessionState::AwaitingGreeterLogin => Screen::GreeterHint,
                SessionState::Connected { .. } => Screen::Live,
                SessionState::Reconnecting { .. } => Screen::Reconnecting,
                SessionState::Disconnected { .. } | SessionState::Failed { .. } => Screen::Error,
            }
        };
    }
}

/// Emitted to a session window whenever its [`SessionView`] changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct SessionViewChanged(pub SessionView);
