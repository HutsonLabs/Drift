//! The Connections window's model and the session windows' identity (task **UI-windows**,
//! `docs/adr/UI-windows-gallery.md`).
//!
//! Types in this module are the IPC contract between Rust and the Connections page
//! (`ConnectionsChanged`, `ThumbnailUpdated`, `ConnectionsIntentRequested`) and the session
//! windows' title-bar webview (`WindowIdentityChanged`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use drift_core::{ConnectMode, SessionState, Size};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::view::SessionView;

/// Where a profile's connection stands (gallery pills, Dock menu, Window menu, identity capsule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectionStatus {
    /// No session window.
    Idle,
    /// Connecting, or waiting for a certificate decision (spinner).
    Connecting,
    /// The picture (or the GNOME login screen) is live (green).
    Live,
    /// Waiting out a reconnect backoff (amber).
    Reconnecting,
    /// Failed or disconnected with an explanation (red).
    Failed,
}

/// One profile that has a session window (the gallery's "Open" section).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct OpenConnection {
    /// The profile.
    pub profile_id: Uuid,
    /// Its window's label (`session-<n>`).
    pub window: String,
    /// Where the connection stands.
    pub status: ConnectionStatus,
    /// Uptime in seconds at emit time while live (the UI ticks locally).
    pub live_secs: Option<u32>,
    /// Seconds until the next attempt while reconnecting.
    pub reconnect_in_secs: Option<u32>,
    /// The reconnect attempt while reconnecting.
    pub attempt: Option<u32>,
}

/// Everything the Connections window needs besides the profile list.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
pub struct Connections {
    /// Profiles with a session window, in window opening order.
    pub open: Vec<OpenConnection>,
}

/// A session window's title bar: the identity capsule and the buttons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct WindowIdentity {
    /// The window's profile.
    pub profile_id: Uuid,
    /// Display name.
    pub name: String,
    /// Host as saved in the profile.
    pub host: String,
    /// Connection mode (picks the glyph).
    pub mode: ConnectMode,
    /// Dot or spinner.
    pub status: ConnectionStatus,
    /// Greeter hint (tooltip) while the GNOME login screen waits.
    pub hint: Option<String>,
    /// The statistics HUD is on (gauge pressed).
    pub show_stats: bool,
}

/// A live session's preview for its gallery card, as a `data:image/png;base64,…` URL.
/// `image: None` drops the preview (the session ended). Never logged, never written to disk.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Thumbnail {
    /// The profile whose card shows it.
    pub profile_id: Uuid,
    /// The PNG as a data URL, or `None`.
    pub image: Option<String>,
}

impl std::fmt::Debug for Thumbnail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Thumbnail")
            .field("profile_id", &self.profile_id)
            .field("image", &self.image.as_ref().map(|i| format!("<{} bytes>", i.len())))
            .finish()
    }
}

/// What the Connections page should open when Rust brings it forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ConnectionsIntent {
    /// The New Connection sheet (Cmd+N, Dock ▸ New Connection…).
    New,
    /// The edit sheet of a profile (Cmd+E, the error sheet's Edit Connection…).
    Edit {
        /// The profile to edit.
        profile_id: Uuid,
    },
}

/// Emitted to the `connections` page after any status change, window open or close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct ConnectionsChanged(pub Connections);

/// Emitted to the `connections` page only, while it is visible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct ThumbnailUpdated(pub Thumbnail);

/// Emitted to the `connections` page to open a sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct ConnectionsIntentRequested(pub ConnectionsIntent);

/// Emitted to a session window's `<label>-titlebar` webview (identical pushes are skipped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct WindowIdentityChanged(pub WindowIdentity);

// ---- the Connections window and its live thumbnails ------------------------------------------

/// Label (and template) of the one Connections window.
pub const CONNECTIONS_WINDOW: &str = "connections";

/// How often a live session's thumbnail is refreshed while Connections is visible.
pub const THUMBNAIL_INTERVAL: Duration = Duration::from_secs(15);

/// Thumbnails fit this box (the card preview is 16:9), in pixels.
pub const THUMBNAIL_MAX: Size<u32> = Size { width: 320, height: 180 };

/// Decides when to sample which live session (ADR decision 8): nothing while the Connections
/// window is hidden, at most once per session per [`THUMBNAIL_INTERVAL`], and immediately when
/// the window is shown or a session first goes live. Starts hidden.
#[derive(Debug, Clone, Default)]
pub struct ThumbnailThrottle {
    visible: bool,
    last: HashMap<String, Instant>,
}

