//! HOST-8 Red: what the host advertises with Bonjour and puts in its QR code is exactly what the
//! client's parsers (IOS-9, `drift_core::pairing`) accept.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use drift_core::CertFingerprint;
use drift_core::pairing::{HostAdvert, PairingPayload, SERVICE_TYPE, ShortCode};
use drift_host::advert::{HostAnnouncement, SERVICE};
use uuid::Uuid;

fn announcement() -> HostAnnouncement {
    HostAnnouncement {
        host_id: Uuid::from_u128(0xD1F7),
        name: "Andrew’s Mac Studio".into(),
        host: "andrews-mac-studio.local".into(),
        port: 3389,
        fingerprint: CertFingerprint::from_bytes([0x5a; 32]),
    }
}

#[test]
fn the_txt_record_is_what_the_client_reads() {
    assert_eq!(SERVICE, SERVICE_TYPE);
    let a = announcement();
    let advert = HostAdvert::from_txt("Andrew’s Mac Studio", &a.txt()).unwrap();
    assert_eq!(advert.host_id, a.host_id);
    assert_eq!(advert.name, a.name);
    assert_eq!(advert.host, a.host);
    assert_eq!(advert.port, a.port);
    assert_eq!(advert.fingerprint, a.fingerprint);
}

#[test]
fn the_pairing_link_is_what_the_client_scans() {
    let a = announcement();
    let code = ShortCode::from_body(3_141_592).unwrap();
    let link = a.pairing_link(&code);
    let target = PairingPayload::parse(&link).unwrap().target();
    assert_eq!((target.host_id, target.port, target.fingerprint), (a.host_id, a.port, a.fingerprint));
    assert_eq!((target.name.as_str(), target.host.as_str()), (a.name.as_str(), a.host.as_str()));
    assert_eq!(target.code, code);
    let qr = a.pairing_qr(&code);
    assert!(qr.lines().count() > 10, "a terminal QR code:\n{qr}");
}
