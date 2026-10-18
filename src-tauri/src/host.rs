//! The production [`SessionHost`]: starts real session actors and applies their output to the
//! window (tasks **M6-1**, **M1-6**, **M3-2**).

use std::sync::Arc;

use drift_core::{ConnectionProfile, SystemClock};
use drift_gfx::GfxCaps;
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
    pairing: crate::pairing::PairingService,
}

impl<R: Runtime> TauriHost<R> {
    /// A host for `app`, reading secrets and pins from `profiles` and paired Macs' handshakes
    /// from `pairing`.
    pub fn new(
        app: AppHandle<R>,
        profiles: Arc<ProfileService>,
        pairing: crate::pairing::PairingService,
    ) -> Self {
        Self { app, profiles, pairing }
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
        let (handle, events) = spawn_session(
            profile.clone(),
            secrets,
            sink,
            Arc::new(SystemClock),
            self.session_options(profile)?,
        );
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

impl<R: Runtime> TauriHost<R> {
    /// [`session_options`] with this profile's pairing or authentication exchange (HOST-8).
    fn session_options(
        &self,
        profile: &drift_core::ConnectionProfile,
    ) -> Result<SessionOptions, CommandError> {
        Ok(session_options(profile, self.pairing.client_pairing(profile.id)?))
    }
}

/// Every session offers the `Drift::Mac::v1` channel (EXT-1): a Drift Host opens it, every
/// other server leaves it closed and the session is plain RDP. Only a paired Drift Host gets
/// the V10 graphics caps that allow AVC444; GNOME keeps the verified V8.1 list, because V10
/// makes g-r-d send AVC444v2 (ADR `AVC444-client`). Every session also plays the server's
/// sound (HOST-11), through its own `AVAudioEngine` player. A paired Mac also gets its pairing or
/// authentication exchange (HOST-8).
pub(crate) fn session_options(
    profile: &ConnectionProfile,
    pairing: Option<drift_ext::pairing::ClientPairing>,
) -> SessionOptions {
    let gfx_caps = if profile.paired_host.is_some() { GfxCaps::DriftHost } else { GfxCaps::Gnome };
    SessionOptions {
        drift_mac: Some(drift_ext::channel::Link::new()),
        gfx_caps,
        pairing,
        audio: Some(drift_rdp::audio::AudioOutput::new(SessionSound(drift_apple::audio_out::Player::new()))),
        ..SessionOptions::default()
    }
}

/// A session's sound output: the `drift-apple` player behind `drift-rdp`'s sink trait.
struct SessionSound(drift_apple::audio_out::Player);

impl drift_rdp::audio::AudioSink for SessionSound {
    fn start(&self, format: drift_core::audio::PcmFormat) {
        if let Err(e) = self.0.start(format) {
            tracing::warn!(error = %e, "no sound for this session");
        }
    }

    fn play(&self, pcm: &[u8]) {
        self.0.play(pcm);
    }

    fn stop(&self) {
        self.0.stop();
    }
}

#[cfg(test)]
mod tests {
    use drift_core::ConnectMode;

    use super::*;

    #[test]
    fn only_paired_drift_hosts_get_the_v10_caps() {
        let mut profile = ConnectionProfile::new("Studio", "studio.local", ConnectMode::Headless);
        assert_eq!(session_options(&profile, None).gfx_caps, GfxCaps::Gnome);
        profile.paired_host = Some(Uuid::new_v4());
        assert_eq!(session_options(&profile, None).gfx_caps, GfxCaps::DriftHost);
        assert!(session_options(&profile, None).drift_mac.is_some());
        assert!(session_options(&profile, None).audio.is_some(), "every session plays sound (HOST-11)");
    }
}
