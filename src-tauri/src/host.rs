//! The production [`SessionHost`]: starts real session actors and applies their output to the
//! window (tasks **M6-1**, **M1-6**, **M3-2**).

use std::sync::Arc;

use drift_core::SystemClock;
use drift_rdp::{CursorUpdate, SessionEvent, SessionEvents, SessionHandle, SessionOptions, spawn_session};
use tauri::{AppHandle, Runtime};
use uuid::Uuid;

use crate::manager::SessionHost;
use crate::present;
use crate::profiles::{CommandError, ProfileService};
use crate::view::SessionView;
use crate::windows;

/// Wires the `SessionManager` to Tauri, AppKit and the profile store.
pub struct TauriHost<R: Runtime> {
    app: AppHandle<R>,
    profiles: Arc<ProfileService>,
}

impl<R: Runtime> TauriHost<R> {
    /// A host for `app`, reading secrets and pins from `profiles`.
    pub fn new(app: AppHandle<R>, profiles: Arc<ProfileService>) -> Self {
        Self { app, profiles }
    }
}

impl<R: Runtime> SessionHost for TauriHost<R> {
    fn spawn(
        &self,
        window: &str,
        profile: &drift_core::ConnectionProfile,
    ) -> Result<(SessionHandle, SessionEvents), CommandError> {
        let secrets = self.profiles.session_secrets(profile.id)?;
        let sink = windows::create_render_sink(&self.app, window)?;
        let runtime = tauri::async_runtime::handle();
        let _guard = runtime.inner().enter();
        let (handle, events) =
            spawn_session(profile.clone(), secrets, sink, Arc::new(SystemClock), SessionOptions::default());
        windows::attach_session(&self.app, window, handle.clone());
        Ok((handle, events))
    }

    fn view_changed(&self, window: &str, view: &SessionView) {
        windows::apply_view(&self.app, window, view);
    }

    fn session_event(&self, window: &str, event: &SessionEvent) {
        match event {
            SessionEvent::Cursor(update) => {
                if let Some((shape, scale)) = present::cursor_shape(update) {
                    windows::apply_cursor(&self.app, window, shape, scale);
                }
                if let CursorUpdate::Position(_) = update {
                    // The server's pointer position never warps the Mac pointer (plan M2-5).
                }
            }
            SessionEvent::ClipboardRemote(contents) => {
                windows::write_pasteboard(&self.app, window, contents.clone());
            }
            SessionEvent::Capabilities(caps) => {
                tracing::debug!(%window, display_control = caps.display_control, clipboard = caps.clipboard);
            }
            // The sample is also folded into the window's `SessionView` (M1 statistics HUD);
            // this is only the log line the e2e smoke reads.
            SessionEvent::Stats(stats) => {
                tracing::trace!(%window, fps = stats.fps, unacked = stats.unacked_frames);
            }
            SessionEvent::State(_)
            | SessionEvent::CertificatePrompt { .. }
            | SessionEvent::CertificatePinned(_) => {}
        }
    }

    fn certificate_pinned(&self, profile: Uuid, fingerprint: drift_core::CertFingerprint) {
        if let Err(e) = self.profiles.set_pin(profile, Some(fingerprint)) {
            tracing::error!(error = %e, "could not store the certificate pin");
        }
    }

    fn session_ended(&self, window: &str) {
        windows::release_session(&self.app, window);
    }
}
