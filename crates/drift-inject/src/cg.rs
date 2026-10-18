//! Posts [`InjectEvent`]s with `CGEventPost` at the HID tap (task **HOST-6**). Humble object.
//!
//! Posting needs the Accessibility permission ([`accessibility_trusted`]); without it macOS
//! drops the events silently. [`describe`] builds the event and reads it back without posting,
//! which is what the smoke tests check.

use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventTapLocation, CGEventType, CGMomentumScrollPhase,
    CGMouseButton, CGScrollEventUnit, CGScrollPhase,
};

use crate::plan::{InjectEvent, ModFlags, MouseButton, ScrollPhase};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// Whether this process may post events (System Settings › Privacy & Security › Accessibility).
pub fn accessibility_trusted() -> bool {
    // SAFETY: a plain query without arguments.
    unsafe { AXIsProcessTrusted() }
}

/// The kind of a built event, for tests and logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// `kCGEventKeyDown`.
    KeyDown,
    /// `kCGEventKeyUp`.
    KeyUp,
    /// `kCGEventFlagsChanged` (a modifier key).
    FlagsChanged,
    /// `kCGEventMouseMoved`.
    MouseMoved,
    /// `kCGEventLeftMouseDown`.
    LeftMouseDown,
    /// `kCGEventLeftMouseUp`.
    LeftMouseUp,
    /// `kCGEventRightMouseDown`.
    RightMouseDown,
    /// `kCGEventRightMouseUp`.
    RightMouseUp,
    /// `kCGEventOtherMouseDown`.
    OtherMouseDown,
    /// `kCGEventOtherMouseUp`.
    OtherMouseUp,
    /// `kCGEventLeftMouseDragged`.
    LeftMouseDragged,
    /// `kCGEventRightMouseDragged`.
    RightMouseDragged,
    /// `kCGEventOtherMouseDragged`.
    OtherMouseDragged,
    /// `kCGEventScrollWheel`.
    ScrollWheel,
    /// Anything else.
    Other(u32),
}

impl From<CGEventType> for EventKind {
    fn from(t: CGEventType) -> Self {
        match t {
            CGEventType::KeyDown => Self::KeyDown,
            CGEventType::KeyUp => Self::KeyUp,
            CGEventType::FlagsChanged => Self::FlagsChanged,
            CGEventType::MouseMoved => Self::MouseMoved,
            CGEventType::LeftMouseDown => Self::LeftMouseDown,
            CGEventType::LeftMouseUp => Self::LeftMouseUp,
            CGEventType::RightMouseDown => Self::RightMouseDown,
            CGEventType::RightMouseUp => Self::RightMouseUp,
            CGEventType::OtherMouseDown => Self::OtherMouseDown,
            CGEventType::OtherMouseUp => Self::OtherMouseUp,
            CGEventType::LeftMouseDragged => Self::LeftMouseDragged,
            CGEventType::RightMouseDragged => Self::RightMouseDragged,
            CGEventType::OtherMouseDragged => Self::OtherMouseDragged,
            CGEventType::ScrollWheel => Self::ScrollWheel,
            other => Self::Other(other.0),
        }
    }
}

/// What a built event contains, read back from the `CGEvent`.
#[derive(Debug, Clone, PartialEq)]
pub struct Described {
    /// Its type.
    pub kind: EventKind,
    /// `kCGKeyboardEventKeycode` for key events.
    pub keycode: Option<i64>,
    /// Shift flag.
    pub shift: bool,
    /// Control flag.
    pub control: bool,
    /// Option flag.
    pub option: bool,
    /// Command flag.
    pub command: bool,
    /// Location (global points).
    pub location: (f64, f64),
    /// `kCGMouseEventClickState`.
    pub clicks: i64,
    /// `kCGMouseEventButtonNumber`.
    pub button_number: i64,
    /// Pixel deltas (vertical, horizontal) of a scroll.
    pub scroll_pixels: (i64, i64),
    /// `kCGScrollWheelEventIsContinuous`.
    pub continuous: bool,
    /// `kCGScrollWheelEventScrollPhase`.
    pub phase: i64,
    /// The Unicode string of a key event, if one was set.
    pub text: Option<String>,
}

