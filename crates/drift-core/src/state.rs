//! Session state machine (plan §3) and disconnect classification.
//!
//! [`SessionState::transition`] is the single gate through which the session actor
//! changes state; it rejects transitions that the protocol flow cannot produce, so
//! bugs surface as `Err(InvalidTransition)` instead of a confused UI.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::geometry::DesktopSize;

/// Maximum Server Redirection PDUs followed per connection attempt before
/// [`DisconnectReason::RedirectLoop`] (plan M3-1).
pub const MAX_REDIRECTS: u8 = 4;

/// Highest valid connection leg: the initial leg plus [`MAX_REDIRECTS`] redirected legs.
/// Remote Login normally uses legs 1..=3 (NLA, greeter, user session).
pub const MAX_LEG: u8 = 1 + MAX_REDIRECTS;

/// Sub-stage of a connection leg, for progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum ConnectStage {
    /// TCP connect.
    Tcp,
    /// TLS handshake and certificate verification.
    Tls,
    /// NLA (CredSSP/NTLM), leg 1 and Headless/Sharing.
    Nla,
    /// RDSTLS with one-time redirect credentials (Remote Login legs ≥ 2).
    Rdstls,
    /// MCS / capability exchange / finalization.
    Activation,
}

/// Why a session stopped (plan §3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", content = "detail", rename_all = "kebab-case")]
pub enum DisconnectReason {
    /// Transport-level failure (reset, unreachable, DNS). Retryable.
    Network,
    /// TLS stream closed without close_notify. Retryable.
    TlsEof,
    /// The server shut the session down (e.g. daemon restart). Retryable.
    ServerShutdown,
    /// A connect or protocol timeout. Retryable.
    Timeout,
    /// NLA rejected the credentials. Not retryable.
    AuthFailed,
    /// RDSTLS AuthResponse returned a non-zero result (e.g. `0x52E`). Not retryable.
    RdstlsFailed(u32),
    /// The server certificate does not match the pin / redirect target certificate.
    CertMismatch,
    /// Malformed or unexpected server data.
    ProtocolError(String),
    /// More than [`MAX_REDIRECTS`] redirections in one attempt.
    RedirectLoop,
    /// The user closed the window or cancelled.
    UserClosed,
    /// The remote user logged off.
    LoggedOffRemotely,
    /// macOS Local Network Privacy denied the connection (`EHOSTUNREACH`, errno 65).
    LocalNetworkDenied,
}

impl DisconnectReason {
    /// Whether automatic reconnection may be attempted for this reason (plan M7-1).
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Network | Self::TlsEof | Self::ServerShutdown | Self::Timeout => true,
            Self::AuthFailed
            | Self::RdstlsFailed(_)
            | Self::CertMismatch
            | Self::ProtocolError(_)
            | Self::RedirectLoop
            | Self::UserClosed
            | Self::LoggedOffRemotely
            | Self::LocalNetworkDenied => false,
        }
    }
}

/// Lifecycle of one session (window), plan §3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum SessionState {
    /// Not started.
    Idle,
    /// Establishing connection leg `leg` (1-based, `1..=MAX_LEG`).
    Connecting {
        /// Connection leg (1 = initial, ≥ 2 = after a Server Redirection).
        leg: u8,
        /// Progress within the leg.
        stage: ConnectStage,
    },
    /// Remote Login: the GDM greeter is visible and the user must log in.
    AwaitingGreeterLogin,
    /// A live desktop is displayed.
    Connected {
        /// Remote desktop size in pixels.
        desktop: DesktopSize,
        /// `DesktopScaleFactor` in percent (100 or 200).
        scale: u32,
    },
    /// Waiting to retry after a retryable disconnect.
    Reconnecting {
        /// 1-based attempt number.
        attempt: u32,
        /// Delay until the next attempt (serialized as integer milliseconds).
        #[serde(with = "duration_ms")]
        #[cfg_attr(feature = "specta", specta(type = u32))]
        next_in: Duration,
        /// The retryable reason that triggered reconnection.
        reason: DisconnectReason,
    },
    /// Stopped; the user may reconnect manually.
    Disconnected {
        /// Why.
        reason: DisconnectReason,
    },
    /// Stopped with an error that needs user action.
    Failed {
        /// Why.
        reason: DisconnectReason,
    },
}

