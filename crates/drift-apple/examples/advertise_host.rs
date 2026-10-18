//! Advertises a fake Drift Host with Bonjour until Ctrl-C, and prints its pairing code and QR
//! payload (IOS-9: trying the "Pair a Mac" sheet before HOST-8 exists). Test values only.
//!
//! ```text
//! cargo run -p drift-apple --example advertise_host -- ["Mac name"] [host] [port]
//! ```
#![allow(missing_docs, clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use drift_apple::bonjour::Advertisement;
use drift_core::CertFingerprint;
use drift_core::pairing::{PairingPayload, PairingTarget, SERVICE_TYPE, ShortCode};
use uuid::Uuid;

fn main() {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "Studio Mac".into());
    let host = args.next().unwrap_or_else(|| "studio.local".into());
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(3389);
    let host_id = Uuid::new_v4();
    let fingerprint = CertFingerprint::from_bytes(*Uuid::new_v4().as_bytes().repeat(2).first_chunk().unwrap());
    let code = ShortCode::from_body(u32::try_from(host_id.as_u128() % 10_000_000).unwrap()).unwrap();
    let fp_hex: String = fingerprint.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let txt = vec![
        ("v".to_owned(), "1".to_owned()),
        ("id".to_owned(), host_id.to_string()),
        ("name".to_owned(), name.clone()),
        ("host".to_owned(), host.clone()),
        ("port".to_owned(), port.to_string()),
        ("fp".to_owned(), fp_hex),
    ];
    let _advert = Advertisement::register(&name, SERVICE_TYPE, port, &txt).expect("DNSServiceRegister");
    let payload = PairingPayload::from(PairingTarget { host_id, name: name.clone(), host, port, fingerprint, code: code.clone() });
    println!("advertising \"{name}\" as {SERVICE_TYPE}");
    println!("code: {code}");
    println!("qr: {}", payload.to_uri());
    loop {
        std::thread::park();
    }
}
