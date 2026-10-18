//! EXT-4 Red: host status and quality hints on `Drift::Mac::v1`.
#![allow(missing_docs, clippy::unwrap_used)]

use std::net::IpAddr;

use drift_ext::Negotiated;
use drift_ext::status::{
    Backlog, HostStatus, QualityLevel, QualityPreference, StatusClient, away_from, types,
};
use drift_ext::wire::{Capabilities, Message, MessageType, decode, encode};

fn status() -> HostStatus {
    HostStatus { backlog: Backlog::Building, quality: QualityLevel::Medium, bitrate_kbps: 12_500, fps: 30 }
}

#[test]
fn status_and_preference_round_trip() {
    let s = status();
    let back = decode(&encode(&s.to_message()).unwrap()).unwrap();
    assert_eq!(HostStatus::from_message(&back), Some(Ok(s)));
    let p = QualityPreference { max: QualityLevel::High, away: true };
    let back = decode(&encode(&p.to_message()).unwrap()).unwrap();
    assert_eq!(QualityPreference::from_message(&back), Some(Ok(p)));
    assert_eq!(types::HOST_STATUS.area(), "host status and quality (EXT-4)");
    assert_eq!(types::QUALITY_PREFERENCE.area(), "host status and quality (EXT-4)");
}

#[test]
fn malformed_status_messages_are_errors() {
    for (kind, payload) in [
        (types::HOST_STATUS, vec![0; 3]),
        (types::HOST_STATUS, vec![9, 0, 0, 0, 0, 0, 30]), // unknown backlog
        (types::QUALITY_PREFERENCE, vec![7, 0]),          // unknown level
        (types::QUALITY_PREFERENCE, vec![0, 2]),          // away is a bool
    ] {
        let m = Message::Unknown { kind, payload };
        assert!(
            matches!(HostStatus::from_message(&m), Some(Err(_)))
                || matches!(QualityPreference::from_message(&m), Some(Err(_))),
            "{m:?}"
        );
    }
    let other = Message::Unknown { kind: MessageType(0x0101), payload: vec![] };
    assert_eq!(HostStatus::from_message(&other), None);
}

#[test]
fn levels_are_ordered_best_first() {
    assert!(QualityLevel::Best < QualityLevel::High);
    assert!(QualityLevel::Medium < QualityLevel::Low);
    assert_eq!(QualityLevel::Best.lower(), QualityLevel::High);
    assert_eq!(QualityLevel::Low.lower(), QualityLevel::Low);
    assert_eq!(QualityLevel::Low.higher(), QualityLevel::Medium);
    assert_eq!(QualityLevel::Best.higher(), QualityLevel::Best);
}

#[test]
fn a_host_away_from_the_local_network_gets_the_away_default() {
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    for local in
        ["192.168.1.20", "10.1.2.40", "172.20.0.5", "127.0.0.1", "169.254.3.4", "fe80::1", "fd00::5", "::1"]
    {
        assert!(!away_from(ip(local)), "{local} is on the local network");
    }
    for away in ["8.8.8.8", "100.101.102.103", "2001:db8::1", "17.253.144.10"] {
        assert!(away_from(ip(away)), "{away} is away (the internet, or a VPN's 100.64/10 range)");
    }
    assert_eq!(
        QualityPreference::default_for(false),
        QualityPreference { max: QualityLevel::Best, away: false }
    );
    assert_eq!(
        QualityPreference::default_for(true),
        QualityPreference { max: QualityLevel::Medium, away: true }
    );
}

#[test]
fn the_client_sends_its_preference_once_status_was_agreed_and_keeps_the_hosts_status() {
    let pref = QualityPreference::default_for(true);
    let client = StatusClient::new(pref);
    let mut responder = client.responder();
    let none = Negotiated { version: 1, capabilities: Capabilities::SCROLL };
    assert!(responder.on_negotiated(&none).is_empty(), "no STATUS, no preference");
    let both = Negotiated { version: 1, capabilities: Capabilities::STATUS };
    let sent = responder.on_negotiated(&both);
    assert_eq!(sent.len(), 1);
    assert_eq!(QualityPreference::from_message(&sent[0]), Some(Ok(pref)));
    assert_eq!(client.host_status(), None);
    assert_eq!(responder.on_message(&status().to_message()), Some(Vec::new()), "consumed");
    assert_eq!(client.host_status(), Some(status()));
    let other = Message::Unknown { kind: MessageType(0x0101), payload: vec![] };
    assert_eq!(responder.on_message(&other), None, "not ours");
}

#[test]
fn profile_host_names_and_addresses() {
    use drift_ext::status::away_from_host;
    for local in
        ["10.1.2.40", "studio.local", "studio", "nas.lan", "box.home.arpa", "[fe80::1]", "Studio.Local."]
    {
        assert!(!away_from_host(local), "{local}");
    }
    for away in ["mac.example.com", "8.8.8.8", "100.90.1.2", "[2001:db8::1]"] {
        assert!(away_from_host(away), "{away}");
    }
}
