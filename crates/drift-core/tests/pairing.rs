//! IOS-9 Red: the client half of Mac host pairing (ADR IOS-0 decision 7, ADR IOS-9) — the
//! typed code, the QR payload, the Bonjour advert, the profile a pairing creates, and the
//! client's pairing state machine. Test values only.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_core::pairing::{
    HostAdvert, PROTOCOL_VERSION, PairReply, PairingError, PairingFlow, PairingPayload, PairingTarget,
    RejectReason, SERVICE_TYPE, ShortCode, damm_check_digit,
};
use drift_core::{CertFingerprint, ConnectMode, ConnectionProfile};
use proptest::prelude::*;
use uuid::Uuid;

const FP: CertFingerprint = CertFingerprint::from_bytes([0xab; 32]);

fn target() -> PairingTarget {
    PairingTarget {
        host_id: Uuid::from_u128(0x1234),
        name: "Studio Mac".into(),
        host: "studio.local".into(),
        port: 3389,
        fingerprint: FP,
        code: ShortCode::from_body(1_234_567).unwrap(),
    }
}

// ---- the typed code -------------------------------------------------------------------------

#[test]
fn damm_check_digit_matches_the_published_example() {
    // Damm's worked example: 572 → check digit 4.
    assert_eq!(damm_check_digit("572"), Some(4));
    assert_eq!(damm_check_digit(""), Some(0));
    assert_eq!(damm_check_digit("57a"), None);
}

#[test]
fn a_code_is_seven_digits_and_a_check_digit() {
    let code = ShortCode::from_body(5_720_000).unwrap();
    assert_eq!(code.as_str().len(), 8);
    assert!(code.as_str().starts_with("5720000"));
    assert_eq!(code.to_string(), format!("{} {}", &code.as_str()[..4], &code.as_str()[4..]));
    assert!(ShortCode::from_body(10_000_000).is_none(), "the body has seven digits");
    assert_eq!(ShortCode::from_body(42).unwrap().as_str()[..7], *"0000042");
}

#[test]
fn typed_codes_ignore_spaces_and_dashes() {
    let code = ShortCode::from_body(1_234_567).unwrap();
    let digits = code.as_str().to_owned();
    for typed in [
        digits.clone(),
        format!("{} {}", &digits[..4], &digits[4..]),
        format!(" {}-{} ", &digits[..4], &digits[4..]),
    ] {
        assert_eq!(ShortCode::parse(&typed).unwrap(), code, "{typed:?}");
    }
}

#[test]
fn typed_codes_reject_typos() {
    let digits = ShortCode::from_body(1_234_567).unwrap().as_str().to_owned();
    // Every single-digit error is caught by the check digit.
    for i in 0..8 {
        for d in b'0'..=b'9' {
            let mut wrong = digits.clone().into_bytes();
            if wrong[i] == d {
                continue;
            }
            wrong[i] = d;
            let wrong = String::from_utf8(wrong).unwrap();
            assert_eq!(ShortCode::parse(&wrong), Err(PairingError::WrongCheckDigit), "{wrong}");
        }
    }
    assert_eq!(ShortCode::parse("1234"), Err(PairingError::MalformedCode));
    assert_eq!(ShortCode::parse("1234 56789"), Err(PairingError::MalformedCode));
    assert_eq!(ShortCode::parse("1234 567x"), Err(PairingError::MalformedCode));
}

proptest! {
    #[test]
    fn every_code_round_trips_and_catches_adjacent_swaps(body in 0_u32..10_000_000) {
        let code = ShortCode::from_body(body).unwrap();
        prop_assert_eq!(ShortCode::parse(&code.to_string()).unwrap(), code.clone());
        let digits = code.as_str().as_bytes().to_vec();
        for i in 0..7 {
            if digits[i] != digits[i + 1] {
                let mut swapped = digits.clone();
                swapped.swap(i, i + 1);
                let swapped = String::from_utf8(swapped).unwrap();
                prop_assert_eq!(ShortCode::parse(&swapped), Err(PairingError::WrongCheckDigit));
            }
        }
    }
}

// ---- the QR payload -------------------------------------------------------------------------

#[test]
fn qr_payload_round_trips() {
    let payload = PairingPayload::from(target());
    let uri = payload.to_uri();
    assert!(uri.starts_with("drift-pair:1?"), "{uri}");
    assert_eq!(PairingPayload::parse(&uri).unwrap(), payload);
    assert_eq!(payload.target(), target());
}

