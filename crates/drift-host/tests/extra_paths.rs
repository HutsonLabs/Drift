//! HOST-14 Red: the host's listener also accepts connections over peer-to-peer Wi-Fi
//! (`SO_RECV_ANYIF`, which BSD sockets need for AWDL) when it listens beyond loopback.
#![allow(missing_docs, clippy::unwrap_used)]

use std::os::fd::AsRawFd as _;

use drift_host::link::{allow_peer_to_peer, peer_to_peer_allowed};

#[test]
fn a_listener_can_take_peer_to_peer_connections() {
    let listener = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    assert!(!peer_to_peer_allowed(listener.as_raw_fd()).unwrap(), "off by default");
    allow_peer_to_peer(listener.as_raw_fd()).unwrap();
    assert!(peer_to_peer_allowed(listener.as_raw_fd()).unwrap());
}
