//! RDP fast-path input → the macOS events Drift Host injects (task **HOST-6**). Pure.
//!
//! An [`Injector`] holds what a sequence of input events needs: which modifiers and buttons are
//! held, where the pointer is, and the last click for double-click counting. It turns each
//! [`FastPathInputEvent`] into zero or more [`InjectEvent`]s, which [`crate::cg`] posts.
//!
//! * **Keys**: set-1 scancodes go back to `kVK_*` codes through
//!   [`drift_input::keymap::kvk_for`] (one table for both directions). Every key event carries
//!   the modifiers held at that moment, because an injected modifier key does not change the
//!   flags of later injected events by itself.
//! * **Pointer**: desktop pixels map to global display points through a [`DisplayMap`] and are
//!   clamped to the captured display. Moves with a button held are drags. Presses count clicks
//!   within [`DOUBLE_CLICK_MS`] and [`DOUBLE_CLICK_SLOP`] points.
//! * **Wheel**: RDP sends 120 units per notch; each notch is [`PIXELS_PER_NOTCH`] pixels of
//!   continuous scrolling. Windows' horizontal wheel is positive to the right and macOS's to the
//!   left, so it is negated. Trackpad scrolls with phases come from EXT-2 through
//!   [`Injector::scroll_pixels`].

use drift_input::KeyboardType;
use drift_input::keymap::{Scancode, kvk, kvk_for};
use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags, SynchronizeFlags};
use ironrdp_pdu::input::mouse::{MousePdu, PointerFlags};
use ironrdp_pdu::input::mouse_rel::{MouseRelPdu, PointerRelFlags};
use ironrdp_pdu::input::mouse_x::{MouseXPdu, PointerXFlags};

/// Pixels of continuous scrolling per wheel notch (120 RDP units).
pub const PIXELS_PER_NOTCH: f64 = 40.0;

/// RDP wheel units per notch.
pub const UNITS_PER_NOTCH: f64 = 120.0;

/// Two presses at most this far apart in time make a double click (ms).
pub const DOUBLE_CLICK_MS: u64 = 500;

/// … and at most this far apart on screen (points, each axis).
pub const DOUBLE_CLICK_SLOP: f64 = 4.0;

/// Where the remote desktop sits on the Mac's global display space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayMap {
    /// Global coordinates (points, top-left origin) of the desktop's top-left pixel.
    pub origin: (f64, f64),
    /// The desktop's size in pixels, as the client sees it.
    pub desktop: (u32, u32),
    /// Points per desktop pixel (0.5 on a 2× Retina capture).
    pub points_per_pixel: f64,
}

impl DisplayMap {
    /// The global point of desktop pixel `(x, y)`, clamped to the desktop.
    pub fn point(&self, x: f64, y: f64) -> (f64, f64) {
        let max_x = f64::from(self.desktop.0.saturating_sub(1));
        let max_y = f64::from(self.desktop.1.saturating_sub(1));
        (
            self.origin.0 + x.clamp(0.0, max_x) * self.points_per_pixel,
            self.origin.1 + y.clamp(0.0, max_y) * self.points_per_pixel,
        )
    }
}

/// Modifier flags attached to an injected event (`kCGEventFlagMask*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ModFlags(u8);

impl ModFlags {
    /// No modifiers.
    pub const NONE: Self = Self(0);
    /// Shift (either side).
    pub const SHIFT: Self = Self(1);
    /// Control (either side).
    pub const CONTROL: Self = Self(2);
    /// Option (either side).
    pub const OPTION: Self = Self(4);
    /// Command (either side).
    pub const COMMAND: Self = Self(8);

    /// Whether every flag of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The modifier a `kVK_*` key holds, if it is one.
    pub const fn of_key(code: u16) -> Self {
        match code {
            kvk::SHIFT | kvk::RIGHT_SHIFT => Self::SHIFT,
            kvk::CONTROL | kvk::RIGHT_CONTROL => Self::CONTROL,
            kvk::OPTION | kvk::RIGHT_OPTION => Self::OPTION,
            kvk::COMMAND | kvk::RIGHT_COMMAND => Self::COMMAND,
            _ => Self::NONE,
        }
    }
}

impl std::ops::BitOr for ModFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// A mouse button, numbered as CoreGraphics numbers them for `Other` (3 and 4 are X1 and X2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// Button 0.
    Left,
    /// Button 1.
    Right,
    /// Button 2.
    Middle,
    /// Button 3 and up (`kCGEventOtherMouse*`).
    Other(u8),
}

/// The phase of a trackpad scroll (`kCGScrollWheelEventScrollPhase`); `None` for wheels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScrollPhase {
    /// A mouse wheel: no phase.
    None,
    /// Fingers touched down and started moving.
    Began,
    /// Fingers moving.
    Changed,
    /// Fingers lifted.
    Ended,
    /// The scroll was cancelled.
    Cancelled,
    /// Momentum after the fingers lifted (`kCGScrollWheelEventMomentumPhase` = continue).
    Momentum,
    /// Momentum ended.
    MomentumEnded,
}