proptest! {
    #[test]
    fn qr_payload_round_trips_any_name_and_host(
        name in "\\PC{1,40}",
        host in "[a-z0-9.-]{1,30}",
        port in 1_u16..,
        id in any::<u128>(),
        fp in any::<[u8; 32]>(),
        body in 0_u32..10_000_000,
    ) {
        let payload = PairingPayload::from(PairingTarget {
            host_id: Uuid::from_u128(id),
            name,
            host,
            port,
            fingerprint: CertFingerprint::from_bytes(fp),
            code: ShortCode::from_body(body).unwrap(),
        });
        prop_assert_eq!(PairingPayload::parse(&payload.to_uri()).unwrap(), payload);
    }

    #[test]
    fn parsing_arbitrary_text_never_panics(text in "\\PC{0,200}") {
        let _ = PairingPayload::parse(&text);
        let _ = ShortCode::parse(&text);
    }
}

#[test]
fn qr_payload_rejects_malformed_input() {
    let good = PairingPayload::from(target()).to_uri();
    let cases = [
        good.replace("drift-pair:1?", "https://example.com/?"),
        good.replace("drift-pair:1?", "drift-pair:9?"),
        good.replace("&port=3389", "&port=0"),
        good.replace("&port=3389", "&port=99999"),
        good.replace("&port=3389", ""),
        good.replacen("fp=ab", "fp=zz", 1),
        good.replacen("fp=abab", "fp=ab", 1),
        good.replace(&format!("code={}", target().code.as_str()), "code=12345678"),
        good.replace(&format!("id={}", Uuid::from_u128(0x1234)), "id=nope"),
        format!("{good}&id={}", Uuid::new_v4()),
        String::new(),
    ];
    for bad in cases {
        assert!(PairingPayload::parse(&bad).is_err(), "accepted {bad}");
    }
    assert_eq!(
        PairingPayload::parse(&good.replace("drift-pair:1?", "drift-pair:9?")),
        Err(PairingError::UnsupportedVersion(9))
    );
}

// ---- Bonjour ----------------------------------------------------------------------------------

