//! The client's keyboard and mouse injected into this Mac (HOST-6's `drift-inject`).
//!
//! `ironrdp-server` hands over decoded [`KeyboardEvent`]s and [`MouseEvent`]s; `drift-inject`'s
//! [`Injector`] plans from fast-path events, so the events are turned back into fast-path form
//! here (pure, tested), planned, and posted. Posting is a function, so tests record the plan
//! instead of moving the real pointer. A session ending releases every held key and button.

use std::time::Instant;

use drift_ext::gestures::Gesture;
use drift_inject::gesture::GesturePlanner;
use drift_inject::plan::{DisplayMap, InjectEvent, Injector};
use drift_input::KeyboardType;
use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
use ironrdp_pdu::input::mouse::{MousePdu, PointerFlags};
use ironrdp_pdu::input::mouse_rel::{MouseRelPdu, PointerRelFlags};
use ironrdp_pdu::input::mouse_x::{MouseXPdu, PointerXFlags};
use ironrdp_server::{KeyboardEvent, MouseButton, MouseEvent};

use crate::input::InputSink;

/// A server keyboard event as the fast-path event the client sent.
pub fn to_fast_path(event: &KeyboardEvent) -> Option<FastPathInputEvent> {
    let ext = |extended: bool| if extended { KeyboardFlags::EXTENDED } else { KeyboardFlags::empty() };
    Some(match *event {
        KeyboardEvent::Pressed { code, extended } => FastPathInputEvent::KeyboardEvent(ext(extended), code),
        KeyboardEvent::Released { code, extended } => {
            FastPathInputEvent::KeyboardEvent(ext(extended) | KeyboardFlags::RELEASE, code)
        }
        KeyboardEvent::UnicodePressed(u) => {
            FastPathInputEvent::UnicodeKeyboardEvent(KeyboardFlags::empty(), u)
        }
        KeyboardEvent::UnicodeReleased(u) => {
            FastPathInputEvent::UnicodeKeyboardEvent(KeyboardFlags::RELEASE, u)
        }
        KeyboardEvent::Synchronize(flags) => FastPathInputEvent::SyncEvent(flags),
    })
}

fn clamp16(v: i32) -> i16 {
    i16::try_from(v.clamp(i32::from(i16::MIN), i32::from(i16::MAX))).unwrap_or(0)
}

fn mouse(flags: PointerFlags, units: i16, x: u16, y: u16) -> FastPathInputEvent {
    FastPathInputEvent::MouseEvent(MousePdu {
        flags,
        number_of_wheel_rotation_units: units,
        x_position: x,
        y_position: y,
    })
}

/// A server mouse event as the fast-path event the client sent (`None` for kinds the injector
/// has no use for).
pub fn mouse_to_fast_path(event: &MouseEvent) -> Option<FastPathInputEvent> {
    Some(match *event {
        MouseEvent::Move { x, y } => mouse(PointerFlags::MOVE, 0, x, y),
        MouseEvent::Button { x, y, button, pressed } => {
            let down = |f: PointerFlags| if pressed { f | PointerFlags::DOWN } else { f };
            match button {
                MouseButton::Left => mouse(down(PointerFlags::LEFT_BUTTON), 0, x, y),
                MouseButton::Right => mouse(down(PointerFlags::RIGHT_BUTTON), 0, x, y),
                MouseButton::Middle => mouse(down(PointerFlags::MIDDLE_BUTTON_OR_WHEEL), 0, x, y),
                MouseButton::X1 | MouseButton::X2 => {
                    let mut flags = if button == MouseButton::X1 {
                        PointerXFlags::BUTTON1
                    } else {
                        PointerXFlags::BUTTON2
                    };
                    if pressed {
                        flags |= PointerXFlags::DOWN;
                    }
                    FastPathInputEvent::MouseEventEx(MouseXPdu { flags, x_position: x, y_position: y })
                }
                _ => return None,
            }
        }
        MouseEvent::VerticalScroll { value } => mouse(PointerFlags::VERTICAL_WHEEL, value, 0, 0),
        MouseEvent::HorizontalScroll { value } => mouse(PointerFlags::HORIZONTAL_WHEEL, value, 0, 0),
        MouseEvent::RelMove { x, y } => FastPathInputEvent::MouseEventRel(MouseRelPdu {
            flags: PointerRelFlags::MOVE,
            x_delta: clamp16(x),
            y_delta: clamp16(y),
        }),
        MouseEvent::ButtonRel { x, y, button, pressed } => {
            let mut flags = match button {
                MouseButton::Left => PointerRelFlags::BUTTON1,
                MouseButton::Right => PointerRelFlags::BUTTON2,
                MouseButton::Middle => PointerRelFlags::BUTTON3,
                MouseButton::X1 => PointerRelFlags::XBUTTON1,
                MouseButton::X2 => PointerRelFlags::XBUTTON2,
                _ => return None,
            };
            if pressed {
                flags |= PointerRelFlags::DOWN;
            }
            FastPathInputEvent::MouseEventRel(MouseRelPdu { flags, x_delta: clamp16(x), y_delta: clamp16(y) })
        }
        _ => return None,
    })
}

type Post = Box<dyn FnMut(&InjectEvent) + Send>;

/// Plans each client event with an [`Injector`] and posts the result.
pub struct InjectingInput {
    injector: Injector,
    gestures: GesturePlanner,
    post: Post,
    start: Instant,
}

impl std::fmt::Debug for InjectingInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InjectingInput").field("injector", &self.injector).finish_non_exhaustive()
    }
}

impl InjectingInput {
    /// Injects into the desktop at `map` (an ANSI keyboard), handing each event to `post`.
    pub fn new(map: DisplayMap, post: impl FnMut(&InjectEvent) + Send + 'static) -> Self {
        Self::with_keyboard(map, KeyboardType::default(), post)
    }

    /// As [`Self::new`], for a Mac whose keyboard is `keyboard`.
    pub fn with_keyboard(
        map: DisplayMap,
        keyboard: KeyboardType,
        post: impl FnMut(&InjectEvent) + Send + 'static,
    ) -> Self {
        Self {
            injector: Injector::new(map, keyboard),
            gestures: GesturePlanner::default(),
            post: Box::new(post),
            start: Instant::now(),
        }
    }

    /// Posts through CoreGraphics (needs Accessibility; see `drift_inject::cg`).
    pub fn posting(map: DisplayMap, keyboard: KeyboardType) -> Self {
        Self::with_keyboard(map, keyboard, |e: &InjectEvent| drift_inject::cg::post(e))
    }

    fn apply(&mut self, event: Option<FastPathInputEvent>) {
        let Some(event) = event else { return };
        let now = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        for e in self.injector.apply(&event, now) {
            (self.post)(&e);
        }
    }

    fn release(&mut self) {
        for e in self.injector.release_all() {
            (self.post)(&e);
        }
    }
}

impl InputSink for InjectingInput {
    fn keyboard(&mut self, event: KeyboardEvent) {
        self.apply(to_fast_path(&event));
    }

    fn mouse(&mut self, event: MouseEvent) {
        self.apply(mouse_to_fast_path(&event));
    }

    fn gesture(&mut self, gesture: Gesture) {
        for e in self.gestures.plan(&gesture) {
            (self.post)(&e);
        }
    }

    fn session_ended(&mut self) {
        self.gestures = GesturePlanner::default();
        self.release();
    }
}

impl Drop for InjectingInput {
    fn drop(&mut self) {
        self.release();
    }
}
