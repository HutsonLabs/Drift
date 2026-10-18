//! HOST-14 Red: which path a connection to a host takes — the LAN, peer-to-peer Wi-Fi (AWDL), the
//! user's own VPN (`utun`), or something else ("away from the local network", EXT-4) — and
//! Bonjour that also works peer to peer.
#![allow(missing_docs, clippy::unwrap_used)]

use std::net::IpAddr;

use drift_apple::bonjour::{REGISTER_FLAGS, browse_includes_peer_to_peer};
use drift_apple::paths::{Interface, NetworkPath, classify, interfaces};

fn iface(name: &str, addr: &str, prefix: u8) -> Interface {
    Interface { name: name.into(), addr: addr.parse().unwrap(), prefix }
}

fn table() -> Vec<Interface> {
    vec![
        iface("lo0", "127.0.0.1", 8),
        iface("en0", "192.168.1.20", 24),
        iface("en0", "fe80::1c2b:3a4d:5e6f:7081", 64),
        iface("awdl0", "fe80::a8bb:ccff:fedd:eeff", 64),
        iface("utun4", "10.8.0.6", 24),
        iface("utun5", "fd7a:115c:a1e0::5", 48),
    ]
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[test]
fn the_lan_the_vpn_and_loopback_are_told_apart() {
    let t = table();
    assert_eq!(classify(ip("192.168.1.40"), None, &t), NetworkPath::Lan { interface: "en0".into() });
    assert_eq!(classify(ip("10.8.0.1"), None, &t), NetworkPath::Vpn { interface: "utun4".into() });
    assert_eq!(classify(ip("fd7a:115c:a1e0::9"), None, &t), NetworkPath::Vpn { interface: "utun5".into() });
    assert_eq!(classify(ip("127.0.0.1"), None, &t), NetworkPath::Local);
    assert_eq!(classify(ip("203.0.113.9"), None, &t), NetworkPath::Remote);
}

#[test]
fn link_local_addresses_follow_their_scope() {
    let t = table();
    // A Bonjour result over AWDL carries the awdl0 scope.
    assert_eq!(classify(ip("fe80::99"), Some("awdl0"), &t), NetworkPath::PeerToPeer { interface: "awdl0".into() });
    assert_eq!(classify(ip("fe80::99"), Some("llw0"), &t), NetworkPath::PeerToPeer { interface: "llw0".into() });
    assert_eq!(classify(ip("fe80::99"), Some("en0"), &t), NetworkPath::Lan { interface: "en0".into() });
    // Without a scope a link-local address cannot be placed.
    assert_eq!(classify(ip("fe80::99"), None, &t), NetworkPath::Remote);
}

#[test]
fn away_means_vpn_or_remote() {
    assert!(NetworkPath::Remote.is_away());
    assert!(NetworkPath::Vpn { interface: "utun4".into() }.is_away());
    assert!(!NetworkPath::Lan { interface: "en0".into() }.is_away());
    assert!(!NetworkPath::PeerToPeer { interface: "awdl0".into() }.is_away());
    assert!(!NetworkPath::Local.is_away());
}

#[test]
fn this_machine_has_loopback() {
    let all = interfaces().unwrap();
    assert!(all.iter().any(|i| i.name == "lo0" && i.addr == ip("127.0.0.1")), "{all:?}");
}

#[test]
fn bonjour_also_goes_peer_to_peer() {
    // kDNSServiceFlagsIncludeP2P: the advert is also offered over AWDL.
    assert_ne!(REGISTER_FLAGS & 0x0002_0000, 0);
    assert!(browse_includes_peer_to_peer(), "nw_parameters for the browser include peer-to-peer");
}
