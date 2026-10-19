//! HOST-15 Red: pairing a Mac picked from the Bonjour list without a code; the Mac's user
//! approves it on the first connection (ADR `HOST-15-guided-setup`). Memory stores; test values
//! only.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use drift_app::pairing::{MemoryPairingStore, PairHostRequest, PairingService, PairingStore as _};
use drift_app::profiles::{CommandError, ProfileFile, ProfileService};
use drift_app::secrets::MemorySecretStore;
use drift_core::CertFingerprint;
use drift_core::pairing::HostAdvert;
use drift_ext::pairing::{PairStatus, PairingMessage, PairingOutcome};
use uuid::Uuid;

const FP: CertFingerprint = CertFingerprint::from_bytes([0xcd; 32]);

fn service() -> (tempfile::TempDir, Arc<MemoryPairingStore>, PairingService) {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecretStore::new());
    let profiles =
        Arc::new(ProfileService::open(ProfileFile::in_dir(&dir.path().join("config")), secrets).unwrap());
    let store = Arc::new(MemoryPairingStore::new());
    let service = PairingService::new(profiles, store.clone());
    (dir, store, service)
}

fn advert(approval: bool) -> HostAdvert {
    HostAdvert {
        service_name: "Studio".into(),
        host_id: Uuid::from_u128(0x88),
        name: "Studio".into(),
        host: "studio.local".into(),
        port: 3389,
        fingerprint: FP,
        account: Some("andrew".into()),
        approval,
    }
}

#[test]
fn a_discovered_mac_pairs_by_approval_then_authenticates() {
    let (_dir, store, service) = service();
    let entry = service
        .pair(PairHostRequest::Approve { advert: advert(true), username: "andrew".into(), password: None })
        .unwrap();
    assert_eq!(entry.profile.paired_host, Some(Uuid::from_u128(0x88)));
    assert_eq!(entry.profile.cert_pin, Some(FP));
    assert_eq!(entry.profile.rdp_username, "andrew");
    assert_eq!(store.host_pin(Uuid::from_u128(0x88)).unwrap(), Some(FP));

    let first = service.client_pairing(entry.profile.id).unwrap().unwrap();
    assert!(first.asks_approval(), "the first connection asks the Mac");
    assert_eq!(first.code(), None);

    let mut responder = first.responder();
    responder.on_message(&PairingMessage::Challenge { nonce: [2; 32] }.to_message());
    responder.on_message(&PairingMessage::PairReply(PairStatus::Accepted).to_message());
    assert_eq!(first.outcome(), Some(PairingOutcome::Paired));
    let next = service.client_pairing(entry.profile.id).unwrap().unwrap();
    assert!(!next.asks_approval(), "afterwards the device key is enough");
}

#[test]
fn a_declined_device_asks_again_next_time() {
    let (_dir, _store, service) = service();
    let entry = service
        .pair(PairHostRequest::Approve { advert: advert(true), username: "andrew".into(), password: None })
        .unwrap();
    let first = service.client_pairing(entry.profile.id).unwrap().unwrap();
    let mut responder = first.responder();
    responder.on_message(&PairingMessage::Challenge { nonce: [2; 32] }.to_message());
    responder.on_message(&PairingMessage::PairReply(PairStatus::Declined).to_message());
    assert!(service.client_pairing(entry.profile.id).unwrap().unwrap().asks_approval());
}

#[test]
fn an_older_mac_still_needs_its_code() {
    let (_dir, _store, service) = service();
    let r = service.pair(PairHostRequest::Approve {
        advert: advert(false),
        username: "a".into(),
        password: None,
    });
    assert!(
        matches!(&r, Err(CommandError::Pairing { message }) if message.contains("code")),
        "{:?}",
        r.map(|e| e.profile.id)
    );
}