/// One event to post.
#[derive(Debug, Clone, PartialEq)]
pub enum InjectEvent {
    /// A key press or release by `kVK_*` code.
    Key {
        /// The virtual key code.
        kvk: u16,
        /// Press or release.
        down: bool,
        /// Modifiers held (after this event).
        flags: ModFlags,
    },
    /// A key event carrying one UTF-16 code unit (RDP Unicode keyboard events).
    Text {
        /// The code unit.
        utf16: u16,
        /// Press or release.
        down: bool,
    },
    /// The pointer moved to a global point.
    Move {
        /// Global x in points.
        x: f64,
        /// Global y in points.
        y: f64,
        /// The button held, which makes this a drag.
        drag: Option<MouseButton>,
        /// Modifiers held.
        flags: ModFlags,
    },
    /// A button press or release at a global point.
    Button {
        /// Which button.
        button: MouseButton,
        /// Press or release.
        down: bool,
        /// Global x in points.
        x: f64,
        /// Global y in points.
        y: f64,
        /// Click count (2 for the second press of a double click).
        clicks: u8,
        /// Modifiers held.
        flags: ModFlags,
    },
    /// Continuous (pixel) scrolling; positive `dy` scrolls up, positive `dx` scrolls left.
    Scroll {
        /// Horizontal pixels.
        dx: f64,
        /// Vertical pixels.
        dy: f64,
        /// Trackpad phase (EXT-2), or `None` for a wheel.
        phase: ScrollPhase,
    },
    /// The client's lock-key state (RDP synchronize event). The poster does not change the
    /// Mac's Caps Lock (CGEvent cannot toggle it reliably); it is reported for the host's logs.
    Locks {
        /// Caps Lock is on at the client.
        caps_lock: bool,
    },
}

/// Turns fast-path input events into [`InjectEvent`]s (one per session).
#[derive(Debug)]
pub struct Injector {
    map: DisplayMap,
    keyboard: KeyboardType,
    /// Keys held, by `kVK_*` code, in press order.
    keys: Vec<u16>,
    /// Buttons held, in press order.
    buttons: Vec<MouseButton>,
    /// Pointer position in desktop pixels.
    pointer: (f64, f64),
    /// Time (ms), point and count of the last press, for double clicks.
    last_press: Option<(u64, (f64, f64), MouseButton, u8)>,
}

impl Injector {
    /// An injector for a desktop shown at `map` on a Mac with a `keyboard`-type keyboard.
    pub fn new(map: DisplayMap, keyboard: KeyboardType) -> Self {
        Self { map, keyboard, keys: Vec::new(), buttons: Vec::new(), pointer: (0.0, 0.0), last_press: None }
    }

    /// Moves the target (the client resized the desktop, or the captured display changed).
    pub fn set_display(&mut self, map: DisplayMap) {
        self.map = map;
    }

    /// The modifiers currently held.
    pub fn flags(&self) -> ModFlags {
        self.keys.iter().fold(ModFlags::NONE, |f, &k| f | ModFlags::of_key(k))
    }

    /// The events for one fast-path input event at time `now_ms` (any monotonic clock).
    pub fn apply(&mut self, event: &FastPathInputEvent, now_ms: u64) -> Vec<InjectEvent> {
        match event {
            FastPathInputEvent::KeyboardEvent(flags, code) => self.key(*flags, *code),
            FastPathInputEvent::UnicodeKeyboardEvent(flags, utf16) => {
                vec![InjectEvent::Text { utf16: *utf16, down: !flags.contains(KeyboardFlags::RELEASE) }]
            }
            FastPathInputEvent::MouseEvent(pdu) => self.mouse(pdu, now_ms),
            FastPathInputEvent::MouseEventEx(pdu) => self.mouse_x(pdu, now_ms),
            FastPathInputEvent::MouseEventRel(pdu) => self.mouse_rel(pdu, now_ms),
            FastPathInputEvent::SyncEvent(flags) => {
                vec![InjectEvent::Locks { caps_lock: flags.contains(SynchronizeFlags::CAPS_LOCK) }]
            }
            FastPathInputEvent::QoeEvent(_) => Vec::new(),
        }
    }

    /// A trackpad scroll with its phase (EXT-2's precise scrolling).
    pub fn scroll_pixels(&self, dx: f64, dy: f64, phase: ScrollPhase) -> InjectEvent {
        InjectEvent::Scroll { dx, dy, phase }
    }

    /// Releases every held key and button (session ended or suspended), so nothing stays stuck
    /// on the Mac.
    pub fn release_all(&mut self) -> Vec<InjectEvent> {
        let mut out = Vec::new();
        while let Some(code) = self.keys.pop() {
            out.push(InjectEvent::Key { kvk: code, down: false, flags: self.flags() });
        }
        let (x, y) = self.global();
        while let Some(button) = self.buttons.pop() {
            out.push(InjectEvent::Button { button, down: false, x, y, clicks: 1, flags: ModFlags::NONE });
        }
        out
    }

