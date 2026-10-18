//! IPC commands exposed to the webview.
//!
//! Commands are thin: profile logic lives in [`crate::profiles::ProfileService`] and
//! `drift-core`; session intents are forwarded to the `SessionManager` (M6-1); window intents to
//! `crate::windows` (UI-windows). "The calling window" is the `tauri::Window` a command came
//! from, so a session window's page and its title bar answer alike.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use drift_core::{
    CertFingerprint, ConnectMode, ConnectionProfile, DisconnectReason, ErrorExplanation, ProfileIssue,
    explain_disconnect,
};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use crate::connections::{Connections, ConnectionsIntent, WindowIdentity};
use crate::frames::{FrameStore, profile_key};
use crate::manager::SessionManager;
use crate::profiles::{CommandError, ProfileEntry, ProfileService, SecretsUpdate};

/// Shared state managed by Tauri.
#[derive(Debug)]
pub struct AppState {
    /// Saved profiles and their passwords.
    pub profiles: Arc<ProfileService>,
    /// Sessions, one per session window.
    pub sessions: SessionManager,
    /// Remembered window frames (`window-frames.json`).
    pub frames: FrameStore,
    next_window: AtomicU64,
    /// The profile of every session window from the moment it is built until it closes.
    window_profiles: Mutex<HashMap<String, ConnectionProfile>>,
    closing: Mutex<HashSet<String>>,
    quitting: AtomicBool,
}

impl AppState {
    /// The state for a running app.
    pub fn new(profiles: Arc<ProfileService>, sessions: SessionManager, frames: FrameStore) -> Self {
        Self {
            profiles,
            sessions,
            frames,
            next_window: AtomicU64::new(0),
            window_profiles: Mutex::new(HashMap::new()),
            closing: Mutex::new(HashSet::new()),
            quitting: AtomicBool::new(false),
        }
    }

    fn window_profiles(&self) -> std::sync::MutexGuard<'_, HashMap<String, ConnectionProfile>> {
        self.window_profiles.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Records (or, with `None`, forgets) the profile session window `window` is for.
    pub(crate) fn set_window_profile(&self, window: &str, profile: Option<ConnectionProfile>) {
        match profile {
            Some(profile) => self.window_profiles().insert(window.to_owned(), profile),
            None => self.window_profiles().remove(window),
        };
    }

    /// The profile session window `window` was built for.
    pub(crate) fn window_profile(&self, window: &str) -> Option<ConnectionProfile> {
        self.window_profiles().get(window).cloned()
    }

    /// The session window built (or being built) for `profile`.
    pub(crate) fn window_of_profile(&self, profile: Uuid) -> Option<String> {
        self.window_profiles().iter().find(|(_, p)| p.id == profile).map(|(w, _)| w.clone())
    }

    /// The number of the next session window.
    pub(crate) fn next_window(&self) -> u64 {
        self.next_window.fetch_add(1, Ordering::Relaxed)
    }

    fn closing(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.closing.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Marks `window` as closing; `false` if it already was (the close is under way).
    pub(crate) fn begin_closing(&self, window: &str) -> bool {
        self.closing().insert(window.to_owned())
    }

    /// Whether `window` is being closed by Drift (its `CloseRequested` may proceed).
    pub(crate) fn is_closing(&self, window: &str) -> bool {
        self.closing().contains(window)
    }

    /// The window is gone.
    pub(crate) fn finish_closing(&self, window: &str) {
        self.closing().remove(window);
    }

    /// Starts the quit sequence; `false` if it is already running.
    pub(crate) fn begin_quit(&self) -> bool {
        !self.quitting.swap(true, Ordering::SeqCst)
    }
}

/// Static information about the running app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// Product name.
    pub name: String,
    /// Crate version.
    pub version: String,
}

/// Returns the app name and version.
#[tauri::command]
#[specta::specta]
pub fn app_info() -> AppInfo {
    AppInfo { name: "Drift".into(), version: env!("CARGO_PKG_VERSION").into() }
}

/// Lists saved profiles (sorted by name) with which passwords are stored.
#[tauri::command]
#[specta::specta]
pub fn list_profiles(state: State<'_, AppState>) -> Result<Vec<ProfileEntry>, CommandError> {
    state.profiles.list()
}