/// Serde adapter: `Duration` as whole milliseconds (a JS-safe number over IPC).
mod duration_ms {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        u64::deserialize(d).map(Duration::from_millis)
    }
}

/// A rejected state change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid session state transition: {from:?} -> {to:?}")]
pub struct InvalidTransition {
    /// The state before.
    pub from: SessionState,
    /// The rejected target state.
    pub to: SessionState,
}

impl SessionState {
    /// Validates a transition from `self` to `next` and returns `next` if allowed.
    ///
    /// Rules:
    /// - `Idle`, `Disconnected`, `Failed` → `Connecting{leg: 1}`; `Disconnected`/`Failed` → `Idle`;
    ///   `Idle` → `Disconnected{UserClosed}`.
    /// - `Connecting{leg}` → `Connecting{leg | leg+1}` (≤ [`MAX_LEG`]), `AwaitingGreeterLogin`
    ///   (only from leg ≥ 2), `Connected`.
    /// - `AwaitingGreeterLogin` / `Connected` → `Connecting{leg ≥ 2}` (a redirect);
    ///   `Connected` → `Connected` (resize).
    /// - `Reconnecting{attempt}` → `Reconnecting{attempt' ≥ attempt}`, `Connecting{leg: 1}`.
    /// - Any active state (`Connecting`, `AwaitingGreeterLogin`, `Connected`) → `Reconnecting`
    ///   (retryable reason only, `attempt ≥ 1`), `Disconnected`, `Failed`.
    /// - `Reconnecting` → `Disconnected`, `Failed`.
    /// - `Connecting.leg` must be in `1..=MAX_LEG`; `Reconnecting.reason` must be retryable.
    pub fn transition(&self, next: SessionState) -> Result<SessionState, InvalidTransition> {
        if self.allows(&next) { Ok(next) } else { Err(InvalidTransition { from: self.clone(), to: next }) }
    }

    fn allows(&self, next: &SessionState) -> bool {
        use SessionState as S;

        // Per-target validity, independent of the source state.
        match next {
            S::Connecting { leg, .. } if !(1..=MAX_LEG).contains(leg) => return false,
            S::Reconnecting { attempt, reason, .. } if *attempt == 0 || !reason.is_retryable() => {
                return false;
            }
            _ => {}
        }

        match (self, next) {
            // (Re)start at leg 1.
            (S::Idle | S::Disconnected { .. } | S::Failed { .. }, S::Connecting { leg: 1, .. }) => true,
            (S::Disconnected { .. } | S::Failed { .. }, S::Idle) => true,
            (S::Idle, S::Disconnected { reason: DisconnectReason::UserClosed }) => true,

            // Progress within a leg, or a redirect to the next leg.
            (S::Connecting { leg: from, .. }, S::Connecting { leg: to, .. }) => {
                *to == *from || Some(*to) == from.checked_add(1)
            }
            (S::Connecting { leg, .. }, S::AwaitingGreeterLogin) => *leg >= 2,
            (S::Connecting { .. }, S::Connected { .. }) => true,

            // A Server Redirection from the greeter or a live session.
            (S::AwaitingGreeterLogin | S::Connected { .. }, S::Connecting { leg, .. }) => *leg >= 2,
            // Resize (new desktop size / scale).
            (S::Connected { .. }, S::Connected { .. }) => true,

            // Drops from any active state.
            (
                S::Connecting { .. } | S::AwaitingGreeterLogin | S::Connected { .. },
                S::Reconnecting { .. } | S::Disconnected { .. } | S::Failed { .. },
            ) => true,

            // Backoff.
            (S::Reconnecting { attempt: from, .. }, S::Reconnecting { attempt: to, .. }) => to >= from,
            (S::Reconnecting { .. }, S::Connecting { leg: 1, .. }) => true,
            (S::Reconnecting { .. }, S::Disconnected { .. } | S::Failed { .. }) => true,

            _ => false,
        }
    }

    /// `true` while a transport is open or being opened.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Connecting { .. } | Self::AwaitingGreeterLogin | Self::Connected { .. })
    }
}
