//! HOST-15 Red: the advert tells devices the Mac's account and that it can approve them, so a
//! device picked from the Bonjour list needs no code (ADR `HOST-15-guided-setup`).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use drift_core::CertFingerprint;
use drift_core::pairing::HostAdvert;
use drift_host::advert::HostAnnouncement;
use uuid::Uuid;

#[test]
fn the_advert_carries_the_account_and_approval() {
    let a = HostAnnouncement {
        host_id: Uuid::from_u128(0xD1F7),
        name: "Studio".into(),
        host: "studio.local".into(),
        port: 3389,
        fingerprint: CertFingerprint::from_bytes([0x5a; 32]),
        account: "andrew".into(),
    };
    let txt = a.txt();
    assert!(txt.contains(&("user".into(), "andrew".into())), "{txt:?}");
    assert!(txt.contains(&("approve".into(), "1".into())), "{txt:?}");
    let advert = HostAdvert::from_txt("Studio", &txt).unwrap();
    assert_eq!(advert.account.as_deref(), Some("andrew"));
    assert!(advert.approval);

    // The window draws the same QR code the terminal prints.
    let code = drift_core::pairing::ShortCode::from_body(3_141_592).unwrap();
    let (width, dark) = a.pairing_qr_modules(&code).unwrap();
    assert_eq!(dark.len(), width * width);
    assert!(width >= 21 && dark.iter().any(|d| *d) && dark.iter().any(|d| !*d));
}
