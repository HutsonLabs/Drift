//! User-facing explanations for disconnects, with next steps (tasks M1-6, M9-4).
//!
//! Kept in Rust (not in the webview) so the mode-specific wording is table-tested; the UI only
//! renders an [`ErrorExplanation`] and wires its [`ErrorAction`]s to commands.

use serde::{Deserialize, Serialize};

use crate::profile::ConnectMode;
use crate::state::DisconnectReason;

/// `x-apple.systempreferences` URL of System Settings › Privacy & Security › Local Network.
pub const LOCAL_NETWORK_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";

/// Human path to the Local Network privacy pane, as shown in the UI.
pub const LOCAL_NETWORK_SETTINGS_PATH: &str = "System Settings › Privacy & Security › Local Network";

/// A button offered on an error screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum ErrorAction {
    /// Connect again.
    Reconnect,
    /// Open the profile form.
    EditProfile,
    /// Open System Settings › Privacy & Security › Local Network.
    OpenLocalNetworkSettings,
    /// Close the window.
    Close,
}

/// What went wrong, in words, and what the user can do about it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ErrorExplanation {
    /// Short headline.
    pub title: String,
    /// One or two sentences describing the problem.
    pub message: String,
    /// Concrete next steps, in order.
    pub next_steps: Vec<String>,
    /// Buttons to offer; the first is the default.
    pub actions: Vec<ErrorAction>,
}

fn explanation(
    title: &str,
    message: impl Into<String>,
    steps: &[&str],
    actions: &[ErrorAction],
) -> ErrorExplanation {
    ErrorExplanation {
        title: title.into(),
        message: message.into(),
        next_steps: steps.iter().map(|s| (*s).to_owned()).collect(),
        actions: actions.to_vec(),
    }
}

/// Where the RDP credentials of each mode are configured on the host.
fn credential_hint(mode: ConnectMode) -> &'static str {
    match mode {
        ConnectMode::RemoteLogin => {
            "Use the system RDP credentials: on the host run “sudo grdctl --system status --show-credentials”."
        }
        ConnectMode::Headless => {
            "Use the credentials set with “grdctl --headless rdp set-credentials” for that user on the host."
        }
        ConnectMode::DesktopSharing => {
            "Desktop Sharing credentials must be set in GNOME Settings on the host."
        }
    }
}

/// The service that must be running on the host for each mode.
fn service_hint(mode: ConnectMode) -> &'static str {
    match mode {
        ConnectMode::RemoteLogin => {
            "Check that Remote Login is enabled on the host: “sudo grdctl --system status”."
        }
        ConnectMode::Headless => "Headless session not running — see host/drift-host-setup.sh.",
        ConnectMode::DesktopSharing => {
            "Check that Desktop Sharing is on in GNOME Settings › System › Remote Desktop and that someone is logged in on the host."
        }
    }
}

/// Where to read the certificate fingerprint on the host, per mode.
fn fingerprint_hint(mode: ConnectMode) -> &'static str {
    match mode {
        ConnectMode::RemoteLogin => "Compare it with “sudo grdctl --system status” on the host.",
        ConnectMode::Headless => "Compare it with “grdctl --headless status” on the host.",
        ConnectMode::DesktopSharing => "Compare it with “grdctl status” on the host.",
    }
}

/// Explains why a session with `mode` ended with `reason` (M9-4 texts).
pub fn explain_disconnect(reason: &DisconnectReason, mode: ConnectMode) -> ErrorExplanation {
    use ErrorAction::{Close, EditProfile, OpenLocalNetworkSettings, Reconnect};

    let check_address = "Check the host name and port in the profile.";
    match reason {
        DisconnectReason::Network => explanation(
            "Can’t reach the host",
            "Drift couldn’t connect to the remote computer.",
            &[service_hint(mode), check_address],
            &[Reconnect, EditProfile],
        ),
        DisconnectReason::Timeout => explanation(
            "The host didn’t respond",
            "The connection timed out.",
            &[service_hint(mode), check_address],
            &[Reconnect, EditProfile],
        ),
        DisconnectReason::TlsEof => explanation(
            "Connection interrupted",
            "The secure connection to the host closed unexpectedly.",
            &["Reconnect. If this keeps happening, check the network between this Mac and the host."],
            &[Reconnect, Close],
        ),
        DisconnectReason::ServerShutdown => explanation(
            "The remote desktop service stopped",
            "GNOME Remote Desktop on the host shut the connection down, for example because it restarted.",
            &[service_hint(mode), "Then reconnect."],
            &[Reconnect, Close],
        ),
        DisconnectReason::AuthFailed => explanation(
            "Wrong user name or password",
            "The host rejected the RDP credentials in this profile.",
            &[credential_hint(mode), "Update the user name and password in the profile."],
            &[EditProfile, Reconnect],
        ),
        DisconnectReason::RdstlsFailed(code) => explanation(
            "Login hand-off failed",
            format!(
                "The host rejected the one-time login that follows the GNOME login screen (code 0x{code:X}). One-time logins can’t be reused."
            ),
            &["Reconnect to start again from the login screen."],
            &[Reconnect, Close],
        ),
        DisconnectReason::CertMismatch => explanation(
            "The host’s certificate changed",
            "The certificate doesn’t match the one you trusted before. The host may have been reinstalled, or someone may be intercepting the connection.",
            &[
                fingerprint_hint(mode),
                "If you trust the change, clear the saved certificate in the profile and connect again.",
            ],
            &[EditProfile, Close],
        ),
        DisconnectReason::ProtocolError(detail) => explanation(
            "Unexpected data from the host",
            format!("The connection failed because the host sent something Drift didn’t expect ({detail})."),
            &[
                "Reconnect. If this keeps happening, check that the host runs GNOME Remote Desktop 50 or later.",
            ],
            &[Reconnect, Close],
        ),
        DisconnectReason::RedirectLoop => explanation(
            "Too many redirects",
            "The host kept redirecting the connection instead of opening a session.",
            &["End stale remote sessions on the host (see “loginctl list-sessions”), then reconnect."],
            &[Reconnect, Close],
        ),
        DisconnectReason::UserClosed => {
            explanation("Disconnected", "You closed the connection.", &[], &[Reconnect, Close])
        }
        DisconnectReason::LoggedOffRemotely => explanation(
            "Logged out",
            "The remote session was logged out on the host.",
            &["Reconnect to log in again."],
            &[Reconnect, Close],
        ),
        DisconnectReason::LocalNetworkDenied => ErrorExplanation {
            title: "Drift can’t access your local network".into(),
            message: "macOS blocked Drift from connecting to computers on your local network.".into(),
            next_steps: vec![
                format!("Open {LOCAL_NETWORK_SETTINGS_PATH} and turn on Drift."),
                "Then reconnect.".into(),
            ],
            actions: vec![OpenLocalNetworkSettings, Reconnect],
        },
    }
}
