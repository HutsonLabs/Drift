//! HOST-8 Red: the pairing and device-authentication messages on `Drift::Mac::v1`, and the
//! client's side of the exchange (ADR `HOST-8-pairing`).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use drift_ext::pairing::{
    AuthStatus, ClientPairing, DeviceSigner, NONCE_LEN, PairStatus, PairingMessage, PairingOutcome, Purpose,
    signed_bytes,
};
use drift_ext::wire::{self, Capabilities, Message, MessageType};
use drift_ext::{Negotiated, negotiate};

const HOST_FP: [u8; 32] = [7; 32];
const NONCE: [u8; NONCE_LEN] = [9; NONCE_LEN];

/// A signer that records what it signed and returns a fixed "signature".
#[derive(Debug, Default)]
struct FakeSigner(Mutex<Vec<Vec<u8>>>);

impl DeviceSigner for FakeSigner {
    fn public_key(&self) -> Vec<u8> {
        let mut key = vec![4_u8];
        key.extend([1_u8; 64]);
        key
    }

    fn sign(&self, message: &[u8]) -> Option<Vec<u8>> {
        self.0.lock().unwrap().push(message.to_vec());
        Some(vec![0x30, 0x02, 0x01, 0x01])
    }
}

fn roundtrip(m: &PairingMessage) -> PairingMessage {
    let bytes = wire::encode(&m.to_message()).unwrap();
    let decoded = wire::decode(&bytes).unwrap();
    PairingMessage::from_message(&decoded).expect("a pairing type").unwrap()
}

#[test]
fn every_pairing_message_round_trips_in_the_pairing_range() {
    let messages = [
        PairingMessage::Challenge { nonce: NONCE },
        PairingMessage::PairRequest {
            version: 1,
            code: "12345674".into(),
            device_name: "Andrew’s iPad".into(),
            device_key: FakeSigner::default().public_key(),
            signature: vec![1, 2, 3],
        },
        PairingMessage::AuthRequest { device_key: FakeSigner::default().public_key(), signature: vec![4, 5] },
        PairingMessage::PairReply(PairStatus::Accepted),
        PairingMessage::PairReply(PairStatus::WrongCode),
        PairingMessage::PairReply(PairStatus::Expired),
        PairingMessage::PairReply(PairStatus::TooManyAttempts),
        PairingMessage::PairReply(PairStatus::NotPairing),
        PairingMessage::AuthReply(AuthStatus::Ok),
        PairingMessage::AuthReply(AuthStatus::UnknownDevice),
        PairingMessage::AuthReply(AuthStatus::BadSignature),
    ];
    for m in &messages {
        assert_eq!(m.to_message().kind().area(), "pairing (HOST-8)", "{m:?}");
        assert_eq!(&roundtrip(m), m);
    }
}

#[test]
fn malformed_pairing_payloads_are_errors_not_panics() {
    for (kind, payload) in [
        (0x0401, vec![1, 2, 3]),         // short nonce
        (0x0402, vec![1, 0, 0, 0, 200]), // code longer than the payload
        (0x0403, vec![0xFF, 0xFF]),      // key length beyond the payload
        (0x0404, vec![]),                // no status
        (0x0404, vec![9]),               // unknown status
        (0x0405, vec![0, 0]),            // trailing byte
    ] {
        let m = Message::Unknown { kind: MessageType(kind), payload };
        assert!(PairingMessage::from_message(&m).is_some_and(|r| r.is_err()), "{kind:#06x}");
    }
    let other = Message::Unknown { kind: MessageType(0x0101), payload: vec![] };
    assert!(PairingMessage::from_message(&other).is_none(), "not a pairing type");
}

#[test]
fn signed_bytes_bind_the_purpose_the_nonce_and_the_host_certificate() {
    let pair = signed_bytes(Purpose::Pair, &NONCE, &HOST_FP);
    let auth = signed_bytes(Purpose::Authenticate, &NONCE, &HOST_FP);
    assert_ne!(pair, auth);
    assert_ne!(pair, signed_bytes(Purpose::Pair, &[1; NONCE_LEN], &HOST_FP));
    assert_ne!(pair, signed_bytes(Purpose::Pair, &NONCE, &[8; 32]));
    assert!(pair.ends_with(&HOST_FP));
}

