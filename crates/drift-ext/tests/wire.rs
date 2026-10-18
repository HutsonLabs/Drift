//! EXT-1 Red: `Drift::Mac::v1` framing — versioned, length-delimited, strict.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_ext::wire::{
    Capabilities, HEADER_LEN, Hello, MAGIC, MAX_PAYLOAD, Message, MessageType, VERSION, WireError, decode,
    encode,
};
use proptest::prelude::*;

fn hello() -> Hello {
    Hello {
        min_version: 1,
        max_version: 1,
        capabilities: Capabilities::SCROLL | Capabilities::STATUS,
        agent: "Drift 0.2.0 iPadOS".into(),
    }
}

#[test]
fn the_header_is_magic_version_flags_type_length() {
    let bytes = encode(&Message::Hello(hello())).unwrap();
    assert_eq!(&bytes[..4], &MAGIC);
    assert_eq!(bytes[4], VERSION);
    assert_eq!(bytes[5], 0, "reserved flags");
    assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), MessageType::HELLO.0);
    let len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    assert_eq!(HEADER_LEN, 12);
    assert_eq!(len, bytes.len() - HEADER_LEN);
}

#[test]
fn hello_round_trips() {
    let msg = Message::Hello(hello());
    assert_eq!(decode(&encode(&msg).unwrap()).unwrap(), msg);
}

#[test]
fn unknown_types_are_kept_for_forward_compatibility() {
    let msg = Message::Unknown { kind: MessageType(0x0777), payload: vec![1, 2, 3] };
    assert_eq!(decode(&encode(&msg).unwrap()).unwrap(), msg);
}

#[test]
fn malformed_frames_are_refused_without_panicking() {
    let good = encode(&Message::Hello(hello())).unwrap();
    assert!(matches!(decode(&good[..5]), Err(WireError::Truncated { .. })));
    let mut bad = good.clone();
    bad[0] = b'X';
    assert!(matches!(decode(&bad), Err(WireError::BadMagic)));
    let mut bad = good.clone();
    bad[4] = 9;
    assert!(matches!(decode(&bad), Err(WireError::UnsupportedVersion(9))));
    let mut bad = good.clone();
    bad[5] = 1;
    assert!(matches!(decode(&bad), Err(WireError::ReservedFlags(1))));
    let mut long = good.clone();
    long.push(0);
    assert!(matches!(decode(&long), Err(WireError::LengthMismatch { .. })));
    let short = &good[..good.len() - 1];
    assert!(matches!(decode(short), Err(WireError::LengthMismatch { .. })));
    // A Hello whose agent string claims more bytes than there are.
    let mut lying = good.clone();
    let agent_len_at = HEADER_LEN + 2 + 2 + 4;
    lying[agent_len_at] = 0xFF;
    assert!(matches!(decode(&lying), Err(WireError::Malformed(_))));
}

#[test]
fn oversized_messages_are_refused_both_ways() {
    let big = Message::Unknown { kind: MessageType(0x0777), payload: vec![0; MAX_PAYLOAD + 1] };
    assert!(matches!(encode(&big), Err(WireError::TooLarge(_))));
    let mut header = Vec::from(MAGIC);
    header.extend([VERSION, 0]);
    header.extend(0x0777_u16.to_le_bytes());
    header.extend(u32::try_from(MAX_PAYLOAD + 1).unwrap().to_le_bytes());
    assert!(matches!(decode(&header), Err(WireError::TooLarge(_))));
}

#[test]
fn reserved_ranges_name_the_ext_tasks() {
    assert_eq!(MessageType::HELLO.area(), "negotiation");
    assert_eq!(MessageType(0x0101).area(), "scroll and gestures (EXT-2)");
    assert_eq!(MessageType(0x0201).area(), "menu mirroring (EXT-3)");
    assert_eq!(MessageType(0x0301).area(), "host status and quality (EXT-4)");
    assert_eq!(MessageType(0x0401).area(), "pairing (HOST-8)");
    assert_eq!(MessageType(0x0501).area(), "codec negotiation (EXT-5)");
    assert_eq!(MessageType(0x7000).area(), "unassigned");
}

proptest! {
    #[test]
    fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = decode(&bytes);
    }

    #[test]
    fn unknown_messages_round_trip(kind in 0x0100_u16..0xFFFF, payload in proptest::collection::vec(any::<u8>(), 0..512)) {
        let msg = Message::Unknown { kind: MessageType(kind), payload };
        prop_assert_eq!(decode(&encode(&msg).unwrap()).unwrap(), msg);
    }

    #[test]
    fn hellos_round_trip(min in 1_u16..5, extra in 0_u16..5, caps in any::<u32>(), agent in "[a-zA-Z0-9 .]{0,64}") {
        let msg = Message::Hello(Hello { min_version: min, max_version: min + extra, capabilities: Capabilities::from_bits(caps), agent });
        prop_assert_eq!(decode(&encode(&msg).unwrap()).unwrap(), msg);
    }
}
