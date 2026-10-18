//! IOS-9 Red: pairing a Mac host from the Connections page (ADR IOS-9). The service turns a
//! scanned QR payload, or a Bonjour advert plus a typed code, into a pinned, paired profile,
//! keeps the host's pin and the device key in the pairing store, and refuses to connect a
//! paired profile whose pin no longer matches. Memory stores; test values only.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use drift_app::pairing::{
    MemoryPairingStore, PairHostRequest, PairingInput, PairingService, PairingStore as _,
};
use drift_app::profiles::{CommandError, ProfileFile, ProfileService};
use drift_app::secrets::{MemorySecretStore, SecretStore as _};
use drift_core::pairing::{HostAdvert, PairingPayload, PairingTarget, ShortCode};
use drift_core::{CertFingerprint, SecretRole};
use uuid::Uuid;

const FP: CertFingerprint = CertFingerprint::from_bytes([0xab; 32]);

struct Fixture {
    _dir: tempfile::TempDir,
    profiles: Arc<ProfileService>,
    secrets: Arc<MemorySecretStore>,
    store: Arc<MemoryPairingStore>,
    service: PairingService,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecretStore::new());
    let profiles = Arc::new(
        ProfileService::open(ProfileFile::in_dir(&dir.path().join("config")), secrets.clone()).unwrap(),
    );
    let store = Arc::new(MemoryPairingStore::new());
    let service = PairingService::new(profiles.clone(), store.clone());
    Fixture { _dir: dir, profiles, secrets, store, service }
}

fn target() -> PairingTarget {
    PairingTarget {
        host_id: Uuid::from_u128(0x77),
        name: "Studio Mac".into(),
        host: "studio.local".into(),
        port: 3389,
        fingerprint: FP,
        code: ShortCode::from_body(7_654_321).unwrap(),
    }
}

fn advert() -> HostAdvert {
    let t = target();
    HostAdvert {
        service_name: "Studio Mac".into(),
        host_id: t.host_id,
        name: t.name,
        host: t.host,
        port: t.port,
        fingerprint: t.fingerprint,
    }
}

#[test]
fn reading_a_code_recognises_qr_payloads_and_typed_codes() {
    let uri = PairingPayload::from(target()).to_uri();
    match PairingService::read_code(&uri).unwrap() {
        PairingInput::Qr { name, host, .. } => {
            assert_eq!((name.as_str(), host.as_str()), ("Studio Mac", "studio.local"))
        }
        other => panic!("expected a QR payload, got {other:?}"),
    }
    let typed = target().code.to_string();
    assert_eq!(PairingService::read_code(&typed).unwrap(), PairingInput::Code { code: typed.clone() });
    assert!(matches!(PairingService::read_code("1234 5679"), Err(CommandError::Pairing { .. })));
}

#[test]
fn pairing_from_a_qr_code_saves_a_pinned_paired_profile() {
    let f = fixture();
    let request = PairHostRequest::Qr {
        uri: PairingPayload::from(target()).to_uri(),
        username: "mac-user".into(),
        password: Some("fake-pw".into()),
    };
    let entry = f.service.pair(request).unwrap();
    let p = &entry.profile;
    assert_eq!(p.cert_pin, Some(FP));
    assert_eq!(p.paired_host, Some(target().host_id));
    assert_eq!(p.rdp_username, "mac-user");
    assert!(entry.has_rdp_password);
    assert_eq!(
        f.secrets.get(p.id, SecretRole::RdpUser).unwrap().as_deref().map(String::as_str),
        Some("fake-pw")
    );
    assert_eq!(f.store.host_pin(target().host_id).unwrap(), Some(FP));
    assert!(f.store.device_public_key().is_ok(), "the device key exists once paired");
    assert_eq!(f.profiles.list().unwrap().len(), 1);
}

#[test]
fn pairing_from_bonjour_needs_the_typed_code() {
    let f = fixture();
    let ok = PairHostRequest::Advert {
        advert: advert(),
        code: target().code.to_string(),
        username: "mac-user".into(),
        password: None,
    };
    let entry = f.service.pair(ok).unwrap();
    assert_eq!(entry.profile.paired_host, Some(target().host_id));
    assert!(!entry.has_rdp_password);

    let typo = PairHostRequest::Advert {
        advert: advert(),
        code: "7654 3210".into(),
        username: "u".into(),
        password: None,
    };
    assert!(matches!(f.service.pair(typo), Err(CommandError::Pairing { .. })));
}

