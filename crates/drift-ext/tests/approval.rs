//! HOST-15 Red: pairing approved on the Mac instead of a typed code (ADR `HOST-15-guided-setup`).
//! The device asks; the Mac's user clicks Allow or Don't Allow; the host replies when they did.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use drift_ext::pairing::{
    ClientPairing, DeviceSigner, NONCE_LEN, PairStatus, PairingMessage, PairingOutcome, Purpose,
    signed_bytes, types,
};
use drift_ext::wire::{self, Capabilities, Message};
use drift_ext::{Negotiated, negotiate};

const HOST_FP: [u8; 32] = [7; 32];
const NONCE: [u8; NONCE_LEN] = [9; NONCE_LEN];

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

fn negotiated(host: Capabilities) -> Negotiated {
    let hello = |caps| wire::Hello { min_version: 1, max_version: 1, capabilities: caps, agent: "t".into() };
    negotiate(&hello(Capabilities::PAIRING | Capabilities::APPROVAL), &hello(host)).unwrap()
}

fn challenge() -> Message {
    PairingMessage::Challenge { nonce: NONCE }.to_message()
}

#[test]
fn approval_messages_round_trip_in_the_pairing_range() {
    let request = PairingMessage::ApprovalRequest {
        version: 1,
        device_name: "Andrew’s iPad".into(),
        device_key: FakeSigner::default().public_key(),
        signature: vec![1, 2, 3],
    };
    assert_eq!(request.to_message().kind(), types::APPROVAL_REQUEST);
    for m in [
        request,
        PairingMessage::PairReply(PairStatus::Declined),
        PairingMessage::PairReply(PairStatus::NoAnswer),
    ] {
        assert_eq!(&roundtrip(&m), &m);
    }
}

#[test]
fn approval_is_its_own_capability_and_purpose() {
    assert_ne!(Capabilities::APPROVAL, Capabilities::PAIRING);
    assert!(!Capabilities::PAIRING.contains(Capabilities::APPROVAL));
    assert!(ClientPairing::capabilities().contains(Capabilities::APPROVAL));
    let approve = signed_bytes(Purpose::Approve, &NONCE, &HOST_FP);
    assert_ne!(approve, signed_bytes(Purpose::Pair, &NONCE, &HOST_FP));
    assert_ne!(approve, signed_bytes(Purpose::Authenticate, &NONCE, &HOST_FP));
}

#[test]
fn a_client_asking_for_approval_signs_and_waits_for_the_mac() {
    let signer = Arc::new(FakeSigner::default());
    let pairing = ClientPairing::new(signer.clone(), HOST_FP, "iPad").asking_approval();
    assert!(pairing.asks_approval());
    assert_eq!(pairing.code(), None);
    let mut responder = pairing.responder();
    assert!(responder.on_negotiated(&negotiated(Capabilities::PAIRING | Capabilities::APPROVAL)).is_empty());
    assert!(!pairing.awaiting_approval(), "nothing asked before the challenge");

    let out = responder.on_message(&challenge()).expect("answers the challenge");
    match PairingMessage::from_message(&out[0]).unwrap().unwrap() {
        PairingMessage::ApprovalRequest { version, device_name, device_key, .. } => {
            assert_eq!((version, device_name.as_str()), (1, "iPad"));
            assert_eq!(device_key, signer.public_key());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(signer.0.lock().unwrap().as_slice(), [signed_bytes(Purpose::Approve, &NONCE, &HOST_FP)]);
    assert!(pairing.awaiting_approval(), "the UI tells the user to click Allow on the Mac");

    responder.on_message(&PairingMessage::PairReply(PairStatus::Accepted).to_message());
    assert_eq!(pairing.outcome(), Some(PairingOutcome::Paired));
    assert!(!pairing.awaiting_approval());

    // Reconnects prove the key; they never ask again.
    let mut again = pairing.responder();
    let out = again.on_message(&challenge()).unwrap();
    assert!(matches!(
        PairingMessage::from_message(&out[0]).unwrap().unwrap(),
        PairingMessage::AuthRequest { .. }
    ));
}

#[test]
fn declined_and_unanswered_requests_say_so() {
    for (status, words) in [(PairStatus::Declined, "declined"), (PairStatus::NoAnswer, "in time")] {
        let pairing = ClientPairing::new(Arc::new(FakeSigner::default()), HOST_FP, "iPad").asking_approval();
        let mut responder = pairing.responder();
        responder.on_negotiated(&negotiated(Capabilities::PAIRING | Capabilities::APPROVAL));
        responder.on_message(&challenge());
        responder.on_message(&PairingMessage::PairReply(status).to_message());
        assert_eq!(pairing.outcome(), Some(PairingOutcome::PairRefused(status)));
        assert!(pairing.refusal().is_some_and(|why| why.contains(words)), "{:?}", pairing.refusal());
        assert!(!pairing.awaiting_approval());
    }
}

#[test]
fn an_older_host_without_approval_is_told_apart() {
    let pairing = ClientPairing::new(Arc::new(FakeSigner::default()), HOST_FP, "iPad").asking_approval();
    let mut responder = pairing.responder();
    responder.on_negotiated(&negotiated(Capabilities::PAIRING));
    assert_eq!(responder.on_message(&challenge()), Some(Vec::new()), "nothing is sent");
    assert_eq!(pairing.outcome(), Some(PairingOutcome::ApprovalUnsupported));
    assert!(pairing.refusal().is_some_and(|why| why.contains("code")), "{:?}", pairing.refusal());
}

#[test]
fn a_code_pairing_still_wins_over_approval() {
    let pairing = ClientPairing::new(Arc::new(FakeSigner::default()), HOST_FP, "iPad")
        .asking_approval()
        .with_code("12345674");
    assert!(!pairing.asks_approval());
    let mut responder = pairing.responder();
    responder.on_negotiated(&negotiated(Capabilities::PAIRING | Capabilities::APPROVAL));
    let out = responder.on_message(&challenge()).unwrap();
    assert!(matches!(
        PairingMessage::from_message(&out[0]).unwrap().unwrap(),
        PairingMessage::PairRequest { .. }
    ));
}
