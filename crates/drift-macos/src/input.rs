//! Per-view input state behind `RemoteView` (tasks **M2-2**, **M2-4**; pure).
//!
//! [`InputController`] is the humble object's brain: `RemoteView` extracts plain values from
//! each `NSEvent` (key code, modifier flags, point in flipped view coordinates, deltas) and gets
//! back the exact [`InputEvent`]s to send. It combines `drift-input`'s keyboard translator,
//! scroll accumulator and viewport mapping with the pointer-button state, and makes the
//! `performKeyEquivalent:` decision ([`KeyEquivalent`]).

use std::collections::BTreeSet;

use drift_core::{
    DesktopSize, InputEvent, KeyboardPrefs, MomentumPhase, MouseButton, Point, ScrollPhase, Size,
    ViewGeometry,
};
use drift_input::{
    KeyDown, Keyboard, KeyboardType, MenuShortcut, ModifierFlags, ScaleMode, ScrollAccumulator, ScrollConfig,
    ScrollDelta, Viewport, menu_shortcut,
};

/// Outcome of `performKeyEquivalent:` for one key-down event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyEquivalent {
    /// Not ours: return `NO` (the view is not first responder, or no Command/Control is held,
    /// so AppKit delivers the event through `keyDown:`).
    Pass,
    /// An allow-listed Drift shortcut: return `NO` so the menu handles it; the remote never
    /// sees the combo.
    Menu(MenuShortcut),
    /// Claimed for the remote: return `YES` and send these events.
    Claim(Vec<InputEvent>),
}

/// Maps an `NSEvent.buttonNumber` to an RDP pointer button.
pub fn mouse_button(button_number: i64) -> Option<MouseButton> {
    Some(match button_number {
        0 => MouseButton::Left,
        1 => MouseButton::Right,
        2 => MouseButton::Middle,
        3 => MouseButton::X1,
        4 => MouseButton::X2,
        _ => return None,
    })
}

/// Stable ordering key for held buttons.
fn button_index(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::X1 => 3,
        MouseButton::X2 => 4,
    }
}

fn button_from_index(i: u8) -> MouseButton {
    match i {
        0 => MouseButton::Left,
        1 => MouseButton::Right,
        2 => MouseButton::Middle,
        3 => MouseButton::X1,
        _ => MouseButton::X2,
    }
}

/// `NSEventPhase` bits (`phase` and `momentumPhase` share them).
mod phase_bits {
    pub const BEGAN: u64 = 1;
    pub const STATIONARY: u64 = 2;
    pub const CHANGED: u64 = 4;
    pub const ENDED: u64 = 8;
    pub const CANCELLED: u64 = 16;
    pub const MAY_BEGIN: u64 = 32;
}

/// A scroll or magnify event's `phase` (raw `NSEventPhase` bits) as the wire's phase (EXT-2).
pub const fn scroll_phase(raw: u64) -> ScrollPhase {
    use phase_bits::*;
    if raw & BEGAN != 0 {
        ScrollPhase::Began
    } else if raw & (CHANGED | STATIONARY) != 0 {
        ScrollPhase::Changed
    } else if raw & ENDED != 0 {
        ScrollPhase::Ended
    } else if raw & CANCELLED != 0 {
        ScrollPhase::Cancelled
    } else if raw & MAY_BEGIN != 0 {
        ScrollPhase::MayBegin
    } else {
        ScrollPhase::None
    }
}

/// A scroll event's `momentumPhase` (raw `NSEventPhase` bits) as the wire's momentum (EXT-2).
pub const fn momentum_phase(raw: u64) -> MomentumPhase {
    use phase_bits::*;
    if raw & BEGAN != 0 {
        MomentumPhase::Began
    } else if raw & (CHANGED | STATIONARY) != 0 {
        MomentumPhase::Changed
    } else if raw & (ENDED | CANCELLED) != 0 {
        MomentumPhase::Ended
    } else {
        MomentumPhase::None
    }
}

