//! EXT-5 Red: one channel end serves several tasks (pairing, codec negotiation, …) through
//! [`drift_ext::compose::Responders`]: every responder may speak after the handshake, and each
//! message goes to the first responder that answers it.
#![allow(missing_docs, clippy::unwrap_used)]

use drift_ext::channel::Responder;
use drift_ext::compose::Responders;
use drift_ext::wire::{Capabilities, Hello, Message, MessageType};
use drift_ext::{Negotiated, negotiate};

#[derive(Debug)]
struct Echo {
    owns: u16,
    greeting: u16,
}

impl Responder for Echo {
    fn on_negotiated(&mut self, _n: &Negotiated) -> Vec<Message> {
        vec![Message::Unknown { kind: MessageType(self.greeting), payload: vec![] }]
    }

    fn on_message(&mut self, m: &Message) -> Option<Vec<Message>> {
        (m.kind().0 >> 8 == self.owns).then(|| vec![Message::Unknown { kind: m.kind(), payload: vec![1] }])
    }
}

#[test]
fn every_responder_speaks_and_the_owner_answers() {
    let mut all = Responders::new()
        .with(Box::new(Echo { owns: 0x04, greeting: 0x0401 }))
        .with(Box::new(Echo { owns: 0x05, greeting: 0x0501 }));
    let hello =
        Hello { min_version: 1, max_version: 1, capabilities: Capabilities::CODEC, agent: String::new() };
    let n = negotiate(&hello, &hello).unwrap();
    let kinds: Vec<u16> = all.on_negotiated(&n).iter().map(|m| m.kind().0).collect();
    assert_eq!(kinds, [0x0401, 0x0501]);

    let to_codec = Message::Unknown { kind: MessageType(0x0502), payload: vec![] };
    assert_eq!(
        all.on_message(&to_codec),
        Some(vec![Message::Unknown { kind: MessageType(0x0502), payload: vec![1] }])
    );
    let nobody = Message::Unknown { kind: MessageType(0x0301), payload: vec![] };
    assert_eq!(all.on_message(&nobody), None);
    assert!(Responders::new().is_empty());
    assert_eq!(all.len(), 2);
}
