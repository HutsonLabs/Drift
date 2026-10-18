//! Scroll with phase and momentum, and trackpad gestures, on `Drift::Mac::v1` (task **EXT-2**;
//! ADR `EXT-2-scroll-gestures`).
//!
//! Payloads (little-endian):
//!
//! ```text
//! SCROLL      dx: i32, dy: i32 (hundredths of a point, AppKit signs), phase: u8, momentum: u8
//! SWIPE       direction: u8, fingers: u8
//! PINCH       magnification: i32 (thousandths, change since the last event), phase: u8
//! SMART_ZOOM  (empty)
//! ```
//!
//! A client sends these only for capabilities both ends agreed on ([`from_input`]); otherwise
//! the input stays plain RDP (a precise scroll's wheel units; gestures are dropped).

use drift_core::{InputEvent, MomentumPhase, ScrollPhase, SwipeDirection};

use crate::Negotiated;
use crate::wire::{Capabilities, Message, MessageType, WireError};

/// Message types of the EXT-2 range (`0x01xx`).
pub mod types {
    use crate::wire::MessageType;

    /// Client → host: a precise scroll.
    pub const SCROLL: MessageType = MessageType(0x0101);
    /// Client → host: a multi-finger swipe.
    pub const SWIPE: MessageType = MessageType(0x0102);
    /// Client → host: a pinch.
    pub const PINCH: MessageType = MessageType(0x0103);
    /// Client → host: a smart zoom (two-finger double tap).
    pub const SMART_ZOOM: MessageType = MessageType(0x0104);
}

/// One scroll or gesture event as it travels on the channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gesture {
    /// A precise scroll.
    Scroll {
        /// Hundredths of a point; positive scrolls towards the left.
        dx: i32,
        /// Hundredths of a point; positive scrolls towards the top.
        dy: i32,
        /// Scroll phase.
        phase: ScrollPhase,
        /// Momentum phase.
        momentum: MomentumPhase,
    },
    /// A multi-finger swipe.
    Swipe {
        /// Which way the fingers moved.
        direction: SwipeDirection,
        /// How many fingers.
        fingers: u8,
    },
    /// A pinch: change in magnification since the last event, in thousandths.
    Pinch {
        /// Thousandths of magnification (positive zooms in).
        magnification: i32,
        /// Gesture phase.
        phase: ScrollPhase,
    },
    /// A smart zoom.
    SmartZoom,
}

const PHASES: [ScrollPhase; 6] = [
    ScrollPhase::None,
    ScrollPhase::MayBegin,
    ScrollPhase::Began,
    ScrollPhase::Changed,
    ScrollPhase::Ended,
    ScrollPhase::Cancelled,
];
const MOMENTA: [MomentumPhase; 4] =
    [MomentumPhase::None, MomentumPhase::Began, MomentumPhase::Changed, MomentumPhase::Ended];
const DIRECTIONS: [SwipeDirection; 4] =
    [SwipeDirection::Left, SwipeDirection::Right, SwipeDirection::Up, SwipeDirection::Down];

fn index<T: PartialEq>(table: &[T], value: T) -> u8 {
    table.iter().position(|v| *v == value).and_then(|i| u8::try_from(i).ok()).unwrap_or(0)
}

fn lookup<T: Copy>(table: &[T], byte: u8, what: &'static str) -> Result<T, WireError> {
    table.get(usize::from(byte)).copied().ok_or(WireError::Malformed(what))
}

fn i32_at(p: &[u8], at: usize) -> i32 {
    i32::from_le_bytes([p[at], p[at + 1], p[at + 2], p[at + 3]])
}

impl Gesture {
    /// The message type it travels as.
    pub const fn kind(&self) -> MessageType {
        match self {
            Self::Scroll { .. } => types::SCROLL,
            Self::Swipe { .. } => types::SWIPE,
            Self::Pinch { .. } => types::PINCH,
            Self::SmartZoom => types::SMART_ZOOM,
        }
    }

    /// The capability both ends must have agreed on to send it.
    pub const fn capability(&self) -> Capabilities {
        match self {
            Self::Scroll { .. } => Capabilities::SCROLL,
            _ => Capabilities::GESTURES,
        }
    }

