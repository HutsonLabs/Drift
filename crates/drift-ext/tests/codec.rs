//! EXT-5 Red: codec negotiation on `Drift::Mac::v1` (ADR `EXT-5-hevc-hdr`). The client offers the
//! HEVC profiles it decodes and the transfer functions its display shows; the host picks one from
//! what its encoder supports and the picture's size, and answers.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use drift_ext::channel::Responder;
use drift_ext::codec::{
    ClientCodec, CodecMessage, CodecOffer, CodecSelect, HevcProfile, HostCodecs, MAX_PROFILES, Transfer,
    select, types,
};
use drift_ext::wire::{self, Capabilities, Message, MessageType};
use drift_ext::{Negotiated, negotiate};

fn roundtrip(m: &CodecMessage) -> CodecMessage {
    let bytes = wire::encode(&m.to_message()).unwrap();
    CodecMessage::from_message(&wire::decode(&bytes).unwrap()).expect("a codec type").unwrap()
}

fn offer(hevc: &[HevcProfile], transfers: &[Transfer]) -> CodecOffer {
    CodecOffer { hevc: hevc.to_vec(), transfers: transfers.to_vec() }
}

fn host(hevc: &[HevcProfile], transfers: &[Transfer]) -> HostCodecs {
    HostCodecs {
        hevc: hevc.to_vec(),
        transfers: transfers.to_vec(),
        max_pixels: 3840 * 2160,
        max_444_pixels: 2560 * 1600,
    }
}

const UHD: (u32, u32) = (3840, 2160);
const QHD: (u32, u32) = (2560, 1440);

#[test]
fn codec_messages_round_trip_in_the_codec_range() {
    let messages = [
        CodecMessage::Offer(offer(
            &[HevcProfile::Main, HevcProfile::Main444],
            &[Transfer::Sdr, Transfer::Pq],
        )),
        CodecMessage::Offer(offer(&[], &[Transfer::Sdr])),
        CodecMessage::Select(CodecSelect { hevc: Some(HevcProfile::Main10), transfer: Transfer::Hlg }),
        CodecMessage::Select(CodecSelect { hevc: None, transfer: Transfer::Sdr }),
    ];
    for m in &messages {
        assert_eq!(&roundtrip(m), m);
        assert_eq!(m.to_message().kind().0 >> 8, 0x05, "{m:?} is in EXT-5's range");
        assert_eq!(m.to_message().kind().area(), "codec negotiation (EXT-5)");
    }
    assert_eq!(types::OFFER, MessageType(0x0501));
    assert_eq!(types::SELECT, MessageType(0x0502));
}

#[test]
fn other_types_are_not_codec_messages() {
    let hello = Message::Unknown { kind: MessageType(0x0401), payload: vec![0; 4] };
    assert!(CodecMessage::from_message(&hello).is_none());
}

#[test]
fn malformed_codec_payloads_are_errors_not_panics() {
    let bad = [
        Message::Unknown { kind: types::OFFER, payload: vec![] },
        Message::Unknown { kind: types::OFFER, payload: vec![1] },
        // Count says two profiles, one present.
        Message::Unknown { kind: types::OFFER, payload: vec![2, 0, 1, 0] },
        // An unknown profile id.
        Message::Unknown { kind: types::OFFER, payload: vec![1, 99, 1, 0] },
        // Too many profiles.
        Message::Unknown {
            kind: types::OFFER,
            payload: [vec![u8::try_from(MAX_PROFILES + 1).unwrap()], vec![0; MAX_PROFILES + 1], vec![1, 0]]
                .concat(),
        },
        Message::Unknown { kind: types::SELECT, payload: vec![] },
        Message::Unknown { kind: types::SELECT, payload: vec![7, 7, 7] },
        // Trailing bytes.
        Message::Unknown { kind: types::SELECT, payload: vec![0, 0, 0, 0] },
    ];
    for m in &bad {
        assert!(matches!(CodecMessage::from_message(m), Some(Err(_))), "{m:?}");
    }
}

#[test]
fn the_host_picks_full_chroma_hevc_when_both_ends_have_it_and_the_picture_fits() {
    let all = [HevcProfile::Main, HevcProfile::Main10, HevcProfile::Main444, HevcProfile::Main44410];
    let h = host(&all, &[Transfer::Sdr]);
    let o = offer(&[HevcProfile::Main, HevcProfile::Main444], &[Transfer::Sdr]);
    assert_eq!(
        select(&o, &h, QHD),
        CodecSelect { hevc: Some(HevcProfile::Main444), transfer: Transfer::Sdr }
    );
    // Too big for 4:4:4 at full rate: 4:2:0.
    assert_eq!(select(&o, &h, UHD), CodecSelect { hevc: Some(HevcProfile::Main), transfer: Transfer::Sdr });
}

