//! Fuzz target: `Drift::Mac::v1` frames (task EXT-1, plan M9-2).
//!
//! Every byte of a frame comes from the other end of the channel. The target decodes one frame,
//! re-encodes what decoded and checks it decodes the same, then feeds a **sequence** of frames
//! (u16-length-prefixed) through a client processor, which must never return an error (that
//! would end the RDP session) and must stay in a sane state. EXT-2: a decoded gesture re-encodes
//! to the same gesture, and a host end with the gesture responder takes the same sequence.
#![no_main]

use drift_ext::channel::{DriftMacClient, DriftMacServer, Link, LinkState};
use drift_ext::gestures::{Gesture, GestureResponder};
use drift_ext::wire::{Capabilities, decode, encode};
use ironrdp_dvc::DvcProcessor;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(message) = decode(data) {
        let bytes = encode(&message).expect("a decoded message re-encodes");
        assert_eq!(decode(&bytes).expect("and decodes again"), message);
        if let Some(Ok(gesture)) = Gesture::from_message(&message) {
            assert_eq!(Gesture::from_message(&gesture.to_message()), Some(Ok(gesture)));
        }
    }

    let mut host = DriftMacServer::new(Link::new(), Capabilities::SCROLL | Capabilities::GESTURES)
        .with_responder(Box::new(GestureResponder::new(|_| {})));
    host.start(1).expect("start never fails");

    let link = Link::new();
    let mut client = DriftMacClient::new(link.clone(), Capabilities::SCROLL);
    client.start(1).expect("start never fails");
    let mut rest = data;
    while rest.len() >= 2 {
        let len = usize::from(u16::from_le_bytes([rest[0], rest[1]]));
        let take = len.min(rest.len() - 2);
        let frame = &rest[2..2 + take];
        assert!(client.process(1, frame).is_ok(), "a bad frame must not end the session");
        assert!(host.process(1, frame).is_ok(), "nor on the host");
        rest = &rest[2 + take..];
    }
    let _ = link.take_inbox();
    assert!(!matches!(link.state(), LinkState::Absent), "the channel was opened");
});