    fn key(&mut self, flags: KeyboardFlags, code: u8) -> Vec<InjectEvent> {
        let scancode = Scancode { code, extended: flags.contains(KeyboardFlags::EXTENDED) };
        let Some(key) = kvk_for(scancode, self.keyboard) else { return Vec::new() };
        let down = !flags.contains(KeyboardFlags::RELEASE);
        self.keys.retain(|&k| k != key);
        if down {
            self.keys.push(key);
        }
        vec![InjectEvent::Key { kvk: key, down, flags: self.flags() }]
    }

    fn global(&self) -> (f64, f64) {
        self.map.point(self.pointer.0, self.pointer.1)
    }

    fn move_to(&mut self, x: f64, y: f64) -> InjectEvent {
        self.pointer = (x, y);
        let (gx, gy) = self.global();
        InjectEvent::Move { x: gx, y: gy, drag: self.buttons.last().copied(), flags: self.flags() }
    }

    fn button(&mut self, button: MouseButton, down: bool, now_ms: u64) -> InjectEvent {
        let (x, y) = self.global();
        let mut clicks = 1;
        self.buttons.retain(|&b| b != button);
        if down {
            if let Some((t, (px, py), b, n)) = self.last_press
                && b == button
                && now_ms.saturating_sub(t) <= DOUBLE_CLICK_MS
                && (px - x).abs() <= DOUBLE_CLICK_SLOP
                && (py - y).abs() <= DOUBLE_CLICK_SLOP
            {
                clicks = n.saturating_add(1);
            }
            self.last_press = Some((now_ms, (x, y), button, clicks));
            self.buttons.push(button);
        } else if let Some((_, _, b, n)) = self.last_press
            && b == button
        {
            clicks = n;
        }
        InjectEvent::Button { button, down, x, y, clicks, flags: self.flags() }
    }

    fn mouse(&mut self, pdu: &MousePdu, now_ms: u64) -> Vec<InjectEvent> {
        let flags = pdu.flags;
        if flags.intersects(PointerFlags::VERTICAL_WHEEL | PointerFlags::HORIZONTAL_WHEEL) {
            let pixels = f64::from(pdu.number_of_wheel_rotation_units) / UNITS_PER_NOTCH * PIXELS_PER_NOTCH;
            let (dx, dy) =
                if flags.contains(PointerFlags::HORIZONTAL_WHEEL) { (-pixels, 0.0) } else { (0.0, pixels) };
            return vec![InjectEvent::Scroll { dx, dy, phase: ScrollPhase::None }];
        }
        let mut out = Vec::new();
        let (x, y) = (f64::from(pdu.x_position), f64::from(pdu.y_position));
        let button = if flags.contains(PointerFlags::LEFT_BUTTON) {
            Some(MouseButton::Left)
        } else if flags.contains(PointerFlags::RIGHT_BUTTON) {
            Some(MouseButton::Right)
        } else if flags.contains(PointerFlags::MIDDLE_BUTTON_OR_WHEEL) {
            Some(MouseButton::Middle)
        } else {
            None
        };
        if flags.contains(PointerFlags::MOVE) || button.is_none() {
            out.push(self.move_to(x, y));
        } else {
            self.pointer = (x, y);
        }
        if let Some(button) = button {
            out.push(self.button(button, flags.contains(PointerFlags::DOWN), now_ms));
        }
        out
    }

    fn mouse_x(&mut self, pdu: &MouseXPdu, now_ms: u64) -> Vec<InjectEvent> {
        self.pointer = (f64::from(pdu.x_position), f64::from(pdu.y_position));
        let button = if pdu.flags.contains(PointerXFlags::BUTTON1) {
            MouseButton::Other(3)
        } else if pdu.flags.contains(PointerXFlags::BUTTON2) {
            MouseButton::Other(4)
        } else {
            return Vec::new();
        };
        vec![self.button(button, pdu.flags.contains(PointerXFlags::DOWN), now_ms)]
    }

    fn mouse_rel(&mut self, pdu: &MouseRelPdu, now_ms: u64) -> Vec<InjectEvent> {
        let mut out = Vec::new();
        if pdu.flags.contains(PointerRelFlags::MOVE) {
            let max_x = f64::from(self.map.desktop.0.saturating_sub(1));
            let max_y = f64::from(self.map.desktop.1.saturating_sub(1));
            let x = (self.pointer.0 + f64::from(pdu.x_delta)).clamp(0.0, max_x);
            let y = (self.pointer.1 + f64::from(pdu.y_delta)).clamp(0.0, max_y);
            out.push(self.move_to(x, y));
        }
        let down = pdu.flags.contains(PointerRelFlags::DOWN);
        for (flag, button) in [
            (PointerRelFlags::BUTTON1, MouseButton::Left),
            (PointerRelFlags::BUTTON2, MouseButton::Right),
            (PointerRelFlags::BUTTON3, MouseButton::Middle),
            (PointerRelFlags::XBUTTON1, MouseButton::Other(3)),
            (PointerRelFlags::XBUTTON2, MouseButton::Other(4)),
        ] {
            if pdu.flags.contains(flag) {
                out.push(self.button(button, down, now_ms));
            }
        }
        out
    }
}
