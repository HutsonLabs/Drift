//! [`InputEvent`] → fast-path input events, with held-key tracking for `ReleaseAll`. Pure.

use std::collections::BTreeSet;

use drift_core::{InputEvent, MouseButton};
use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags, SynchronizeFlags};
use ironrdp_pdu::input::mouse::{MousePdu, PointerFlags};
use ironrdp_pdu::input::mouse_x::{MouseXPdu, PointerXFlags};

/// Encodes input events and remembers what is held so `ReleaseAll` releases exactly that.
#[derive(Debug, Default)]
pub(crate) struct InputEncoder {
    keys: BTreeSet<(u8, bool)>,
    buttons: BTreeSet<ButtonId>,
    position: (u16, u16),
    /// Last lock-key state the app reported, re-sent after a reconnect (M7-3).
    toggles: (bool, bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ButtonId {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

impl From<MouseButton> for ButtonId {
    fn from(b: MouseButton) -> Self {
        match b {
            MouseButton::Left => Self::Left,
            MouseButton::Right => Self::Right,
            MouseButton::Middle => Self::Middle,
            MouseButton::X1 => Self::X1,
            MouseButton::X2 => Self::X2,
        }
    }
}

impl InputEncoder {
    /// The fast-path events for one input event.
    pub(crate) fn encode(&mut self, event: InputEvent) -> Vec<FastPathInputEvent> {
        match event {
            InputEvent::Key { scancode, extended, down } => {
                if down {
                    self.keys.insert((scancode, extended));
                } else {
                    self.keys.remove(&(scancode, extended));
                }
                vec![key(scancode, extended, down)]
            }
            InputEvent::Unicode { ch, down } => {
                let flags = if down { KeyboardFlags::empty() } else { KeyboardFlags::RELEASE };
                vec![FastPathInputEvent::UnicodeKeyboardEvent(flags, ch)]
            }
            InputEvent::MouseMove { x, y } => {
                self.position = (x, y);
                vec![mouse(PointerFlags::MOVE, 0, x, y)]
            }
            InputEvent::MouseButton { button, down, x, y } => {
                self.position = (x, y);
                let id = ButtonId::from(button);
                if down {
                    self.buttons.insert(id);
                } else {
                    self.buttons.remove(&id);
                }
                vec![self::button(id, down, x, y)]
            }
            InputEvent::Wheel { horizontal, units } => {
                let flags =
                    if horizontal { PointerFlags::HORIZONTAL_WHEEL } else { PointerFlags::VERTICAL_WHEEL };
                let (x, y) = self.position;
                vec![mouse(flags, units.clamp(-255, 255), x, y)]
            }
            InputEvent::SyncToggles { caps, num } => {
                self.toggles = (caps, num);
                let mut flags = SynchronizeFlags::empty();
                flags.set(SynchronizeFlags::CAPS_LOCK, caps);
                flags.set(SynchronizeFlags::NUM_LOCK, num);
                vec![FastPathInputEvent::SyncEvent(flags)]
            }
            // EXT-2: a server without `Drift::Mac::v1` gets the wheel units a precise scroll
            // carries, and nothing for gestures (plain RDP has no equivalent).
            InputEvent::PreciseScroll { wheel_v, wheel_h, .. } => {
                drift_input::scroll::wheel_events(wheel_v, wheel_h)
                    .into_iter()
                    .flat_map(|e| self.encode(e))
                    .collect()
            }
            InputEvent::Swipe { .. } | InputEvent::Pinch { .. } | InputEvent::SmartZoom => Vec::new(),
            InputEvent::ReleaseAll => {
                let (x, y) = self.position;
                let keys = std::mem::take(&mut self.keys).into_iter().map(|(sc, ext)| key(sc, ext, false));
                let buttons = std::mem::take(&mut self.buttons).into_iter().map(|b| button(b, false, x, y));
                keys.chain(buttons).collect()
            }
        }
    }
}

impl InputEncoder {
    /// The last [`InputEvent::SyncToggles`] seen (both off until the app reports them).
    pub(crate) fn toggles(&self) -> InputEvent {
        InputEvent::SyncToggles { caps: self.toggles.0, num: self.toggles.1 }
    }
}

fn key(scancode: u8, extended: bool, down: bool) -> FastPathInputEvent {
    let mut flags = KeyboardFlags::empty();
    flags.set(KeyboardFlags::EXTENDED, extended);
    flags.set(KeyboardFlags::RELEASE, !down);
    FastPathInputEvent::KeyboardEvent(flags, scancode)
}

fn mouse(flags: PointerFlags, units: i16, x: u16, y: u16) -> FastPathInputEvent {
    FastPathInputEvent::MouseEvent(MousePdu {
        flags,
        number_of_wheel_rotation_units: units,
        x_position: x,
        y_position: y,
    })
}

fn button(id: ButtonId, down: bool, x: u16, y: u16) -> FastPathInputEvent {
    let pressed = |f: PointerFlags| if down { f | PointerFlags::DOWN } else { f };
    let pressed_x = |f: PointerXFlags| if down { f | PointerXFlags::DOWN } else { f };
    match id {
        ButtonId::Left => mouse(pressed(PointerFlags::LEFT_BUTTON), 0, x, y),
        ButtonId::Right => mouse(pressed(PointerFlags::RIGHT_BUTTON), 0, x, y),
        ButtonId::Middle => mouse(pressed(PointerFlags::MIDDLE_BUTTON_OR_WHEEL), 0, x, y),
        ButtonId::X1 => FastPathInputEvent::MouseEventEx(MouseXPdu {
            flags: pressed_x(PointerXFlags::BUTTON1),
            x_position: x,
            y_position: y,
        }),
        ButtonId::X2 => FastPathInputEvent::MouseEventEx(MouseXPdu {
            flags: pressed_x(PointerXFlags::BUTTON2),
            x_position: x,
            y_position: y,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(events: &[InputEvent]) -> Vec<FastPathInputEvent> {
        let mut e = InputEncoder::default();
        events.iter().flat_map(|ev| e.encode(*ev)).collect()
    }

    #[test]
    fn keys_and_unicode() {
        assert_eq!(
            enc(&[
                InputEvent::Key { scancode: 0x1C, extended: false, down: true },
                InputEvent::Key { scancode: 0x5B, extended: true, down: false },
                InputEvent::Unicode { ch: 0xE9, down: true },
                InputEvent::Unicode { ch: 0xE9, down: false },
            ]),
            vec![
                FastPathInputEvent::KeyboardEvent(KeyboardFlags::empty(), 0x1C),
                FastPathInputEvent::KeyboardEvent(KeyboardFlags::EXTENDED | KeyboardFlags::RELEASE, 0x5B),
                FastPathInputEvent::UnicodeKeyboardEvent(KeyboardFlags::empty(), 0xE9),
                FastPathInputEvent::UnicodeKeyboardEvent(KeyboardFlags::RELEASE, 0xE9),
            ]
        );
    }

    #[test]
    fn mouse_buttons_and_wheel() {
        let got = enc(&[
            InputEvent::MouseMove { x: 10, y: 20 },
            InputEvent::MouseButton { button: MouseButton::Left, down: true, x: 11, y: 21 },
            InputEvent::MouseButton { button: MouseButton::X2, down: true, x: 11, y: 21 },
            InputEvent::Wheel { horizontal: false, units: -300 },
            InputEvent::Wheel { horizontal: true, units: 12 },
        ]);
        assert_eq!(got[0], mouse(PointerFlags::MOVE, 0, 10, 20));
        assert_eq!(got[1], mouse(PointerFlags::LEFT_BUTTON | PointerFlags::DOWN, 0, 11, 21));
        assert_eq!(
            got[2],
            FastPathInputEvent::MouseEventEx(MouseXPdu {
                flags: PointerXFlags::BUTTON2 | PointerXFlags::DOWN,
                x_position: 11,
                y_position: 21
            })
        );
        assert_eq!(got[3], mouse(PointerFlags::VERTICAL_WHEEL, -255, 11, 21), "clamped to 9 bits");
        assert_eq!(got[4], mouse(PointerFlags::HORIZONTAL_WHEEL, 12, 11, 21));
    }

    #[test]
    fn sync_toggles() {
        assert_eq!(
            enc(&[InputEvent::SyncToggles { caps: true, num: false }]),
            vec![FastPathInputEvent::SyncEvent(SynchronizeFlags::CAPS_LOCK)]
        );
    }

    #[test]
    fn release_all_releases_exactly_what_is_held() {
        let got = enc(&[
            InputEvent::Key { scancode: 0x1D, extended: false, down: true },
            InputEvent::Key { scancode: 0x2E, extended: false, down: true },
            InputEvent::Key { scancode: 0x2E, extended: false, down: false },
            InputEvent::MouseButton { button: MouseButton::Right, down: true, x: 3, y: 4 },
            InputEvent::MouseButton { button: MouseButton::Middle, down: true, x: 3, y: 4 },
            InputEvent::MouseButton { button: MouseButton::Middle, down: false, x: 3, y: 4 },
            InputEvent::ReleaseAll,
            InputEvent::ReleaseAll,
        ]);
        assert_eq!(
            &got[6..],
            &[
                FastPathInputEvent::KeyboardEvent(KeyboardFlags::RELEASE, 0x1D),
                mouse(PointerFlags::RIGHT_BUTTON, 0, 3, 4),
            ]
        );
        let mut e = InputEncoder::default();
        e.encode(InputEvent::MouseButton { button: MouseButton::X1, down: true, x: 1, y: 1 });
        assert_eq!(
            e.encode(InputEvent::ReleaseAll),
            vec![FastPathInputEvent::MouseEventEx(MouseXPdu {
                flags: PointerXFlags::BUTTON1,
                x_position: 1,
                y_position: 1
            })]
        );
    }

    #[test]
    fn precise_scroll_falls_back_to_its_wheel_units_and_gestures_to_nothing() {
        use drift_core::{MomentumPhase, ScrollPhase, SwipeDirection};
        let scroll = InputEvent::PreciseScroll {
            dx: 900,
            dy: -100,
            phase: ScrollPhase::Changed,
            momentum: MomentumPhase::None,
            wheel_v: -300,
            wheel_h: 0,
        };
        assert_eq!(
            enc(&[scroll]),
            enc(&[
                InputEvent::Wheel { horizontal: false, units: -255 },
                InputEvent::Wheel { horizontal: false, units: -45 },
            ])
        );
        assert!(
            enc(&[
                InputEvent::Swipe { direction: SwipeDirection::Up, fingers: 3 },
                InputEvent::Pinch { magnification: 40, phase: ScrollPhase::Changed },
                InputEvent::SmartZoom,
            ])
            .is_empty()
        );
    }
}
