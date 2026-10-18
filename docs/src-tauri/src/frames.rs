//! Window frames remembered per profile (task **UI-windows**, ADR UI-windows-gallery
//! decision 7).
//!
//! `window-frames.json` sits next to `profiles.toml` and maps a key — a profile id, or
//! [`CONNECTIONS_KEY`] for the Connections window — to that window's last frame in screen
//! points (top-left origin, as Tauri reports window positions). Only geometry is written.
//! [`restore`] decides whether a saved frame is still usable on the current screens.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::profiles::CommandError;

/// File name of the frame store inside the app config directory.
pub const FRAMES_FILE: &str = "window-frames.json";

/// Key of the Connections window's frame.
pub const CONNECTIONS_KEY: &str = "connections";

/// How much of a saved frame (in points, each way) must be on one screen for it to be kept.
pub const MIN_VISIBLE: f64 = 64.0;

/// Offset between default frames of successive session windows.
pub const CASCADE: f64 = 24.0;

/// A window frame in screen points, top-left origin.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl Frame {
    /// Width and height of the overlap with `other` (0 when they do not overlap).
    fn overlap(&self, other: &Self) -> (f64, f64) {
        let w = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let h = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        (w.max(0.0), h.max(0.0))
    }
}

/// A window template's size limits (from `tauri.conf.json`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Template {
    /// Default width.
    pub width: f64,
    /// Default height.
    pub height: f64,
    /// Minimum width.
    pub min_width: f64,
    /// Minimum height.
    pub min_height: f64,
}

/// The key of profile `id`'s session window frame.
pub fn profile_key(id: Uuid) -> String {
    id.to_string()
}

/// The frame for a new window: `saved` if at least [`MIN_VISIBLE`]×[`MIN_VISIBLE`] points of
/// it are on one of `screens` and it is at least the template's minimum size; otherwise the
/// template size centred on the first (main) screen, moved down and right by [`CASCADE`] per
/// already open session window (`cascade`).
pub fn restore(saved: Option<Frame>, screens: &[Frame], template: Template, cascade: usize) -> Frame {
    if let Some(saved) = saved {
        let big_enough = saved.width >= template.min_width && saved.height >= template.min_height;
        let visible = screens.iter().any(|screen| {
            let (w, h) = saved.overlap(screen);
            w >= MIN_VISIBLE && h >= MIN_VISIBLE
        });
        if big_enough && visible {
            return saved;
        }
    }
    #[expect(clippy::cast_precision_loss, reason = "a handful of windows")]
    let offset = CASCADE * cascade as f64;
    let (x, y) = screens.first().map_or((0.0, 0.0), |s| {
        (s.x + (s.width - template.width) / 2.0, s.y + (s.height - template.height) / 2.0)
    });
    Frame { x: x + offset, y: y + offset, width: template.width, height: template.height }
}

/// The frames on disk, cached in memory. A missing or unreadable file is an empty store.
#[derive(Debug)]
pub struct FrameStore {
    path: PathBuf,
    frames: Mutex<BTreeMap<String, Frame>>,
}

impl FrameStore {
    /// The store at `path`.
    pub fn open(path: PathBuf) -> Self {
        let frames = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<BTreeMap<String, Frame>>(&text).ok())
            .unwrap_or_default();
        Self { path, frames: Mutex::new(frames) }
    }

    /// The store in config directory `dir` ([`FRAMES_FILE`]).
    pub fn in_dir(dir: &Path) -> Self {
        Self::open(dir.join(FRAMES_FILE))
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Frame>> {
        self.frames.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The saved frame for `key`.
    pub fn get(&self, key: &str) -> Option<Frame> {
        self.lock().get(key).copied()
    }

    /// Saves `frame` for `key` and writes the file.
    pub fn set(&self, key: &str, frame: Frame) -> Result<(), CommandError> {
        let mut frames = self.lock();
        if frames.get(key) == Some(&frame) {
            return Ok(());
        }
        frames.insert(key.to_owned(), frame);
        self.write(&frames)
    }

    /// Forgets `key`'s frame (profile deleted) and writes the file.
    pub fn remove(&self, key: &str) -> Result<(), CommandError> {
        let mut frames = self.lock();
        if frames.remove(key).is_none() {
            return Ok(());
        }
        self.write(&frames)
    }

    fn write(&self, frames: &BTreeMap<String, Frame>) -> Result<(), CommandError> {
        let storage = |e: &dyn std::fmt::Display| CommandError::Storage { message: e.to_string() };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| storage(&e))?;
        }
        let text = serde_json::to_string_pretty(frames).map_err(|e| storage(&e))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| storage(&e))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| storage(&e))
    }
}