fn txt(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

fn fp_hex() -> String {
    "ab".repeat(32)
}

#[test]
fn service_type_and_version() {
    assert_eq!(SERVICE_TYPE, "_drift-host._tcp");
    assert_eq!(PROTOCOL_VERSION, 1);
}

#[test]
fn a_txt_record_becomes_an_advert() {
    let id = Uuid::from_u128(0x1234).to_string();
    let fp = fp_hex();
    let advert = HostAdvert::from_txt(
        "Studio Mac (Drift Host)",
        &txt(&[
            ("v", "1"),
            ("id", &id),
            ("name", "Studio Mac"),
            ("host", "studio.local"),
            ("port", "3390"),
            ("fp", &fp),
        ]),
    )
    .unwrap();
    assert_eq!(advert.host_id, Uuid::from_u128(0x1234));
    assert_eq!(advert.name, "Studio Mac");
    assert_eq!((advert.host.as_str(), advert.port), ("studio.local", 3390));
    assert_eq!(advert.fingerprint, FP);
    let code = ShortCode::from_body(1_234_567).unwrap();
    let t = advert.target(code.clone());
    assert_eq!(t, PairingTarget { port: 3390, ..target() });
}

#[test]
fn a_txt_record_without_a_name_uses_the_service_name_and_defaults_the_port() {
    let id = Uuid::from_u128(1).to_string();
    let fp = fp_hex();
    let advert = HostAdvert::from_txt(
        "Office",
        &txt(&[("v", "1"), ("id", &id), ("host", "office.local"), ("fp", &fp)]),
    )
    .unwrap();
    assert_eq!(advert.name, "Office");
    assert_eq!(advert.port, 3389);
}

#[test]
fn bad_txt_records_are_rejected() {
    let id = Uuid::from_u128(1).to_string();
    let fp = fp_hex();
    let base = [("v", "1"), ("id", id.as_str()), ("host", "h.local"), ("fp", fp.as_str())];
    for drop in 0..base.len() {
        let pairs: Vec<_> = base.iter().enumerate().filter(|(i, _)| *i != drop).map(|(_, p)| *p).collect();
        assert!(HostAdvert::from_txt("x", &txt(&pairs)).is_err(), "missing {}", base[drop].0);
    }
    let mut newer = base.to_vec();
    newer[0] = ("v", "2");
    assert_eq!(HostAdvert::from_txt("x", &txt(&newer)), Err(PairingError::UnsupportedVersion(2)));
    let mut bad_port = base.to_vec();
    bad_port.push(("port", "0"));
    assert!(HostAdvert::from_txt("x", &txt(&bad_port)).is_err());
}

// ---- the profile a pairing creates --------------------------------------------------------

#[test]
fn a_paired_profile_pins_the_host_and_remembers_it() {
    let p = target().profile();
    assert_eq!(p.name, "Studio Mac");
    assert_eq!((p.host.as_str(), p.port), ("studio.local", 3389));
    assert_eq!(p.cert_pin, Some(FP), "paired hosts are pinned up front, never trust-on-first-use");
    assert_eq!(p.paired_host, Some(Uuid::from_u128(0x1234)));
    assert_eq!(p.mode, ConnectMode::Headless);
}

#[test]
fn profiles_without_a_pairing_keep_their_toml_shape() {
    let gnome = ConnectionProfile::new("Homelab", "gnome.local", ConnectMode::Headless);
    assert_eq!(gnome.paired_host, None);
    let text = gnome.to_toml().unwrap();
    assert!(!text.contains("paired_host"), "GNOME profiles are written exactly as before:\n{text}");
    assert_eq!(ConnectionProfile::from_toml(&text).unwrap(), gnome);
    let paired = target().profile();
    assert_eq!(ConnectionProfile::from_toml(&paired.to_toml().unwrap()).unwrap(), paired);
}

// ---- the client state machine --------------------------------------------------------------

#[test]
fn the_flow_checks_the_pin_before_sending_the_code() {
    let mut flow = PairingFlow::new(target(), "iPad", vec![4, 5, 6]);
    let request = flow.on_tls(FP).unwrap();
    assert_eq!(request.version, PROTOCOL_VERSION);
    assert_eq!(request.code, target().code);
    assert_eq!(request.device_name, "iPad");
    assert_eq!(request.device_key, vec![4, 5, 6]);
    let profile = flow.on_reply(PairReply::Accepted).unwrap();
    assert_eq!(profile.paired_host, Some(target().host_id));
    assert!(flow.is_done());
}

#[test]
fn a_different_certificate_never_receives_the_code() {
    let mut flow = PairingFlow::new(target(), "iPad", vec![1]);
    let other = CertFingerprint::from_bytes([0xcd; 32]);
    assert_eq!(flow.on_tls(other), Err(PairingError::FingerprintMismatch));
    assert_eq!(flow.on_reply(PairReply::Accepted), Err(PairingError::UnexpectedMessage));
    assert!(flow.is_done());
}

#[test]
fn the_host_may_refuse() {
    let mut flow = PairingFlow::new(target(), "iPad", vec![1]);
    flow.on_tls(FP).unwrap();
    assert_eq!(
        flow.on_reply(PairReply::Rejected(RejectReason::WrongCode)),
        Err(PairingError::Rejected(RejectReason::WrongCode))
    );
    assert!(flow.is_done());
}

#[test]
fn messages_out_of_order_are_errors() {
    let mut flow = PairingFlow::new(target(), "iPad", vec![1]);
    assert_eq!(flow.on_reply(PairReply::Accepted), Err(PairingError::UnexpectedMessage));
    let mut flow = PairingFlow::new(target(), "iPad", vec![1]);
    flow.on_tls(FP).unwrap();
    assert_eq!(flow.on_tls(FP), Err(PairingError::UnexpectedMessage));
}

// ---- the store keeps a paired host's pin ----------------------------------------------------

#[test]
fn a_paired_hosts_pin_cannot_be_cleared_or_replaced() {
    use drift_core::{ProfileStore, StoreError};
    let mut paired = target().profile();
    paired.rdp_username = "mac-user".into();
    let mut store = ProfileStore::new();
    store.upsert(paired.clone()).unwrap();
    assert_eq!(store.set_pin(paired.id, None), Err(StoreError::PairedHost(paired.id)));
    let other = CertFingerprint::from_bytes([1; 32]);
    assert_eq!(store.set_pin(paired.id, Some(other)), Err(StoreError::PairedHost(paired.id)));
    store.set_pin(paired.id, Some(FP)).unwrap();
    assert_eq!(store.get(paired.id).unwrap().cert_pin, Some(FP));

    let mut gnome = ConnectionProfile::new("Homelab", "gnome.local", ConnectMode::Headless);
    gnome.rdp_username = "u".into();
    store.upsert(gnome.clone()).unwrap();
    store.set_pin(gnome.id, Some(other)).unwrap();
    store.set_pin(gnome.id, None).unwrap();
}