/// Input state of one remote view.
#[derive(Debug, Clone)]
pub struct InputController {
    keyboard: Keyboard,
    scroll: ScrollAccumulator,
    view: ViewGeometry,
    desktop: Option<(DesktopSize, ScaleMode)>,
    viewport: Option<Viewport>,
    /// Buttons pressed on the remote (by [`button_index`]).
    buttons: BTreeSet<u8>,
    /// Last pointer position sent (desktop pixels).
    last: Option<Point<u16>>,
}

impl InputController {
    /// A controller for a profile's keyboard preferences, the local keyboard type and the scroll
    /// settings. No desktop is attached yet, so pointer events are dropped until
    /// [`InputController::set_desktop`].
    pub fn new(prefs: KeyboardPrefs, keyboard: KeyboardType, scroll: ScrollConfig) -> Self {
        Self {
            keyboard: Keyboard::new(prefs, keyboard),
            scroll: ScrollAccumulator::new(scroll),
            view: ViewGeometry { points: Size::new(0.0, 0.0), backing_scale: 1.0 },
            desktop: None,
            viewport: None,
            buttons: BTreeSet::new(),
            last: None,
        }
    }

    /// Changes the keyboard preferences (profile edited while connected).
    pub fn set_keyboard_prefs(&mut self, prefs: KeyboardPrefs) {
        self.keyboard.set_prefs(prefs);
    }

    /// Changes the scroll conversion settings.
    pub fn set_scroll_config(&mut self, config: ScrollConfig) {
        self.scroll = ScrollAccumulator::new(config);
    }

    /// The view's size/backing scale changed.
    pub fn set_view_geometry(&mut self, view: ViewGeometry) {
        self.view = view;
        self.update_viewport();
    }

    /// The remote desktop size and how it is placed in the view (after activation / resize).
    pub fn set_desktop(&mut self, desktop: DesktopSize, mode: ScaleMode) {
        self.desktop = Some((desktop, mode));
        self.update_viewport();
    }

    /// Detaches the desktop (disconnected): pointer events are dropped.
    pub fn clear_desktop(&mut self) {
        self.desktop = None;
        self.viewport = None;
        self.last = None;
    }

    fn update_viewport(&mut self) {
        self.viewport = self.desktop.map(|(desktop, mode)| Viewport::new(self.view, desktop, mode));
        self.last = None;
    }

    /// `keyDown:`.
    pub fn key_down(&mut self, kvk: u16, flags: ModifierFlags, is_repeat: bool, composing: bool) -> KeyDown {
        self.keyboard.key_down(kvk, flags, is_repeat, composing)
    }

    /// Scancode fallback for `doCommandBySelector:` after [`KeyDown::InterpretText`].
    pub fn key_down_scancode(&mut self, kvk: u16, flags: ModifierFlags) -> Vec<InputEvent> {
        self.keyboard.key_down_scancode(kvk, flags)
    }

    /// `keyUp:`.
    pub fn key_up(&mut self, kvk: u16) -> Vec<InputEvent> {
        self.keyboard.key_up(kvk)
    }

    /// `flagsChanged:`.
    pub fn flags_changed(&mut self, kvk: u16, flags: ModifierFlags) -> Vec<InputEvent> {
        self.keyboard.flags_changed(kvk, flags)
    }

    /// `insertText:` (typed text, dead-key and IME commits).
    pub fn insert_text(&mut self, text: &str) -> Vec<InputEvent> {
        self.keyboard.insert_text(text)
    }

    /// `performKeyEquivalent:` for a key-down event.
    ///
    /// * The view is not first responder (e.g. the web view has focus) → [`KeyEquivalent::Pass`].
    /// * Neither Command nor Control held → [`KeyEquivalent::Pass`] (AppKit then calls `keyDown:`).
    /// * An allow-listed combo ([`menu_shortcut`]) → [`KeyEquivalent::Menu`]; the deferred
    ///   Command press is dropped, so the remote never sees it.
    /// * Anything else → [`KeyEquivalent::Claim`] with the scancode events (chords never use
    ///   Unicode routing).
    pub fn key_equivalent(
        &mut self,
        is_first_responder: bool,
        kvk: u16,
        chars_ignoring_modifiers: &str,
        flags: ModifierFlags,
        is_repeat: bool,
    ) -> KeyEquivalent {
        if !is_first_responder || !(flags.command() || flags.control()) {
            return KeyEquivalent::Pass;
        }
        if let Some(shortcut) = menu_shortcut(kvk, chars_ignoring_modifiers, flags) {
            self.keyboard.menu_shortcut_taken();
            return KeyEquivalent::Menu(shortcut);
        }
        let events = match self.keyboard.key_down(kvk, flags, is_repeat, false) {
            KeyDown::Send(events) => events,
            // Unreachable for chords (Command/Control always route to scancodes), but stay safe.
            KeyDown::InterpretText if is_repeat => Vec::new(),
            KeyDown::InterpretText => self.keyboard.key_down_scancode(kvk, flags),
        };
        KeyEquivalent::Claim(events)
    }

