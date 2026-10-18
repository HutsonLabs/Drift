//! IOS-9 Red: Drift Hosts are found with Bonjour (`_drift-host._tcp`, ADR IOS-9). The test
//! advertises a host with `DNSServiceRegister` (what HOST-8 will do) and browses for it with
//! `nw_browser`, the way the Connections page does. Local only: the advert never leaves the
//! machine's mDNS responder.
#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use drift_apple::bonjour::{Advertisement, HostBrowser};
use drift_core::pairing::{HostAdvert, SERVICE_TYPE};
use uuid::Uuid;

#[test]
fn a_registered_host_is_found_with_its_txt_record() {
    let id = Uuid::new_v4();
    let fp = "cd".repeat(32);
    let name = format!("Drift test {}", &id.to_string()[..8]);
    let txt = vec![
        ("v".to_owned(), "1".to_owned()),
        ("id".to_owned(), id.to_string()),
        ("name".to_owned(), "Test Mac".to_owned()),
        ("host".to_owned(), "test-mac.local".to_owned()),
        ("port".to_owned(), "3390".to_owned()),
        ("fp".to_owned(), fp),
    ];
    let _advert = Advertisement::register(&name, SERVICE_TYPE, 3390, &txt).expect("DNSServiceRegister");

    let (tx, rx) = mpsc::channel::<Vec<HostAdvert>>();
    let tx = Mutex::new(tx);
    let browser = HostBrowser::start(move |hosts| {
        let _ = tx.lock().unwrap().send(hosts);
    })
    .expect("nw_browser");
    let deadline = Instant::now() + Duration::from_secs(10);
    let found = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let hosts = rx.recv_timeout(left).expect("the browser reports the advertised host");
        if let Some(h) = hosts.into_iter().find(|h| h.host_id == id) {
            break h;
        }
    };
    assert_eq!(found.name, "Test Mac");
    assert_eq!((found.host.as_str(), found.port), ("test-mac.local", 3390));
    drop(browser);
}
