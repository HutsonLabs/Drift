//! iPhone layout and the soft key row (task **IOS-8**). Stub (Red).

use drift_core::{DesktopSize, InputEvent, Size};
use serde::{Deserialize, Serialize};

/// A rectangle in points, top-left origin.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Frame {
    /// Left.
    pub x: f64,
    /// Top.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

/// Safe-area insets in points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Insets {
    /// Top.
    pub top: f64,
    /// Left.
    pub left: f64,
    /// Bottom.
    pub bottom: f64,
    /// Right.
    pub right: f64,
}

/// Where the picture, the trackpad and the key row go.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PhoneLayout {
    /// The live picture.
    pub picture: Frame,
    /// The on-screen trackpad (portrait only).
    pub pad: Option<Frame>,
    /// The key row (the page), when shown.
    pub keys: Option<Frame>,
}

/// Height of the key row in points (without the safe-area inset below it).
pub const KEY_ROW_HEIGHT: f64 = 56.0;

/// Lays out a phone session screen.
pub fn phone_layout(
    _container: Size<f64>,
    _safe: Insets,
    _desktop: Option<DesktopSize>,
    _controls: bool,
) -> PhoneLayout {
    PhoneLayout::default()
}

/// A key on the soft key row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "kebab-case")]
pub enum SoftKey {
    /// Esc.
    Escape,
    /// Tab.
    Tab,
    /// Left Control.
    Control,
    /// Left Alt / Option.
    Alt,
    /// Left Super (Windows).
    Super,
    /// Left Shift.
    Shift,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Backspace.
    Backspace,
    /// Return.
    Enter,
    /// Forward delete.
    Delete,
}

/// Presses `keys` in order and releases them in reverse (a chord such as Ctrl+Alt+Delete).
pub fn chord_events(_keys: &[SoftKey]) -> Vec<InputEvent> {
    Vec::new()
}

/// Typed text from the iOS keyboard as key events.
pub fn text_events(_text: &str) -> Vec<InputEvent> {
    Vec::new()
}

/// Landscape controls hide after this long without use (seconds).
pub const AUTO_HIDE_SECS: f64 = 4.0;

/// Auto-hide state of the landscape controls.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AutoHide {
    _shown_at: Option<f64>,
}

impl AutoHide {
    /// The controls were shown or used at `now`.
    pub fn touch(&mut self, _now: f64) {}
    /// Whether they are visible at `now`.
    pub fn visible(&self, _now: f64) -> bool {
        false
    }
    /// Hides them now.
    pub fn hide(&mut self) {}
}
