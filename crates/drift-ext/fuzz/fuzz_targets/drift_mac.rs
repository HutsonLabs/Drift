//! Fuzz target: `Drift::Mac::v1` frames (task EXT-1, plan M9-2).
//!
//! Every byte of a frame comes from the other end of the channel. The target decodes one frame,
//! re-encodes what decoded and checks it decodes the same, then feeds a **sequence** of frames
//! (u16-length-prefixed) through a client processor, which must never return an error (that
//! would end the RDP session) and must stay in a sane state. Menu messages (EXT-3) also go
//! through their own decoder, which must round-trip, and through the client's mirror. EXT-2: a
//! decoded gesture re-encodes to the same gesture, and a host end with the gesture responder
//! takes the same sequence.
#![no_main]

use drift_ext::channel::{DriftMacClient, DriftMacServer, Link, LinkState};
use drift_ext::gestures::{Gesture, GestureResponder};
use drift_ext::menus::{MenuMessage, MenuMirror};
use drift_ext::codec::{ClientCodec, CodecMessage, CodecOffer};
use drift_ext::wire::{Capabilities, decode, encode};
use ironrdp_dvc::DvcProcessor;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(message) = decode(data) {
        let bytes = encode(&message).expect("a decoded message re-encodes");
        assert_eq!(decode(&bytes).expect("and decodes again"), message);
        if let Some(Ok(menu)) = MenuMessage::from_message(&message) {
            let again = menu.try_to_message().expect("a decoded menu message re-encodes");
            assert_eq!(MenuMessage::from_message(&again).expect("still a menu message").expect("decodes"), menu);
        }
        if let Some(Ok(gesture)) = Gesture::from_message(&message) {
            assert_eq!(Gesture::from_message(&gesture.to_message()), Some(Ok(gesture)));
        }
        // EXT-5: codec messages decode strictly and round-trip.
        if let Some(Ok(codec)) = CodecMessage::from_message(&message) {
            let again = decode(&encode(&codec.to_message()).expect("re-encodes")).expect("decodes");
            assert_eq!(CodecMessage::from_message(&again).expect("still codec").expect("still valid"), codec);
        }
    }

    let mut host = DriftMacServer::new(Link::new(), Capabilities::SCROLL | Capabilities::GESTURES)
        .with_responder(Box::new(GestureResponder::new(|_| {})));
    host.start(1).expect("start never fails");

    let link = Link::new();
    let codec = ClientCodec::new(CodecOffer::default());
    let mut client =
        DriftMacClient::new(link.clone(), Capabilities::SCROLL | Capabilities::MENUS | Capabilities::CODEC)
            .with_responder(codec.responder());
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
    let mut mirror = MenuMirror::default();
    for message in link.take_inbox() {
        let _ = mirror.on_message(&message);
    }
    assert!(!matches!(link.state(), LinkState::Absent), "the channel was opened");
});
