//! Public interface of the session actor (M0-5). See `docs/adr/M0-5-session-interface.md`.
//!
//! One actor per tab runs as a Tokio task. The app talks to it only through:
//!
//! - [`SessionHandle`]: a cheap, clonable sender of [`SessionCommand`]s. Sending never
//!   blocks, so AppKit event handlers may call it directly on the main thread.
//! - [`SessionEvents`]: the receiver of [`SessionEvent`]s, drained by the app's
//!   `SessionManager` and forwarded to the tab's window.
//!
//! Pixels never cross this interface: the actor drives the tab's [`FrameSink`] (the
//! Metal compositor) directly on the session render thread.
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use drift_core::{ConnectionProfile, ConnectMode, SystemClock};
//! # use drift_rdp::{spawn_session, SessionCommand, SessionEvent, SessionOptions, SessionSecrets};
//! # async fn demo(sink: Box<dyn drift_gfx::FrameSink>) {
//! let profile = ConnectionProfile::new("Headless", "10.0.2.40", ConnectMode::Headless);
//! let secrets = SessionSecrets::new("…password from Keychain…");
//! let (handle, mut events) =
//!     spawn_session(profile, secrets, sink, Arc::new(SystemClock), SessionOptions::default());
//! while let Some(ev) = events.recv().await {
//!     if let SessionEvent::State(s) = ev { println!("{s:?}"); }
//! }
//! handle.send(SessionCommand::Close).ok();
//! # }
//! ```

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use drift_clipboard::ClipboardContents;
use drift_core::{
    CertFingerprint, Clock, ConnectionProfile, InputEvent, Point, ReconnectConfig, SessionState, Size,
    ViewGeometry,
};
use drift_gfx::FrameSink;
pub use drift_input::ScaleMode;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

/// Long-lived secrets for one session, fetched from the Keychain by the app.
///
/// One-time Server Redirection credentials are **not** here: they live only inside the
/// actor and are zeroized after use (plan §2 decision 7).
#[derive(Clone)]
pub struct SessionSecrets {
    /// RDP password: system credentials (RemoteLogin) or daemon credentials.
    pub rdp_password: Zeroizing<String>,
    /// RemoteLogin only: Linux password typed into the greeter after explicit opt-in (M3-2).
    pub linux_password: Option<Zeroizing<String>>,
}

impl SessionSecrets {
    /// Secrets with only an RDP password.
    pub fn new(rdp_password: impl Into<String>) -> Self {
        Self { rdp_password: Zeroizing::new(rdp_password.into()), linux_password: None }
    }
}

impl fmt::Debug for SessionSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionSecrets")
            .field("rdp_password", &"<redacted>")
            .field("linux_password", &self.linux_password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Connection options that are not part of the saved profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOptions {
    /// TLS server name / certificate host override. Used by e2e tests that connect to an
    /// SSH forward on `127.0.0.1` while the certificate belongs to `10.0.2.40`.
    pub tls_server_name: Option<String>,
    /// RDP client name (defaults to the Mac's host name).
    pub client_name: String,
    /// Per-leg connect timeout.
    pub connect_timeout: Duration,
    /// Auto-reconnect backoff (M7-1/M7-3).
    pub reconnect: ReconnectConfig,
    /// Seed for the backoff jitter; `None` seeds from the system time (tests pin it).
    pub reconnect_seed: Option<u64>,
    /// Play the server's sound here (HOST-11). With `None` the client does not offer the sound
    /// channel.
    pub audio: Option<crate::audio::AudioOutput>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            tls_server_name: None,
            client_name: crate::connect::local_client_name(),
            connect_timeout: Duration::from_secs(15),
            reconnect: ReconnectConfig::default(),
            reconnect_seed: None,
            audio: None,
        }
    }
}

/// Which certificate a [`SessionEvent::CertificatePrompt`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CertificateRole {
    /// The server named in the profile (leg 1).
    Server,
    /// A Server Redirection target (legs ≥ 2).
    RedirectTarget,
}

/// Commands from the app to the session actor.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionCommand {
    /// Forward an input event (dropped unless `Connected`/`AwaitingGreeterLogin`).
    Input(InputEvent),
    /// The remote view's geometry changed (drives Display Control, M4-2).
    Resize(ViewGeometry),
    /// Tab became visible/occluded (Suppress Output + ack suspension, M6-3).
    SetVisible(bool),
    /// Window focus changed (clipboard scoping M5-2; focus loss releases keys M2-1).
    Focus(bool),
    /// The local pasteboard changed (M5-2/M5-3).
    ClipboardLocalChanged(ClipboardContents),
    /// The user trusts the prompted certificate. `pin = true` persists it (TOFU).
    AcceptCertificate {
        /// The fingerprint that was shown to the user.
        fingerprint: CertFingerprint,
        /// Persist as the profile's pin.
        pin: bool,
    },
    /// The user rejected the prompted certificate (ends in `Failed{CertMismatch}`).
    RejectCertificate,
    /// Skip the backoff delay and reconnect immediately (overlay button, network up, wake).
    ReconnectNow,
    /// The network became unreachable (`false`) or reachable (`true`) (M7-2).
    NetworkReachable(bool),
    /// Stop reconnecting; stay `Disconnected`.
    Cancel,
    /// Graceful shutdown (sends Shutdown Request at the greeter, M3-4), then the actor exits.
    Close,
}