    /// As a channel message.
    pub fn to_message(&self) -> Message {
        let mut payload = Vec::with_capacity(10);
        match *self {
            Self::Scroll { dx, dy, phase, momentum } => {
                payload.extend(dx.to_le_bytes());
                payload.extend(dy.to_le_bytes());
                payload.push(index(&PHASES, phase));
                payload.push(index(&MOMENTA, momentum));
            }
            Self::Swipe { direction, fingers } => {
                payload.push(index(&DIRECTIONS, direction));
                payload.push(fingers);
            }
            Self::Pinch { magnification, phase } => {
                payload.extend(magnification.to_le_bytes());
                payload.push(index(&PHASES, phase));
            }
            Self::SmartZoom => {}
        }
        Message::Unknown { kind: self.kind(), payload }
    }

    /// The gesture in `message`; `None` when it is not an EXT-2 type, an error when its payload
    /// is malformed.
    pub fn from_message(message: &Message) -> Option<Result<Self, WireError>> {
        let Message::Unknown { kind, payload } = message else { return None };
        let p = payload.as_slice();
        let exact = |len: usize, what: &'static str| {
            if p.len() == len { Ok(()) } else { Err(WireError::Malformed(what)) }
        };
        Some(match *kind {
            types::SCROLL => exact(10, "scroll is 10 bytes").and_then(|()| {
                Ok(Self::Scroll {
                    dx: i32_at(p, 0),
                    dy: i32_at(p, 4),
                    phase: lookup(&PHASES, p[8], "unknown scroll phase")?,
                    momentum: lookup(&MOMENTA, p[9], "unknown momentum phase")?,
                })
            }),
            types::SWIPE => exact(2, "swipe is 2 bytes").and_then(|()| {
                Ok(Self::Swipe {
                    direction: lookup(&DIRECTIONS, p[0], "unknown swipe direction")?,
                    fingers: p[1],
                })
            }),
            types::PINCH => exact(5, "pinch is 5 bytes").and_then(|()| {
                Ok(Self::Pinch {
                    magnification: i32_at(p, 0),
                    phase: lookup(&PHASES, p[4], "unknown pinch phase")?,
                })
            }),
            types::SMART_ZOOM => exact(0, "smart zoom has no payload").map(|()| Self::SmartZoom),
            _ => return None,
        })
    }

    /// The gesture an input event becomes on the channel (`None` for anything else).
    pub fn of_input(event: &InputEvent) -> Option<Self> {
        Some(match *event {
            InputEvent::PreciseScroll { dx, dy, phase, momentum, .. } => {
                Self::Scroll { dx, dy, phase, momentum }
            }
            InputEvent::Swipe { direction, fingers } => Self::Swipe { direction, fingers },
            InputEvent::Pinch { magnification, phase } => Self::Pinch { magnification, phase },
            InputEvent::SmartZoom => Self::SmartZoom,
            _ => return None,
        })
    }
}

/// What a client sends on the channel for `event`, given what was `negotiated`: the gesture when
/// both ends agreed on its capability, else `None` (the event stays plain RDP).
pub fn from_input(event: &InputEvent, negotiated: Option<Negotiated>) -> Option<Gesture> {
    let negotiated = negotiated?;
    Gesture::of_input(event).filter(|g| negotiated.capabilities.contains(g.capability()))
}

/// The host's side: hands every EXT-2 message to `sink` (and consumes it), when both ends agreed
/// on scrolling or gestures. Malformed payloads are dropped with a log line, never fatal.
pub struct GestureResponder {
    sink: Box<dyn FnMut(Gesture) + Send>,
}

impl std::fmt::Debug for GestureResponder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GestureResponder").finish_non_exhaustive()
    }
}

impl GestureResponder {
    /// A responder handing gestures to `sink`.
    pub fn new(sink: impl FnMut(Gesture) + Send + 'static) -> Self {
        Self { sink: Box::new(sink) }
    }
}

impl crate::channel::Responder for GestureResponder {
    fn on_message(&mut self, message: &Message) -> Option<Vec<Message>> {
        match Gesture::from_message(message)? {
            Ok(g) => (self.sink)(g),
            Err(e) => tracing::warn!(error = %e, "dropped a malformed EXT-2 message"),
        }
        Some(Vec::new())
    }
}
