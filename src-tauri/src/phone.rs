//! iPhone layout and the soft key row (task **IOS-8**; ADR `IOS-8-iphone-layout`).
//!
//! Pure decisions for `windows_ios.rs` and the session page:
//!
//! * [`phone_layout`]: portrait puts the picture at the top, full width at the desktop's aspect
//!   (at most 60 % of the usable height), the on-screen trackpad below it and the key row at the
//!   bottom; landscape gives the picture the whole screen and floats the key row over its bottom
//!   edge while the controls are shown.
//! * [`SoftKey`], [`chord_events`], [`named_chord_events`], [`text_events`]: what the key row
//!   and the iOS keyboard send.
//! * [`AutoHide`]: landscape controls disappear after [`AUTO_HIDE_SECS`] without use.

use drift_core::{DesktopSize, InputEvent, Size};
use drift_input::Scancode;
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

/// Largest share of the usable portrait height the picture may take.
const MAX_PICTURE_SHARE: f64 = 0.6;

fn finite(v: f64) -> f64 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

/// Lays out a phone session screen of `container` points with `safe` insets. `controls`: the
/// landscape key row is shown (portrait always shows it).
pub fn phone_layout(
    container: Size<f64>,
    safe: Insets,
    desktop: Option<DesktopSize>,
    controls: bool,
) -> PhoneLayout {
    let (w, h) = (finite(container.width), finite(container.height));
    let full = Frame { x: 0.0, y: 0.0, width: w, height: h };
    if w > h {
        let keys = controls.then(|| Frame {
            x: safe.left,
            y: finite(h - safe.bottom - KEY_ROW_HEIGHT),
            width: finite(w - safe.left - safe.right),
            height: KEY_ROW_HEIGHT,
        });
        return PhoneLayout { picture: full, pad: None, keys };
    }
    let aspect = match desktop {
        Some(d) if d.width > 0 && d.height > 0 => f64::from(d.height) / f64::from(d.width),
        _ => 10.0 / 16.0,
    };
    let usable = finite(h - safe.top - safe.bottom);
    let picture_h = (w * aspect).min(usable * MAX_PICTURE_SHARE);
    let picture = Frame { x: 0.0, y: safe.top, width: w, height: picture_h };
    let keys =
        Frame { x: 0.0, y: finite(h - safe.bottom - KEY_ROW_HEIGHT), width: w, height: KEY_ROW_HEIGHT };
    let pad_top = safe.top + picture_h;
    let pad = Frame { x: 0.0, y: pad_top, width: w, height: finite(keys.y - pad_top) };
    PhoneLayout { picture, pad: Some(pad), keys: Some(keys) }
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
    /// Space (the Mac key row's ⌘Space).
    Space,
}

impl SoftKey {
    /// The key's set-1 scancode.
    pub const fn scancode(self) -> Scancode {
        match self {
            Self::Escape => Scancode::new(0x01),
            Self::Tab => Scancode::new(0x0F),
            Self::Control => Scancode::new(0x1D),
            Self::Alt => Scancode::new(0x38),
            Self::Super => Scancode::ext(0x5B),
            Self::Shift => Scancode::new(0x2A),
            Self::Up => Scancode::ext(0x48),
            Self::Down => Scancode::ext(0x50),
            Self::Left => Scancode::ext(0x4B),
            Self::Right => Scancode::ext(0x4D),
            Self::Backspace => Scancode::new(0x0E),
            Self::Enter => Scancode::new(0x1C),
            Self::Delete => Scancode::ext(0x53),
            Self::Space => Scancode::new(0x39),
        }
    }

    /// The key named `name` in the page's kebab-case spelling (`"page-up"` style).
    pub fn from_name(name: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
    }
}

fn press_release(codes: &[Scancode]) -> Vec<InputEvent> {
    let mut out: Vec<InputEvent> = codes.iter().map(|c| c.event(true)).collect();
    out.extend(codes.iter().rev().map(|c| c.event(false)));
    out
}

/// Presses `keys` in order and releases them in reverse (a chord such as Ctrl+Alt+Delete).
pub fn chord_events(keys: &[SoftKey]) -> Vec<InputEvent> {
    let codes: Vec<Scancode> = keys.iter().map(|k| k.scancode()).collect();
    press_release(&codes)
}

/// A chord from the page: [`SoftKey`] names, plus single letters and digits (US key positions)
/// for chords typed with a latched modifier, such as `["control", "c"]`. `None` if any name is
/// unknown, so a bad request sends nothing.
pub fn named_chord_events(names: &[String]) -> Option<Vec<InputEvent>> {
    let codes = names
        .iter()
        .map(|n| SoftKey::from_name(n).map(SoftKey::scancode).or_else(|| letter_or_digit(n)))
        .collect::<Option<Vec<_>>>()?;
    Some(press_release(&codes))
}

/// US-position scancode of a single ASCII letter or digit.
fn letter_or_digit(name: &str) -> Option<Scancode> {
    let mut chars = name.chars();
    let c = chars.next()?.to_ascii_lowercase();
    if chars.next().is_some() {
        return None;
    }
    // HID usages: a = 0x04 …, 1 = 0x1E … 0 = 0x27; the HID table gives the set-1 code.
    let usage = match c {
        'a'..='z' => 0x04 + u16::from(c as u8 - b'a'),
        '1'..='9' => 0x1E + u16::from(c as u8 - b'1'),
        '0' => 0x27,
        _ => return None,
    };
    drift_input::scancode_for_hid(usage, drift_core::CmdAs::Super)
}

/// Typed text from the iOS keyboard: Unicode key events, one press and release per UTF-16
/// unit (the remote layout does not matter), and a newline as Return.
pub fn text_events(text: &str) -> Vec<InputEvent> {
    let mut out = Vec::new();
    for ch in text.chars() {
        if ch == '\n' || ch == '\r' {
            out.extend(chord_events(&[SoftKey::Enter]));
            continue;
        }
        let mut units = [0_u16; 2];
        for unit in ch.encode_utf16(&mut units) {
            out.push(InputEvent::Unicode { ch: *unit, down: true });
            out.push(InputEvent::Unicode { ch: *unit, down: false });
        }
    }
    out
}

/// What the session page shows of the iPhone controls (the `PhoneControlsChanged` event).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct PhoneControls {
    /// The key row is on screen (the page is laid out as its strip).
    pub visible: bool,
    /// The session's touch mode (resolved: `trackpad` or `direct`).
    pub touch: drift_core::TouchMode,
}

/// Emitted to an iPhone session window whenever its controls change. Never sent on the Mac.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct PhoneControlsChanged(pub PhoneControls);

/// Landscape controls hide after this long without use (seconds).
pub const AUTO_HIDE_SECS: f64 = 4.0;

/// Auto-hide state of the landscape controls.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AutoHide {
    last_use: Option<f64>,
}

impl AutoHide {
    /// The controls were shown or used at `now` (seconds on a monotonic clock).
    pub fn touch(&mut self, now: f64) {
        self.last_use = Some(now);
    }

    /// Whether they are visible at `now`.
    pub fn visible(&self, now: f64) -> bool {
        self.last_use.is_some_and(|t| now - t < AUTO_HIDE_SECS)
    }

    /// Hides them now.
    pub fn hide(&mut self) {
        self.last_use = None;
    }
}