/// A remote pointer shape or position update (M2-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorUpdate {
    /// Hide the pointer.
    Hidden,
    /// Use the system default arrow.
    Default,
    /// A new pointer image.
    Bitmap(CursorBitmap),
    /// Server-initiated pointer move (desktop pixels).
    Position(Point<u32>),
}

/// A decoded pointer image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorBitmap {
    /// Image size in desktop pixels (86×86 at scale 200, 43×43 at 100).
    pub size: Size<u32>,
    /// Hotspot in image pixels.
    pub hotspot: Point<u32>,
    /// Premultiplied BGRA8 pixels, top-down, `size.width * 4` stride.
    pub bgra: Arc<[u8]>,
    /// Desktop scale factor in percent; point size is `size * 100 / scale`.
    pub scale: u32,
}

/// Features the server offered on this connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionCapabilities {
    /// Display Control channel available (false for Desktop Sharing → scale to fit).
    pub display_control: bool,
    /// Clipboard channel available.
    pub clipboard: bool,
    /// How the app places the desktop in the view (M4-2: `Fit` for Desktop Sharing).
    pub scale_mode: ScaleMode,
}

/// Periodic session statistics (about once per second while connected).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SessionStats {
    /// Presented frames per second.
    pub fps: f32,
    /// Received payload bit rate.
    pub bitrate_bps: u64,
    /// Decode + present latency, 95th percentile, milliseconds (plan M9-1: < 8 ms).
    pub frame_latency_p95_ms: f32,
    /// Input-to-wire latency, 99th percentile, milliseconds (plan M9-1: < 2 ms). `0.0` when
    /// no input was sent during the window.
    pub input_to_wire_p99_ms: f32,
    /// Frames awaiting acknowledgement.
    pub unacked_frames: u32,
}

/// Events from the session actor to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// The session state changed (every change goes through `SessionState::transition`).
    State(SessionState),
    /// An unknown certificate needs a user decision; the actor waits for
    /// [`SessionCommand::AcceptCertificate`] or [`SessionCommand::RejectCertificate`].
    CertificatePrompt {
        /// Host as shown to the user.
        host: String,
        /// Port.
        port: u16,
        /// SHA-256 of the leaf DER, `grdctl` format via `Display`.
        fingerprint: CertFingerprint,
        /// Which certificate.
        role: CertificateRole,
    },
    /// A certificate was accepted with `pin = true`; the app stores it in the profile.
    CertificatePinned(CertFingerprint),
    /// Server capabilities for this connection (sent after each activation).
    Capabilities(SessionCapabilities),
    /// Pointer update.
    Cursor(CursorUpdate),
    /// The remote clipboard changed; the preferred formats were fetched eagerly (M5-2).
    ClipboardRemote(ClipboardContents),
    /// Statistics.
    Stats(SessionStats),
}

/// The actor has exited; the command was not delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("session actor has exited")]
pub struct SessionClosed;

/// Clonable, non-blocking command sender for one session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    tx: mpsc::UnboundedSender<SessionCommand>,
}

impl SessionHandle {
    /// Wraps a command channel. Used by `spawn_session` and by fake actors in app tests.
    pub fn from_sender(tx: mpsc::UnboundedSender<SessionCommand>) -> Self {
        Self { tx }
    }

    /// Sends a command without blocking.
    pub fn send(&self, cmd: SessionCommand) -> Result<(), SessionClosed> {
        self.tx.send(cmd).map_err(|_| SessionClosed)
    }

    /// `true` once the actor has exited.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Receiver of a session's events. Yields `None` after the actor exits.
pub type SessionEvents = mpsc::UnboundedReceiver<SessionEvent>;

/// Starts a session actor for `profile` on the current Tokio runtime.
///
/// The actor immediately begins connecting (`Idle` → `Connecting{leg: 1}`), drives
/// `frame_sink` from the session render thread, and uses `clock` for every timer
/// (timeouts, debounce, backoff). It exits after [`SessionCommand::Close`] or when the
/// [`SessionHandle`] and all its clones are dropped, emitting a final
/// `State(Disconnected{UserClosed})`.
///
/// # Panics
/// Must be called from within a Tokio runtime.
///
/// Implemented in `actor.rs` (M1-1 connect, M3-1 redirect loop).
pub fn spawn_session(
    profile: ConnectionProfile,
    secrets: SessionSecrets,
    frame_sink: Box<dyn FrameSink>,
    clock: Arc<dyn Clock>,
    options: SessionOptions,
) -> (SessionHandle, SessionEvents) {
    crate::actor::spawn(profile, secrets, frame_sink, clock, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_sends_until_receiver_dropped() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let h = SessionHandle::from_sender(tx);
        assert!(h.send(SessionCommand::ReconnectNow).is_ok());
        assert_eq!(rx.try_recv().ok(), Some(SessionCommand::ReconnectNow));
        assert!(!h.is_closed());
        drop(rx);
        assert!(h.is_closed());
        assert_eq!(h.clone().send(SessionCommand::Close), Err(SessionClosed));
        assert_eq!(SessionClosed.to_string(), "session actor has exited");
    }

    #[test]
    fn secrets_debug_is_redacted() {
        let mut s = SessionSecrets::new("hunter2-Fake9");
        s.linux_password = Some(Zeroizing::new("other-Fake9".into()));
        let d = format!("{s:?}");
        assert!(!d.contains("hunter2") && !d.contains("other"), "{d}");
        assert_eq!(SessionOptions::default().connect_timeout, Duration::from_secs(15));
    }
}