    fn map(&self, point: Point<f64>) -> Option<Point<u16>> {
        self.viewport.and_then(|v| v.view_to_desktop(point))
    }

    /// `mouseMoved:` / `*MouseDragged:` at a point in flipped view coordinates.
    pub fn mouse_move(&mut self, point: Point<f64>) -> Vec<InputEvent> {
        let Some(p) = self.map(point) else { return Vec::new() };
        if self.last == Some(p) {
            return Vec::new();
        }
        self.last = Some(p);
        vec![InputEvent::MouseMove { x: p.x, y: p.y }]
    }

    /// `*MouseDown:` / `*MouseUp:` at a point in flipped view coordinates.
    pub fn mouse_button(&mut self, button: MouseButton, down: bool, point: Point<f64>) -> Vec<InputEvent> {
        let Some(p) = self.map(point) else { return Vec::new() };
        let index = button_index(button);
        let mut out = Vec::new();
        if down {
            out.extend(self.keyboard.prepare_pointer_button());
            self.buttons.insert(index);
        } else if !self.buttons.remove(&index) {
            // Already released (focus loss) or pressed before the view saw it.
            return out;
        }
        self.last = Some(p);
        out.push(InputEvent::MouseButton { button, down, x: p.x, y: p.y });
        out
    }

    /// `scrollWheel:`. `gesture_began` is `phase == NSEventPhaseBegan` (drops the remainder of
    /// the previous gesture).
    pub fn scroll(&mut self, delta: ScrollDelta, gesture_began: bool) -> Vec<InputEvent> {
        if gesture_began {
            self.scroll.reset();
        }
        self.scroll.push(delta)
    }

    /// A precise (trackpad, Magic Mouse) `scrollWheel:` as one [`InputEvent::PreciseScroll`]
    /// (EXT-2): the points and phases for a Drift Host, and the same wheel units
    /// [`Self::scroll`] sends for anyone else.
    pub fn precise_scroll(
        &mut self,
        dx: f64,
        dy: f64,
        phase: ScrollPhase,
        momentum: MomentumPhase,
    ) -> InputEvent {
        let fallback = self.scroll(ScrollDelta::Precise { dx, dy }, phase == ScrollPhase::Began);
        let (wheel_v, wheel_h) = drift_input::scroll::wheel_units(&fallback);
        let sign = if self.scroll.config().reverse { -1.0 } else { 1.0 };
        InputEvent::PreciseScroll {
            dx: drift_input::scroll::centi(sign * dx),
            dy: drift_input::scroll::centi(sign * dy),
            phase,
            momentum,
            wheel_v,
            wheel_h,
        }
    }

    /// Key focus gained (first responder in the key window) with the current modifier flags.
    pub fn focus_gained(&mut self, flags: ModifierFlags) -> Vec<InputEvent> {
        self.keyboard.focus_gained(flags)
    }

    /// Key focus lost: releases held keys and pointer buttons.
    pub fn focus_lost(&mut self) -> Vec<InputEvent> {
        let mut out = self.keyboard.focus_lost();
        let buttons = std::mem::take(&mut self.buttons);
        if let Some(p) = self.last {
            out.extend(buttons.into_iter().map(|i| InputEvent::MouseButton {
                button: button_from_index(i),
                down: false,
                x: p.x,
                y: p.y,
            }));
        }
        self.scroll.reset();
        out
    }

    /// "Send Ctrl+Alt+Del" menu item.
    pub fn ctrl_alt_del(&mut self) -> Vec<InputEvent> {
        self.keyboard.ctrl_alt_del()
    }
}