/// A new, unsaved profile for `mode` with a fresh id and default settings.
#[tauri::command]
#[specta::specta]
pub fn new_profile(mode: ConnectMode) -> ConnectionProfile {
    ConnectionProfile::new("", "", mode)
}

/// Validates a profile without saving it; an empty list means valid.
#[tauri::command]
#[specta::specta]
pub fn validate_profile(profile: ConnectionProfile) -> Vec<ProfileIssue> {
    drift_core::profile::validate(&profile).err().unwrap_or_default()
}

/// Saves (inserts or updates) a profile and applies password changes.
#[tauri::command]
#[specta::specta]
pub fn save_profile(
    state: State<'_, AppState>,
    profile: ConnectionProfile,
    secrets: SecretsUpdate,
) -> Result<ProfileEntry, CommandError> {
    state.profiles.save(profile, secrets)
}

/// Deletes a profile, its stored passwords and its window frame; an open profile's window is
/// closed first (without asking).
#[tauri::command]
#[specta::specta]
pub fn delete_profile(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<(), CommandError> {
    if let Some(window) = state.sessions.window_for(id).or_else(|| state.window_of_profile(id)) {
        crate::windows::close_session_window(&app, &window);
    }
    state.profiles.delete(id)?;
    state.frames.remove(&profile_key(id))
}

/// Clears a profile's pinned certificate (after a legitimate certificate change).
#[tauri::command]
#[specta::specta]
pub fn forget_certificate(state: State<'_, AppState>, id: Uuid) -> Result<ProfileEntry, CommandError> {
    state.profiles.set_pin(id, None)?;
    state.profiles.get(id)
}

/// Explains a disconnect reason for a mode (title, message, next steps, actions).
#[tauri::command]
#[specta::specta]
pub fn explain(reason: DisconnectReason, mode: ConnectMode) -> ErrorExplanation {
    explain_disconnect(&reason, mode)
}

/// Opens System Settings › Privacy & Security › Local Network (errno 65 screen).
#[tauri::command]
#[specta::specta]
pub fn open_local_network_settings() -> Result<(), CommandError> {
    crate::platform::open_url(drift_core::messages::LOCAL_NETWORK_SETTINGS_URL)
}

/// Connects `profile_id`: opens a session window for it, or brings its window forward if it
/// already has one (from any window).
#[tauri::command]
#[specta::specta]
pub fn connect(app: tauri::AppHandle, profile_id: Uuid) -> Result<(), CommandError> {
    crate::windows::connect_profile(&app, profile_id)
}

/// Trusts the prompted certificate (`pin` = remember it for this profile).
#[tauri::command]
#[specta::specta]
pub fn accept_certificate(
    state: State<'_, AppState>,
    window: tauri::Window,
    fingerprint: CertFingerprint,
    pin: bool,
) -> Result<(), CommandError> {
    state.sessions.accept_certificate(window.label(), fingerprint, pin)
}

/// Rejects the prompted certificate (the session fails with `CertMismatch`).
#[tauri::command]
#[specta::specta]
pub fn reject_certificate(state: State<'_, AppState>, window: tauri::Window) -> Result<(), CommandError> {
    state.sessions.reject_certificate(window.label())
}

/// Skips the backoff delay and reconnects now ("Now" button, error screen "Reconnect").
#[tauri::command]
#[specta::specta]
pub fn reconnect_now(app: tauri::AppHandle, window: tauri::Window) -> Result<(), CommandError> {
    crate::windows::reconnect(&app, window.label())
}

/// Stops reconnecting; the session stays disconnected ("Cancel" button).
#[tauri::command]
#[specta::specta]
pub fn cancel_reconnect(state: State<'_, AppState>, window: tauri::Window) -> Result<(), CommandError> {
    state.sessions.send(window.label(), drift_rdp::SessionCommand::Cancel)
}

/// Closes the calling session window without asking (the error sheet's Close, Cancel while
/// connecting): the session closes gracefully, then the window.
#[tauri::command]
#[specta::specta]
pub fn close_session(app: tauri::AppHandle, window: tauri::Window) -> Result<(), CommandError> {
    crate::windows::close_session_window(&app, window.label());
    Ok(())
}

// ---- Connections gallery and session windows (UI-windows) ------------------------------------

/// Copies a profile and its stored passwords under a new id, named "<name> copy", without the
/// certificate pin.
#[tauri::command]
#[specta::specta]
pub fn duplicate_profile(state: State<'_, AppState>, id: Uuid) -> Result<ProfileEntry, CommandError> {
    state.profiles.duplicate(id)
}

/// The profiles that have a session window (pulled by the Connections page on load).
#[tauri::command]
#[specta::specta]
pub fn connections(state: State<'_, AppState>) -> Result<Connections, CommandError> {
    Ok(state.sessions.connections())
}

/// Brings `profile_id`'s session window forward; `NotFound` if it has none.
#[tauri::command]
#[specta::specta]
pub fn show_window(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    profile_id: Uuid,
) -> Result<(), CommandError> {
    let window = state.sessions.window_for(profile_id).ok_or(CommandError::NotFound)?;
    crate::windows::focus_window(&app, &window);
    Ok(())
}

/// Closes `profile_id`'s session window without asking (the card's Disconnect); `NotFound` if
/// it has none.
#[tauri::command]
#[specta::specta]
pub fn disconnect_profile(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    profile_id: Uuid,
) -> Result<(), CommandError> {
    let window = state.sessions.window_for(profile_id).ok_or(CommandError::NotFound)?;
    crate::windows::close_session_window(&app, &window);
    Ok(())
}

/// Shows the Connections window and makes it key; with `edit`, opens that profile's edit sheet
/// (the title bar's grid button, the error sheet's Edit Connection…).
#[tauri::command]
#[specta::specta]
pub fn show_connections(app: tauri::AppHandle, edit: Option<Uuid>) -> Result<(), CommandError> {
    crate::windows::show_connections(&app, edit.map(|profile_id| ConnectionsIntent::Edit { profile_id }));
    Ok(())
}

/// Shows the Connections window with the New Connection sheet.
#[tauri::command]
#[specta::specta]
pub fn new_connection(app: tauri::AppHandle) -> Result<(), CommandError> {
    crate::windows::show_connections(&app, Some(ConnectionsIntent::New));
    Ok(())
}

/// The calling session window's identity (pulled by its title bar and its page on load).
#[tauri::command]
#[specta::specta]
pub fn window_identity(app: tauri::AppHandle, window: tauri::Window) -> Result<WindowIdentity, CommandError> {
    crate::windows::window_identity(&app, window.label()).ok_or(CommandError::NoSession)
}

/// Turns the calling session window's statistics HUD on or off (the title bar's gauge).
#[tauri::command]
#[specta::specta]
pub fn toggle_stats(state: State<'_, AppState>, window: tauri::Window) -> Result<(), CommandError> {
    state.sessions.toggle_stats(window.label())
}

/// Gives the keyboard back to the calling window's page or live picture (the title bar never
/// keeps it).
#[tauri::command]
#[specta::specta]
pub fn focus_content(app: tauri::AppHandle, window: tauri::Window) -> Result<(), CommandError> {
    crate::windows::focus_content(&app, window.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_info_reports_version() {
        let info = app_info();
        assert_eq!(info.name, "Drift");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn new_profile_uses_mode_defaults() {
        let a = new_profile(ConnectMode::RemoteLogin);
        let b = new_profile(ConnectMode::RemoteLogin);
        assert_ne!(a.id, b.id);
        assert_eq!(a.port, 3389);
        assert_eq!(a.mode, ConnectMode::RemoteLogin);
        assert!(a.name.is_empty() && a.host.is_empty());
    }

    #[test]
    fn validate_profile_lists_issues() {
        let issues = validate_profile(new_profile(ConnectMode::Headless));
        let fields: Vec<_> = issues.iter().map(|i| i.field).collect();
        use drift_core::ProfileField as F;
        assert_eq!(fields, [F::Name, F::Host, F::RdpUsername]);
        let mut ok = new_profile(ConnectMode::Headless);
        ok.name = "n".into();
        ok.host = "h".into();
        ok.rdp_username = "u".into();
        assert!(validate_profile(ok).is_empty());
    }

    #[test]
    fn explain_forwards_to_core() {
        let e = explain(DisconnectReason::LocalNetworkDenied, ConnectMode::Headless);
        assert!(e.next_steps[0].contains("Local Network"));
    }

    #[test]
    fn no_session_error_is_user_readable() {
        assert_eq!(CommandError::NoSession.to_string(), "this window has no active session");
    }
}