#[test]
fn hdr_needs_both_ends_and_a_ten_bit_profile() {
    let all = [HevcProfile::Main, HevcProfile::Main10, HevcProfile::Main444, HevcProfile::Main44410];
    let h = host(&all, &[Transfer::Sdr, Transfer::Pq]);
    // The client shows PQ and decodes Main10: HDR at 4:2:0 10-bit.
    let o = offer(&[HevcProfile::Main, HevcProfile::Main10], &[Transfer::Sdr, Transfer::Pq]);
    assert_eq!(select(&o, &h, UHD), CodecSelect { hevc: Some(HevcProfile::Main10), transfer: Transfer::Pq });
    // 4:4:4 10-bit when it fits.
    let o = offer(&all, &[Transfer::Sdr, Transfer::Pq]);
    assert_eq!(
        select(&o, &h, QHD),
        CodecSelect { hevc: Some(HevcProfile::Main44410), transfer: Transfer::Pq }
    );
    // The host's display is SDR: no HDR whatever the client shows.
    let sdr_host = host(&all, &[Transfer::Sdr]);
    assert_eq!(select(&o, &sdr_host, QHD).transfer, Transfer::Sdr);
    // The client has no 10-bit decoder: SDR.
    let o = offer(&[HevcProfile::Main], &[Transfer::Sdr, Transfer::Pq]);
    assert_eq!(select(&o, &h, UHD), CodecSelect { hevc: Some(HevcProfile::Main), transfer: Transfer::Sdr });
}

#[test]
fn nothing_in_common_means_no_hevc() {
    let h = host(&[HevcProfile::Main], &[Transfer::Sdr]);
    assert_eq!(select(&offer(&[], &[Transfer::Sdr]), &h, QHD).hevc, None);
    assert_eq!(select(&offer(&[HevcProfile::Main444], &[]), &h, QHD).hevc, None);
    let none = host(&[], &[Transfer::Sdr]);
    assert_eq!(select(&offer(&[HevcProfile::Main], &[Transfer::Sdr]), &none, QHD).hevc, None);
    // Over the host's largest picture: no HEVC (the AVC path decides).
    let small = HostCodecs { max_pixels: 1920 * 1080, ..host(&[HevcProfile::Main], &[Transfer::Sdr]) };
    assert_eq!(select(&offer(&[HevcProfile::Main], &[Transfer::Sdr]), &small, UHD).hevc, None);
}

fn negotiated(capabilities: Capabilities) -> Negotiated {
    let hello = wire::Hello { min_version: 1, max_version: 1, capabilities, agent: "test".into() };
    negotiate(&hello, &hello).unwrap()
}

#[test]
fn the_client_offers_once_the_codec_capability_is_agreed_and_keeps_the_answer() {
    let client = ClientCodec::new(offer(&[HevcProfile::Main, HevcProfile::Main444], &[Transfer::Sdr]));
    let mut responder = client.responder();

    assert!(responder.on_negotiated(&negotiated(Capabilities::PAIRING)).is_empty(), "no CODEC, no offer");
    let sent = responder.on_negotiated(&negotiated(Capabilities::CODEC | Capabilities::PAIRING));
    assert_eq!(sent.len(), 1);
    assert_eq!(
        CodecMessage::from_message(&sent[0]).unwrap().unwrap(),
        CodecMessage::Offer(offer(&[HevcProfile::Main, HevcProfile::Main444], &[Transfer::Sdr]))
    );
    assert_eq!(client.selection(), None);

    let answer = CodecSelect { hevc: Some(HevcProfile::Main444), transfer: Transfer::Sdr };
    let replies = responder.on_message(&CodecMessage::Select(answer).to_message());
    assert_eq!(replies, Some(vec![]), "the answer is consumed");
    assert_eq!(client.selection(), Some(answer));

    // Not a codec message: left for the inbox.
    let other = Message::Unknown { kind: MessageType(0x0301), payload: vec![] };
    assert_eq!(responder.on_message(&other), None);
}

#[test]
fn capability_sets_have_a_codec_bit() {
    assert!(ClientCodec::capabilities().contains(Capabilities::CODEC));
}