impl ThumbnailThrottle {
    /// The Connections window became visible (`true`) or hidden (`false`). Showing it makes
    /// every live session due at once.
    pub fn set_visible(&mut self, visible: bool) {
        if visible && !self.visible {
            self.last.clear();
        }
        self.visible = visible;
    }

    /// Whether the Connections window is visible.
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Session window `window` just went live: sample it at the next tick.
    pub fn went_live(&mut self, window: &str) {
        self.last.remove(window);
    }

    /// Session window `window` ended or closed.
    pub fn ended(&mut self, window: &str) {
        self.last.remove(window);
    }

    /// The windows among `live` to sample now (in `live`'s order); records them as sampled.
    pub fn due<S: AsRef<str>>(&mut self, now: Instant, live: &[S]) -> Vec<String> {
        if !self.visible {
            return Vec::new();
        }
        let mut due = Vec::new();
        for window in live.iter().map(AsRef::as_ref) {
            let ready =
                self.last.get(window).is_none_or(|t| now.saturating_duration_since(*t) >= THUMBNAIL_INTERVAL);
            if ready {
                self.last.insert(window.to_owned(), now);
                due.push(window.to_owned());
            }
        }
        due
    }
}

/// The largest size with `size`'s aspect ratio that fits `max` (never upscaled, never 0 wide or
/// high for a non-empty input).
pub fn fit(size: Size<u32>, max: Size<u32>) -> Size<u32> {
    if size.width == 0 || size.height == 0 {
        return Size::new(0, 0);
    }
    let scale = (f64::from(max.width) / f64::from(size.width))
        .min(f64::from(max.height) / f64::from(size.height))
        .min(1.0);
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "0 < value <= max")]
    let scaled = |v: u32, max: u32| ((f64::from(v) * scale).round() as u32).clamp(1, max.max(1));
    Size::new(scaled(size.width, max.width), scaled(size.height, max.height))
}

/// Box-filters a BGRA frame (`size`, `width * height * 4` bytes) down to [`fit`]`(size, max)`
/// and returns it as opaque RGBA (the desktop's alpha is meaningless). A frame shorter than its
/// size claims yields an empty image.
pub fn downscale(bgra: &[u8], size: Size<u32>, max: Size<u32>) -> (Vec<u8>, Size<u32>) {
    let (w, h) = (size.width as usize, size.height as usize);
    if w == 0 || h == 0 || bgra.len() < w * h * 4 {
        return (Vec::new(), Size::new(0, 0));
    }
    let out = fit(size, max);
    let (ow, oh) = (out.width as usize, out.height as usize);
    let mut rgba = Vec::with_capacity(ow * oh * 4);
    for oy in 0..oh {
        let (y0, y1) = (oy * h / oh, ((oy + 1) * h / oh).max(oy * h / oh + 1));
        for ox in 0..ow {
            let (x0, x1) = (ox * w / ow, ((ox + 1) * w / ow).max(ox * w / ow + 1));
            let mut sum = [0u64; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = (y * w + x) * 4;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += u64::from(bgra[p + c]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            #[expect(clippy::cast_possible_truncation, reason = "an average of bytes is a byte")]
            let avg = |s: u64| ((s + n / 2) / n) as u8;
            rgba.extend([avg(sum[2]), avg(sum[1]), avg(sum[0]), 255]);
        }
    }
    (rgba, out)
}

/// Standard base64 (RFC 4648, with padding).
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Encodes opaque RGBA pixels as a `data:image/png;base64,…` URL; `None` if the buffer does not
/// match `size` or encoding fails.
pub fn png_data_url(rgba: &[u8], size: Size<u32>) -> Option<String> {
    if size.width == 0 || size.height == 0 || rgba.len() != size.width as usize * size.height as usize * 4 {
        return None;
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, size.width, size.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(format!("data:image/png;base64,{}", base64(&png)))
}

/// The gallery's entry for session window `window` of `profile_id` showing `view`;
/// `live_for` is how long the picture has been live.
pub fn open_connection(
    window: &str,
    profile_id: Uuid,
    view: &SessionView,
    live_for: Option<Duration>,
) -> OpenConnection {
    let status = crate::present::status_for(view);
    let secs = |d: Duration| u32::try_from(d.as_secs()).unwrap_or(u32::MAX);
    let (reconnect_in_secs, attempt) = match &view.state {
        SessionState::Reconnecting { attempt, next_in, .. } => (Some(secs(*next_in)), Some(*attempt)),
        _ => (None, None),
    };
    OpenConnection {
        profile_id,
        window: window.to_owned(),
        status,
        live_secs: (status == ConnectionStatus::Live).then(|| live_for.map_or(0, secs)),
        reconnect_in_secs,
        attempt,
    }
}
