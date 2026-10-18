//! EXT-2 Red: scroll with phase and momentum, and trackpad gestures, on `Drift::Mac::v1`.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_core::{InputEvent, MomentumPhase, ScrollPhase, SwipeDirection};
use drift_ext::Negotiated;
use drift_ext::gestures::{Gesture, from_input, types};
use drift_ext::wire::{Capabilities, Message, MessageType, decode, encode};
use proptest::prelude::*;

fn all() -> Vec<Gesture> {
    vec![
        Gesture::Scroll { dx: -1250, dy: 300, phase: ScrollPhase::Changed, momentum: MomentumPhase::None },
        Gesture::Scroll { dx: 0, dy: 4, phase: ScrollPhase::None, momentum: MomentumPhase::Changed },
        Gesture::Swipe { direction: SwipeDirection::Up, fingers: 3 },
        Gesture::Swipe { direction: SwipeDirection::Left, fingers: 4 },
        Gesture::Pinch { magnification: -120, phase: ScrollPhase::Began },
        Gesture::SmartZoom,
    ]
}

#[test]
fn every_gesture_round_trips_through_the_wire() {
    for g in all() {
        let msg = g.to_message();
        assert_eq!(msg.kind().0 >> 8, 0x01, "EXT-2's range: {:?}", msg.kind());
        let back = decode(&encode(&msg).unwrap()).unwrap();
        assert_eq!(Gesture::from_message(&back), Some(Ok(g)));
    }
}

#[test]
fn types_are_in_the_ext2_range() {
    for t in [types::SCROLL, types::SWIPE, types::PINCH, types::SMART_ZOOM] {
        assert_eq!(t.area(), "scroll and gestures (EXT-2)");
    }
}

#[test]
fn other_messages_are_not_gestures() {
    let other = Message::Unknown { kind: MessageType(0x0401), payload: vec![0; 32] };
    assert_eq!(Gesture::from_message(&other), None);
}

#[test]
fn malformed_payloads_are_errors_not_panics() {
    for (kind, payload) in [
        (types::SCROLL, vec![1, 2, 3]),
        (types::SWIPE, vec![9, 3]),          // unknown direction
        (types::SWIPE, vec![0]),             // short
        (types::PINCH, vec![0, 0, 0, 0, 7]), // unknown phase
        (types::SMART_ZOOM, vec![1]),        // must be empty
    ] {
        let msg = Message::Unknown { kind, payload };
        assert!(matches!(Gesture::from_message(&msg), Some(Err(_))), "{msg:?}");
    }
}

fn negotiated(caps: Capabilities) -> Option<Negotiated> {
    Some(Negotiated { version: 1, capabilities: caps })
}

#[test]
fn input_goes_over_the_channel_only_when_the_capability_was_agreed() {
    let scroll = InputEvent::PreciseScroll {
        dx: 0,
        dy: 1000,
        phase: ScrollPhase::Changed,
        momentum: MomentumPhase::None,
        wheel_v: 20,
        wheel_h: 0,
    };
    let swipe = InputEvent::Swipe { direction: SwipeDirection::Right, fingers: 3 };
    assert_eq!(from_input(&scroll, None), None, "plain RDP: the wheel fallback goes instead");
    assert_eq!(from_input(&scroll, negotiated(Capabilities::GESTURES)), None);
    assert_eq!(
        from_input(&scroll, negotiated(Capabilities::SCROLL)),
        Some(Gesture::Scroll { dx: 0, dy: 1000, phase: ScrollPhase::Changed, momentum: MomentumPhase::None })
    );
    assert_eq!(from_input(&swipe, negotiated(Capabilities::SCROLL)), None);
    assert_eq!(
        from_input(&swipe, negotiated(Capabilities::GESTURES)),
        Some(Gesture::Swipe { direction: SwipeDirection::Right, fingers: 3 })
    );
    let key = InputEvent::Key { scancode: 0x1E, extended: false, down: true };
    assert_eq!(from_input(&key, negotiated(Capabilities::SCROLL | Capabilities::GESTURES)), None);
}

proptest! {
    #[test]
    fn decoding_any_payload_never_panics(kind in 0x0100u16..0x0110, payload in proptest::collection::vec(any::<u8>(), 0..40)) {
        let _ = Gesture::from_message(&Message::Unknown { kind: MessageType(kind), payload });
    }

    #[test]
    fn scrolls_round_trip(dx in any::<i32>(), dy in any::<i32>(), p in 0u8..6, m in 0u8..4) {
        let phase = [ScrollPhase::None, ScrollPhase::MayBegin, ScrollPhase::Began, ScrollPhase::Changed, ScrollPhase::Ended, ScrollPhase::Cancelled][p as usize];
        let momentum = [MomentumPhase::None, MomentumPhase::Began, MomentumPhase::Changed, MomentumPhase::Ended][m as usize];
        let g = Gesture::Scroll { dx, dy, phase, momentum };
        prop_assert_eq!(Gesture::from_message(&g.to_message()), Some(Ok(g)));
    }
}
