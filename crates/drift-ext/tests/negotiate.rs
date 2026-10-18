//! EXT-1 Red: version and capability negotiation, and the DVC processors' behaviour.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_ext::channel::{CHANNEL_NAME, DriftMacClient, DriftMacServer, Link, LinkState};
use drift_ext::wire::{Capabilities, Hello, Message, decode, encode};
use drift_ext::{Negotiated, NegotiationError, negotiate};
use ironrdp_dvc::DvcProcessor;

fn hello(min: u16, max: u16, caps: Capabilities) -> Hello {
    Hello { min_version: min, max_version: max, capabilities: caps, agent: "test".into() }
}

#[test]
fn the_highest_common_version_and_shared_capabilities_win() {
    let ours = hello(1, 3, Capabilities::SCROLL | Capabilities::MENUS);
    let theirs = hello(2, 5, Capabilities::MENUS | Capabilities::STATUS);
    assert_eq!(negotiate(&ours, &theirs), Ok(Negotiated { version: 3, capabilities: Capabilities::MENUS }));
}

#[test]
fn disjoint_versions_fail() {
    assert_eq!(
        negotiate(&hello(1, 1, Capabilities::NONE), &hello(2, 3, Capabilities::NONE)),
        Err(NegotiationError::NoCommonVersion { ours: (1, 1), theirs: (2, 3) })
    );
    assert!(
        negotiate(&hello(3, 1, Capabilities::NONE), &hello(1, 3, Capabilities::NONE)).is_err(),
        "min > max is invalid"
    );
}

#[test]
fn the_channel_has_the_adr_name() {
    assert_eq!(CHANNEL_NAME, "Drift::Mac::v1");
    let link = Link::new();
    assert_eq!(DriftMacClient::new(link.clone(), Capabilities::NONE).channel_name(), CHANNEL_NAME);
    assert_eq!(DriftMacServer::new(link, Capabilities::NONE).channel_name(), CHANNEL_NAME);
}

/// Runs the two processors against each other in memory, as the DVC layer would.
fn handshake(client_caps: Capabilities, server_caps: Capabilities) -> (Link, Link) {
    let (cl, sl) = (Link::new(), Link::new());
    let mut client = DriftMacClient::new(cl.clone(), client_caps);
    let mut server = DriftMacServer::new(sl.clone(), server_caps);
    assert_eq!(cl.state(), LinkState::Absent);
    let to_client = server.start(7).unwrap();
    let to_server = client.start(7).unwrap();
    assert_eq!(cl.state(), LinkState::Opened);
    for m in to_client {
        let bytes = encode_dvc(&*m);
        assert!(client.process(7, &bytes).unwrap().is_empty());
    }
    for m in to_server {
        let bytes = encode_dvc(&*m);
        assert!(server.process(7, &bytes).unwrap().is_empty());
    }
    (cl, sl)
}

fn encode_dvc(m: &dyn ironrdp_dvc::DvcEncode) -> Vec<u8> {
    let mut buf = vec![0; m.size()];
    let mut cursor = ironrdp_core::WriteCursor::new(&mut buf);
    m.encode(&mut cursor).unwrap();
    buf
}

#[test]
fn client_and_server_negotiate_over_the_processors() {
    let both = Capabilities::SCROLL | Capabilities::STATUS;
    let (cl, sl) = handshake(both, Capabilities::STATUS | Capabilities::PAIRING);
    let expected = LinkState::Negotiated(Negotiated { version: 1, capabilities: Capabilities::STATUS });
    assert_eq!(cl.state(), expected);
    assert_eq!(sl.state(), expected);
}

#[test]
fn a_malformed_message_fails_the_extension_but_not_the_session() {
    let link = Link::new();
    let mut client = DriftMacClient::new(link.clone(), Capabilities::NONE);
    client.start(3).unwrap();
    // Garbage is not an error for the DVC layer (which would end the RDP session): the
    // extension stops and the session carries on as plain RDP.
    assert!(client.process(3, b"garbage").unwrap().is_empty());
    assert!(matches!(link.state(), LinkState::Failed(_)));
    let late = encode(&Message::Hello(hello(1, 1, Capabilities::NONE))).unwrap();
    assert!(client.process(3, &late).unwrap().is_empty());
    assert!(matches!(link.state(), LinkState::Failed(_)), "stays failed");
}

#[test]
fn after_negotiation_other_messages_queue_for_their_task() {
    let (cl, _) = handshake(Capabilities::NONE, Capabilities::NONE);
    let mut client = DriftMacClient::new(cl.clone(), Capabilities::NONE);
    client.start(1).unwrap();
    let server_hello = encode(&Message::Hello(hello(1, 1, Capabilities::NONE))).unwrap();
    client.process(1, &server_hello).unwrap();
    let status = Message::Unknown { kind: drift_ext::wire::MessageType(0x0301), payload: vec![9] };
    client.process(1, &encode(&status).unwrap()).unwrap();
    assert_eq!(cl.take_inbox(), vec![status]);
    assert!(cl.take_inbox().is_empty());
    let _ = decode;
}

#[test]
fn closing_resets_the_link() {
    let (cl, _) = handshake(Capabilities::NONE, Capabilities::NONE);
    let mut client = DriftMacClient::new(cl.clone(), Capabilities::NONE);
    client.start(1).unwrap();
    client.close(1);
    assert_eq!(cl.state(), LinkState::Closed);
}