fn negotiated() -> Negotiated {
    let hello = |caps| wire::Hello { min_version: 1, max_version: 1, capabilities: caps, agent: "t".into() };
    negotiate(&hello(Capabilities::PAIRING), &hello(Capabilities::PAIRING)).unwrap()
}

fn challenge() -> Message {
    PairingMessage::Challenge { nonce: NONCE }.to_message()
}

fn only_reply(out: Option<Vec<Message>>) -> PairingMessage {
    let out = out.expect("the client answers a challenge");
    assert_eq!(out.len(), 1);
    PairingMessage::from_message(&out[0]).unwrap().unwrap()
}

#[test]
fn a_client_with_a_code_pairs_and_signs_the_challenge() {
    let signer = Arc::new(FakeSigner::default());
    let pairing = ClientPairing::new(signer.clone(), HOST_FP, "iPad").with_code("12345674");
    let mut responder = pairing.responder();
    assert!(responder.on_negotiated(&negotiated()).is_empty(), "the host speaks first");
    match only_reply(responder.on_message(&challenge())) {
        PairingMessage::PairRequest { version, code, device_name, device_key, signature } => {
            assert_eq!((version, code.as_str(), device_name.as_str()), (1, "12345674", "iPad"));
            assert_eq!(device_key, signer.public_key());
            assert!(!signature.is_empty());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(signer.0.lock().unwrap().as_slice(), [signed_bytes(Purpose::Pair, &NONCE, &HOST_FP)]);
    assert_eq!(pairing.outcome(), None);

    let accepted = PairingMessage::PairReply(PairStatus::Accepted).to_message();
    assert_eq!(responder.on_message(&accepted), Some(Vec::new()), "a reply is consumed, not queued");
    assert_eq!(pairing.outcome(), Some(PairingOutcome::Paired));

    // The next connection (a reconnect) authenticates instead of sending the code again.
    let mut again = pairing.responder();
    assert!(matches!(only_reply(again.on_message(&challenge())), PairingMessage::AuthRequest { .. }));
}

#[test]
fn a_paired_client_authenticates_and_records_refusals() {
    let signer = Arc::new(FakeSigner::default());
    let pairing = ClientPairing::new(signer.clone(), HOST_FP, "iPhone");
    let mut responder = pairing.responder();
    assert!(matches!(only_reply(responder.on_message(&challenge())), PairingMessage::AuthRequest { .. }));
    assert_eq!(
        signer.0.lock().unwrap().last().unwrap(),
        &signed_bytes(Purpose::Authenticate, &NONCE, &HOST_FP)
    );

    responder.on_message(&PairingMessage::AuthReply(AuthStatus::UnknownDevice).to_message());
    assert_eq!(pairing.outcome(), Some(PairingOutcome::AuthRefused(AuthStatus::UnknownDevice)));
    assert!(pairing.refusal().is_some_and(|why| why.contains("not paired")), "{:?}", pairing.refusal());

    let coded = ClientPairing::new(signer, HOST_FP, "iPhone").with_code("12345674");
    let mut responder = coded.responder();
    responder.on_message(&challenge());
    responder.on_message(&PairingMessage::PairReply(PairStatus::WrongCode).to_message());
    assert_eq!(coded.outcome(), Some(PairingOutcome::PairRefused(PairStatus::WrongCode)));
    assert!(coded.refusal().is_some_and(|why| why.contains("did not accept the code")));
}

#[test]
fn other_extension_messages_pass_through_to_the_inbox() {
    let pairing = ClientPairing::new(Arc::new(FakeSigner::default()), HOST_FP, "iPad");
    let mut responder = pairing.responder();
    let scroll = Message::Unknown { kind: MessageType(0x0101), payload: vec![1] };
    assert_eq!(responder.on_message(&scroll), None);
}
