//! HOST-15 Red: pairing approved on the Mac (ADR `HOST-15-guided-setup`). A device that found the
//! Mac with Bonjour asks; the request waits for the Mac's user; Allow pairs the device and Don't
//! Allow, a timeout or a lockout refuse it. The reply is pushed when the user decided.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_apple::pairing::DeviceKey;
use drift_ext::channel::Responder;
use drift_ext::pairing::{ClientPairing, PairStatus, PairingMessage, PairingOutcome};
use drift_ext::wire::{self, Capabilities, Message};
use drift_ext::{Negotiated, negotiate};
use drift_host::pairing::{
    APPROVAL_TIMEOUT, AuthPhase, ConnectionAuth, DeviceStore, MAX_WRONG_CODES, Pairing, ReplySender,
};
use drift_testkit::ManualClock;

const HOST_FP: [u8; 32] = [3; 32];

fn negotiated() -> Negotiated {
    let caps = Capabilities::PAIRING | Capabilities::APPROVAL;
    let hello = |c| wire::Hello { min_version: 1, max_version: 1, capabilities: c, agent: "t".into() };
    negotiate(&hello(caps), &hello(caps)).unwrap()
}

/// One connection: the host's responder, the device's, and what the host pushed later.
struct Exchange {
    auth: ConnectionAuth,
    pushed: Arc<Mutex<Vec<Message>>>,
    host: Box<dyn Responder>,
    device: ClientPairing,
}

impl Exchange {
    fn new(pairing: &Pairing, name: &str) -> Self {
        let auth = ConnectionAuth::pending();
        let pushed = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&pushed);
        let reply: ReplySender = Arc::new(move |m: Message| sink.lock().unwrap().push(m));
        let host = pairing.responder(HOST_FP, auth.clone(), reply);
        let device =
            ClientPairing::new(Arc::new(DeviceKey::generate().unwrap()), HOST_FP, name).asking_approval();
        Self { auth, pushed, host, device }
    }

    /// Runs the challenge and the device's request; returns the host's immediate replies.
    fn ask(&mut self) -> Vec<Message> {
        let mut device = self.device.responder();
        device.on_negotiated(&negotiated());
        let challenge = self.host.on_negotiated(&negotiated());
        assert_eq!(challenge.len(), 1);
        let request = device.on_message(&challenge[0]).unwrap();
        assert!(matches!(
            PairingMessage::from_message(&request[0]).unwrap().unwrap(),
            PairingMessage::ApprovalRequest { .. }
        ));
        self.host.on_message(&request[0]).unwrap()
    }

    fn pushed(&self) -> Vec<PairingMessage> {
        self.pushed
            .lock()
            .unwrap()
            .iter()
            .map(|m| PairingMessage::from_message(m).unwrap().unwrap())
            .collect()
    }
}

#[test]
fn a_request_waits_for_the_mac_and_allow_pairs_the_device() {
    let clock = Arc::new(ManualClock::new());
    let pairing = Pairing::new(DeviceStore::memory(), clock.clone());
    let mut x = Exchange::new(&pairing, "Andrew’s iPad");
    assert!(x.ask().is_empty(), "no answer until the Mac's user decides");
    let AuthPhase::AwaitingApproval(id) = x.auth.phase() else { panic!("{:?}", x.auth.phase()) };
    assert!(!x.auth.is_authorized(), "nothing is shown while waiting");

    let waiting = pairing.approvals();
    assert_eq!(waiting.len(), 1);
    assert_eq!((waiting[0].id, waiting[0].device_name.as_str()), (id, "Andrew’s iPad"));

    assert!(pairing.approve(id));
    assert_eq!(x.pushed(), [PairingMessage::PairReply(PairStatus::Accepted)]);
    assert!(x.auth.is_authorized());
    assert_eq!(pairing.devices().len(), 1);
    assert_eq!(pairing.devices()[0].name, "Andrew’s iPad");
    assert!(pairing.approvals().is_empty());
    assert!(!pairing.approve(id), "a request is answered once");
}