#[test]
fn pairing_the_same_host_again_updates_its_profile() {
    let f = fixture();
    let req = |fp: CertFingerprint| PairHostRequest::Qr {
        uri: PairingPayload::from(PairingTarget { fingerprint: fp, ..target() }).to_uri(),
        username: "mac-user".into(),
        password: None,
    };
    let first = f.service.pair(req(FP)).unwrap();
    let new_fp = CertFingerprint::from_bytes([0xcd; 32]);
    let second = f.service.pair(req(new_fp)).unwrap();
    assert_eq!(first.profile.id, second.profile.id);
    assert_eq!(second.profile.cert_pin, Some(new_fp));
    assert_eq!(f.profiles.list().unwrap().len(), 1);
    assert_eq!(f.store.host_pin(target().host_id).unwrap(), Some(new_fp));
}

#[test]
fn a_paired_profile_connects_only_while_both_pins_agree() {
    let f = fixture();
    let entry = f
        .service
        .pair(PairHostRequest::Qr {
            uri: PairingPayload::from(target()).to_uri(),
            username: "u".into(),
            password: None,
        })
        .unwrap();
    f.service.check_before_connect(entry.profile.id).unwrap();
    f.store.forget_host(target().host_id).unwrap();
    assert!(matches!(f.service.check_before_connect(entry.profile.id), Err(CommandError::Pairing { .. })));
}

#[test]
fn gnome_profiles_are_not_affected() {
    let f = fixture();
    let mut gnome =
        drift_core::ConnectionProfile::new("Homelab", "gnome.local", drift_core::ConnectMode::Headless);
    gnome.rdp_username = "u".into();
    let saved = f
        .profiles
        .save(
            gnome,
            drift_app::profiles::SecretsUpdate {
                rdp_password: None,
                linux_password: drift_app::profiles::LinuxPasswordUpdate::Keep,
            },
        )
        .unwrap();
    f.service.check_before_connect(saved.profile.id).unwrap();
}

#[test]
fn forgetting_a_paired_hosts_certificate_is_refused() {
    let f = fixture();
    let entry = f
        .service
        .pair(PairHostRequest::Qr {
            uri: PairingPayload::from(target()).to_uri(),
            username: "u".into(),
            password: None,
        })
        .unwrap();
    assert!(f.profiles.set_pin(entry.profile.id, None).is_err());
    assert_eq!(f.profiles.get(entry.profile.id).unwrap().profile.cert_pin, Some(FP));
}

#[test]
fn duplicating_a_paired_profile_drops_the_pairing() {
    let f = fixture();
    let entry = f
        .service
        .pair(PairHostRequest::Qr {
            uri: PairingPayload::from(target()).to_uri(),
            username: "u".into(),
            password: None,
        })
        .unwrap();
    let copy = f.profiles.duplicate(entry.profile.id).unwrap();
    assert_eq!(copy.profile.paired_host, None);
    assert_eq!(copy.profile.cert_pin, None);
}

#[test]
fn opened_urls_yield_the_first_pairing_link() {
    use drift_app::pairing::pairing_link;
    let uri = PairingPayload::from(target()).to_uri();
    let urls = ["https://example.com/".to_owned(), uri.clone(), "drift-pair:1?other".to_owned()];
    assert_eq!(pairing_link(urls.iter().map(String::as_str)), Some(uri));
    assert_eq!(pairing_link(["file:///tmp/x"].into_iter()), None);
}

// ---- HOST-8: the handshake runs on the next connection ---------------------------------------

#[test]
fn the_next_connection_carries_the_code_then_authenticates_with_the_device_key() {
    use drift_app::profiles::{LinuxPasswordUpdate, SecretsUpdate};
    use drift_ext::channel::Responder as _;
    use drift_ext::pairing::{PairStatus, PairingMessage, PairingOutcome};

    let f = fixture();
    let entry = f
        .service
        .pair(PairHostRequest::Qr {
            uri: PairingPayload::from(target()).to_uri(),
            username: "andrew".into(),
            password: None,
        })
        .unwrap();
    let id = entry.profile.id;

    // A GNOME profile gets no pairing exchange at all.
    let none = SecretsUpdate { rdp_password: None, linux_password: LinuxPasswordUpdate::Keep };
    let gnome = f
        .profiles
        .save(
            drift_core::ConnectionProfile::new("GNOME", "gnome.local", drift_core::ConnectMode::Headless),
            none,
        )
        .unwrap();
    assert!(f.service.client_pairing(gnome.profile.id).unwrap().is_none());

    let first = f.service.client_pairing(id).unwrap().expect("a paired profile authenticates");
    assert_eq!(first.code(), Some(target().code.as_str()), "the code the user saw goes to the Mac once");
    assert_eq!(first.host_fingerprint(), FP.as_bytes());

    // The Mac accepts: the following connection only signs with the device key.
    let mut responder = first.responder();
    responder.on_message(&PairingMessage::Challenge { nonce: [1; 32] }.to_message());
    responder.on_message(&PairingMessage::PairReply(PairStatus::Accepted).to_message());
    assert_eq!(first.outcome(), Some(PairingOutcome::Paired));
    let next = f.service.client_pairing(id).unwrap().unwrap();
    assert_eq!(next.code(), None);
}