/// Builds the `CGEvent` for `event`; `None` for events that post nothing ([`InjectEvent::Locks`])
/// or that CoreGraphics refuses to create.
pub fn build(event: &InjectEvent) -> Option<CFRetained<CGEvent>> {
    match *event {
        InjectEvent::Key { kvk, down, flags } => {
            let e = CGEvent::new_keyboard_event(None, kvk, down)?;
            CGEvent::set_flags(Some(&e), cg_flags(flags));
            Some(e)
        }
        InjectEvent::Text { utf16, down } => {
            let e = CGEvent::new_keyboard_event(None, 0, down)?;
            // SAFETY: one UTF-16 unit read from a live local for the duration of the call.
            unsafe { CGEvent::keyboard_set_unicode_string(Some(&e), 1, &raw const utf16) };
            Some(e)
        }
        InjectEvent::Move { x, y, drag, flags } => {
            let (kind, button) = match drag {
                None => (CGEventType::MouseMoved, CGMouseButton::Left),
                Some(MouseButton::Left) => (CGEventType::LeftMouseDragged, CGMouseButton::Left),
                Some(MouseButton::Right) => (CGEventType::RightMouseDragged, CGMouseButton::Right),
                Some(MouseButton::Middle | MouseButton::Other(_)) => {
                    (CGEventType::OtherMouseDragged, CGMouseButton::Center)
                }
            };
            let e = CGEvent::new_mouse_event(None, kind, CGPoint { x, y }, button)?;
            if let Some(MouseButton::Other(n)) = drag {
                CGEvent::set_integer_value_field(
                    Some(&e),
                    CGEventField::MouseEventButtonNumber,
                    i64::from(n),
                );
            }
            CGEvent::set_flags(Some(&e), cg_flags(flags));
            Some(e)
        }
        InjectEvent::Button { button, down, x, y, clicks, flags } => {
            let (kind, cg_button) = match (button, down) {
                (MouseButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
                (MouseButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
                (MouseButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right),
                (MouseButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right),
                (_, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
                (_, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
            };
            let e = CGEvent::new_mouse_event(None, kind, CGPoint { x, y }, cg_button)?;
            if let MouseButton::Other(n) = button {
                CGEvent::set_integer_value_field(
                    Some(&e),
                    CGEventField::MouseEventButtonNumber,
                    i64::from(n),
                );
            }
            CGEvent::set_integer_value_field(Some(&e), CGEventField::MouseEventClickState, i64::from(clicks));
            CGEvent::set_flags(Some(&e), cg_flags(flags));
            Some(e)
        }
        InjectEvent::Scroll { dx, dy, phase } => {
            let e = CGEvent::new_scroll_wheel_event2(
                None,
                CGScrollEventUnit::Pixel,
                2,
                dy.round() as i32,
                dx.round() as i32,
                0,
            )?;
            let (scroll, momentum) = phases(phase);
            CGEvent::set_integer_value_field(Some(&e), CGEventField::ScrollWheelEventScrollPhase, scroll);
            CGEvent::set_integer_value_field(Some(&e), CGEventField::ScrollWheelEventMomentumPhase, momentum);
            Some(e)
        }
        InjectEvent::Locks { .. } => None,
    }
}

/// Builds `event` and reads it back (tests and diagnostics); nothing is posted.
pub fn describe(event: &InjectEvent) -> Option<Described> {
    let e = build(event)?;
    let e = Some(&*e);
    let kind = EventKind::from(CGEvent::r#type(e));
    let flags = CGEvent::flags(e);
    let field = |f: CGEventField| CGEvent::integer_value_field(e, f);
    let location = CGEvent::location(e);
    let keyed = matches!(kind, EventKind::KeyDown | EventKind::KeyUp | EventKind::FlagsChanged);
    let text = if keyed { unicode_string(e) } else { None };
    Some(Described {
        kind,
        keycode: keyed.then(|| field(CGEventField::KeyboardEventKeycode)),
        shift: flags.contains(CGEventFlags::MaskShift),
        control: flags.contains(CGEventFlags::MaskControl),
        option: flags.contains(CGEventFlags::MaskAlternate),
        command: flags.contains(CGEventFlags::MaskCommand),
        location: (location.x, location.y),
        clicks: field(CGEventField::MouseEventClickState),
        button_number: field(CGEventField::MouseEventButtonNumber),
        scroll_pixels: (
            field(CGEventField::ScrollWheelEventPointDeltaAxis1),
            field(CGEventField::ScrollWheelEventPointDeltaAxis2),
        ),
        continuous: field(CGEventField::ScrollWheelEventIsContinuous) != 0,
        phase: field(CGEventField::ScrollWheelEventScrollPhase),
        text,
    })
}

/// Posts `event` at the HID tap (no-op for events that post nothing).
pub fn post(event: &InjectEvent) {
    if let Some(e) = build(event) {
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&e));
    }
}

/// The pointer's current global location, from a fresh event.
pub fn current_pointer() -> Option<(f64, f64)> {
    let e = CGEvent::new(None)?;
    let p = CGEvent::location(Some(&e));
    Some((p.x, p.y))
}

fn cg_flags(flags: ModFlags) -> CGEventFlags {
    let mut out = CGEventFlags::empty();
    for (ours, theirs) in [
        (ModFlags::SHIFT, CGEventFlags::MaskShift),
        (ModFlags::CONTROL, CGEventFlags::MaskControl),
        (ModFlags::OPTION, CGEventFlags::MaskAlternate),
        (ModFlags::COMMAND, CGEventFlags::MaskCommand),
    ] {
        if flags.contains(ours) {
            out |= theirs;
        }
    }
    out
}

/// `kCGScrollWheelEventScrollPhase` and `kCGScrollWheelEventMomentumPhase` values.
fn phases(phase: ScrollPhase) -> (i64, i64) {
    let scroll = |p: CGScrollPhase| i64::from(p.0);
    let momentum = |p: CGMomentumScrollPhase| i64::from(p.0);
    match phase {
        ScrollPhase::None => (0, 0),
        ScrollPhase::Began => (scroll(CGScrollPhase::Began), 0),
        ScrollPhase::Changed => (scroll(CGScrollPhase::Changed), 0),
        ScrollPhase::Ended => (scroll(CGScrollPhase::Ended), 0),
        ScrollPhase::Cancelled => (scroll(CGScrollPhase::Cancelled), 0),
        ScrollPhase::MomentumBegan => (0, momentum(CGMomentumScrollPhase::Begin)),
        ScrollPhase::Momentum => (0, momentum(CGMomentumScrollPhase::Continue)),
        ScrollPhase::MomentumEnded => (0, momentum(CGMomentumScrollPhase::End)),
    }
}

fn unicode_string(e: Option<&CGEvent>) -> Option<String> {
    let mut buf = [0_u16; 8];
    let mut len: std::ffi::c_ulong = 0;
    // SAFETY: the buffer holds `buf.len()` units and `len` is a valid out pointer.
    unsafe { CGEvent::keyboard_get_unicode_string(e, buf.len() as _, &raw mut len, buf.as_mut_ptr()) };
    let len = usize::try_from(len).ok()?.min(buf.len());
    (len > 0).then(|| String::from_utf16_lossy(&buf[..len]))
}