#[test]
fn dont_allow_and_no_answer_refuse_the_device() {
    let clock = Arc::new(ManualClock::new());
    let pairing = Pairing::new(DeviceStore::memory(), clock.clone());

    let mut x = Exchange::new(&pairing, "Stranger");
    x.ask();
    let AuthPhase::AwaitingApproval(id) = x.auth.phase() else { panic!() };
    assert!(pairing.decline(id));
    assert_eq!(x.pushed(), [PairingMessage::PairReply(PairStatus::Declined)]);
    assert_eq!(x.auth.phase(), AuthPhase::Refused);

    let mut y = Exchange::new(&pairing, "Forgotten");
    y.ask();
    let AuthPhase::AwaitingApproval(id) = y.auth.phase() else { panic!() };
    clock.advance(APPROVAL_TIMEOUT + Duration::from_secs(1));
    assert!(pairing.approvals().is_empty(), "an old request is no longer offered");
    assert!(pairing.give_up(id));
    assert_eq!(y.pushed(), [PairingMessage::PairReply(PairStatus::NoAnswer)]);
    assert_eq!(y.auth.phase(), AuthPhase::Refused);
    assert!(!pairing.approve(id), "too late");
    assert!(pairing.devices().is_empty());
}

#[test]
fn repeated_dont_allows_lock_requests_for_a_while() {
    let clock = Arc::new(ManualClock::new());
    let pairing = Pairing::new(DeviceStore::memory(), clock.clone());
    for _ in 0..MAX_WRONG_CODES {
        let mut x = Exchange::new(&pairing, "Pest");
        x.ask();
        let AuthPhase::AwaitingApproval(id) = x.auth.phase() else { panic!() };
        pairing.decline(id);
    }
    let mut x = Exchange::new(&pairing, "Pest");
    let replies = x.ask();
    assert_eq!(
        replies.iter().map(|m| PairingMessage::from_message(m).unwrap().unwrap()).collect::<Vec<_>>(),
        [PairingMessage::PairReply(PairStatus::TooManyAttempts)],
        "no prompt while locked"
    );
    assert!(pairing.approvals().is_empty());
    assert_eq!(x.auth.phase(), AuthPhase::Refused);
}

#[test]
fn a_forged_request_never_reaches_the_mac() {
    let pairing = Pairing::new(DeviceStore::memory(), Arc::new(ManualClock::new()));
    let auth = ConnectionAuth::pending();
    let mut host = pairing.responder(HOST_FP, auth.clone(), Arc::new(|_| {}));
    host.on_negotiated(&negotiated());
    let forged = PairingMessage::ApprovalRequest {
        version: 1,
        device_name: "Forger".into(),
        device_key: DeviceKey::generate().unwrap().public_key().to_vec(),
        signature: vec![0x30, 0x02, 0x01, 0x01],
    };
    let replies = host.on_message(&forged.to_message()).unwrap();
    assert_eq!(
        PairingMessage::from_message(&replies[0]).unwrap().unwrap(),
        PairingMessage::PairReply(PairStatus::Declined)
    );
    assert!(pairing.approvals().is_empty());
    assert_eq!(auth.phase(), AuthPhase::Refused);
}

#[test]
fn the_client_outcome_follows_the_pushed_reply() {
    let pairing = Pairing::new(DeviceStore::memory(), Arc::new(ManualClock::new()));
    let mut x = Exchange::new(&pairing, "iPad");
    let mut device = x.device.responder();
    device.on_negotiated(&negotiated());
    let challenge = x.host.on_negotiated(&negotiated());
    let request = device.on_message(&challenge[0]).unwrap();
    x.host.on_message(&request[0]);
    assert!(x.device.awaiting_approval());
    let AuthPhase::AwaitingApproval(id) = x.auth.phase() else { panic!() };
    pairing.approve(id);
    let reply = x.pushed.lock().unwrap().remove(0);
    device.on_message(&reply);
    assert_eq!(x.device.outcome(), Some(PairingOutcome::Paired));
}
