//! EXT-3 Red: the mirrored menu bar on `Drift::Mac::v1` — the snapshot model and its strict
//! encoding, the client's poll-and-press state machine, shortcut labels, and the responder chain
//! that lets menus share the host's channel with pairing.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use drift_ext::channel::{Chain, Responder};
use drift_ext::menus::{
    KeyEquivalent, MAX_DEPTH, MAX_ITEMS, MAX_TITLE, MenuItem, MenuMessage, MenuMirror, MenuSnapshot,
    MirrorEvent, Modifiers, POLL_INTERVAL, types,
};
use drift_ext::wire::{Capabilities, Message, MessageType, decode, encode};
use drift_ext::{Negotiated, negotiate};

fn item(id: u32, title: &str) -> MenuItem {
    MenuItem {
        id,
        title: title.into(),
        enabled: true,
        checked: false,
        separator: false,
        key: None,
        children: vec![],
    }
}

fn sample() -> MenuSnapshot {
    let mut save = item(3, "Save");
    save.key = Some(KeyEquivalent { key: "s".into(), modifiers: Modifiers::COMMAND });
    let mut save_as = item(4, "Save As…");
    save_as.key = Some(KeyEquivalent { key: "s".into(), modifiers: Modifiers::COMMAND | Modifiers::SHIFT });
    let sep = MenuItem { separator: true, enabled: false, ..item(5, "") };
    let mut ruler = item(7, "Show Ruler");
    ruler.checked = true;
    let mut disabled = item(8, "Revert");
    disabled.enabled = false;
    let mut file = item(2, "File");
    file.children = vec![save, save_as, sep, disabled];
    let mut view = item(6, "View");
    view.children = vec![ruler];
    let mut app = item(1, "TextEdit");
    app.children = vec![item(9, "About TextEdit")];
    MenuSnapshot { revision: 7, app: "TextEdit".into(), menus: vec![app, file, view] }
}

#[test]
fn snapshot_round_trips_through_the_wire() {
    let snap = sample();
    let frame = encode(&MenuMessage::Snapshot(snap.clone()).to_message()).unwrap();
    let message = decode(&frame).unwrap();
    assert_eq!(message.kind(), types::SNAPSHOT);
    let back = MenuMessage::from_message(&message).unwrap().unwrap();
    assert_eq!(back, MenuMessage::Snapshot(snap));
}

#[test]
fn small_messages_round_trip() {
    for m in [
        MenuMessage::Request { known_revision: 0 },
        MenuMessage::Request { known_revision: 41 },
        MenuMessage::Unchanged { revision: 41 },
        MenuMessage::Press { revision: 41, id: 7 },
        MenuMessage::Empty,
    ] {
        let frame = encode(&m.to_message()).unwrap();
        assert_eq!(MenuMessage::from_message(&decode(&frame).unwrap()).unwrap().unwrap(), m);
    }
}

#[test]
fn menu_types_are_in_the_ext3_range_and_other_messages_are_not_ours() {
    for t in [types::REQUEST, types::SNAPSHOT, types::UNCHANGED, types::PRESS, types::EMPTY] {
        assert_eq!(t.area(), "menu mirroring (EXT-3)");
    }
    assert!(
        MenuMessage::from_message(&Message::Unknown { kind: MessageType(0x0401), payload: vec![] }).is_none()
    );
}

#[test]
fn decoding_is_strict_and_bounded() {
    let bad = |payload: Vec<u8>| {
        MenuMessage::from_message(&Message::Unknown { kind: types::SNAPSHOT, payload }).unwrap().is_err()
    };
    assert!(bad(vec![]));
    assert!(bad(vec![1, 2, 3]));
    // Valid snapshot plus trailing junk.
    let Message::Unknown { payload, .. } = MenuMessage::Snapshot(sample()).to_message() else { panic!() };
    let mut extra = payload.clone();
    extra.push(0);
    assert!(bad(extra));
    // Truncated anywhere.
    for cut in 0..payload.len() {
        assert!(bad(payload[..cut].to_vec()), "a snapshot cut at {cut} bytes decoded");
    }
    // A press with a wrong length.
    assert!(
        MenuMessage::from_message(&Message::Unknown { kind: types::PRESS, payload: vec![0; 7] })
            .unwrap()
            .is_err()
    );
}

#[test]
fn encoding_refuses_snapshots_over_the_limits() {
    let deep = (0..=MAX_DEPTH).fold(item(1, "leaf"), |child, depth| {
        let mut parent = item(u32::try_from(depth).unwrap() + 2, "level");
        parent.children = vec![child];
        parent
    });
    let snap = MenuSnapshot { revision: 1, app: "Deep".into(), menus: vec![deep] };
    assert!(MenuMessage::Snapshot(snap).try_to_message().is_err(), "too deep");

    let many = (0..=u32::try_from(MAX_ITEMS).unwrap()).map(|i| item(i, "x")).collect();
    let snap = MenuSnapshot { revision: 1, app: "Many".into(), menus: many };
    assert!(MenuMessage::Snapshot(snap).try_to_message().is_err(), "too many items");

    let long =
        MenuSnapshot { revision: 1, app: "Long".into(), menus: vec![item(1, &"x".repeat(MAX_TITLE + 1))] };
    assert!(MenuMessage::Snapshot(long).try_to_message().is_err(), "title too long");
}

#[test]
fn snapshots_with_the_same_menus_have_the_same_content_hash() {
    let a = sample();
    let mut b = sample();
    b.revision = 99;
    assert_eq!(a.content_hash(), b.content_hash(), "the revision is not content");
    b.menus[1].children[3].enabled = true;
    assert_ne!(a.content_hash(), b.content_hash());
}

#[test]
fn find_and_count_items() {
    let snap = sample();
    assert_eq!(snap.find(7).map(|i| i.title.as_str()), Some("Show Ruler"));
    assert!(snap.find(1000).is_none());
    assert_eq!(snap.item_count(), 9);
}

#[test]
fn shortcut_labels_follow_the_mac_order() {
    let k = |key: &str, modifiers| KeyEquivalent { key: key.into(), modifiers }.label();
    assert_eq!(k("s", Modifiers::COMMAND), "⌘S");
    assert_eq!(k("s", Modifiers::COMMAND | Modifiers::SHIFT), "⇧⌘S");
    assert_eq!(k("q", Modifiers::COMMAND | Modifiers::OPTION | Modifiers::CONTROL), "⌃⌥⌘Q");
    assert_eq!(k(",", Modifiers::COMMAND), "⌘,");
    assert_eq!(k("f", Modifiers::CONTROL | Modifiers::COMMAND), "⌃⌘F");
}

#[test]
fn ax_modifier_bits_map_to_modifiers() {
    // kAXMenuItemModifier*: Shift = 1, Option = 2, Control = 4, NoCommand = 8.
    assert_eq!(Modifiers::from_ax(0), Modifiers::COMMAND);
    assert_eq!(Modifiers::from_ax(1), Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(Modifiers::from_ax(2 | 4), Modifiers::COMMAND | Modifiers::OPTION | Modifiers::CONTROL);
    assert_eq!(Modifiers::from_ax(8), Modifiers::NONE);
    assert_eq!(Modifiers::from_ax(8 | 4), Modifiers::CONTROL);
}

fn menus_negotiated() -> Negotiated {
    Negotiated { version: 1, capabilities: Capabilities::MENUS }
}

#[test]
fn the_mirror_polls_once_per_interval_while_wanted() {
    let t0 = Instant::now();
    let mut mirror = MenuMirror::default();
    assert!(mirror.poll(t0, None, true).is_none(), "nothing before the channel agreed");
    let plain = Negotiated { version: 1, capabilities: Capabilities::NONE };
    assert!(mirror.poll(t0, Some(&plain), true).is_none(), "nothing when the host has no menus");
    assert!(mirror.poll(t0, Some(&menus_negotiated()), false).is_none(), "nothing while not wanted");
    let first = mirror.poll(t0, Some(&menus_negotiated()), true).expect("first poll at once");
    assert_eq!(
        MenuMessage::from_message(&first).unwrap().unwrap(),
        MenuMessage::Request { known_revision: 0 }
    );
    assert!(mirror.poll(t0 + POLL_INTERVAL / 2, Some(&menus_negotiated()), true).is_none());
    assert!(mirror.poll(t0 + POLL_INTERVAL, Some(&menus_negotiated()), true).is_some());
}

#[test]
fn the_mirror_reports_new_snapshots_and_ignores_unchanged() {
    let t0 = Instant::now();
    let mut mirror = MenuMirror::default();
    let snap = sample();
    let update = mirror.on_message(&MenuMessage::Snapshot(snap.clone()).to_message());
    assert_eq!(update, MirrorEvent::Changed(Some(std::sync::Arc::new(snap.clone()))));
    assert_eq!(mirror.current().map(|s| s.revision), Some(7));
    // The next poll names the revision it has.
    let next = mirror.poll(t0, Some(&menus_negotiated()), true).unwrap();
    assert_eq!(
        MenuMessage::from_message(&next).unwrap().unwrap(),
        MenuMessage::Request { known_revision: 7 }
    );
    assert_eq!(
        mirror.on_message(&MenuMessage::Unchanged { revision: 7 }.to_message()),
        MirrorEvent::NoChange,
        "unchanged is not an update"
    );
    // The host lost its menu (no frontmost app, or it went away): the mirror clears it once.
    assert_eq!(mirror.on_message(&MenuMessage::Empty.to_message()), MirrorEvent::Changed(None));
    assert_eq!(mirror.on_message(&MenuMessage::Empty.to_message()), MirrorEvent::NoChange);
    // Not ours.
    assert_eq!(
        mirror.on_message(&Message::Unknown { kind: MessageType(0x0301), payload: vec![] }),
        MirrorEvent::NotOurs
    );
}

#[test]
fn the_mirror_presses_only_items_of_its_snapshot() {
    let mut mirror = MenuMirror::default();
    assert!(mirror.press(3).is_none(), "no snapshot yet");
    mirror.on_message(&MenuMessage::Snapshot(sample()).to_message());
    let press = mirror.press(3).unwrap();
    assert_eq!(
        MenuMessage::from_message(&press).unwrap().unwrap(),
        MenuMessage::Press { revision: 7, id: 3 }
    );
    assert!(mirror.press(8).is_none(), "a disabled item");
    assert!(mirror.press(2).is_none(), "a submenu title");
    assert!(mirror.press(5).is_none(), "a separator");
    assert!(mirror.press(1000).is_none(), "unknown");
}

#[test]
fn reset_forgets_everything_for_a_new_leg() {
    let mut mirror = MenuMirror::default();
    mirror.on_message(&MenuMessage::Snapshot(sample()).to_message());
    mirror.reset();
    assert!(mirror.current().is_none());
    let poll = mirror.poll(Instant::now(), Some(&menus_negotiated()), true).unwrap();
    assert_eq!(
        MenuMessage::from_message(&poll).unwrap().unwrap(),
        MenuMessage::Request { known_revision: 0 }
    );
}

#[derive(Debug)]
struct Fixed(MessageType, Option<Vec<Message>>, Vec<Message>);

impl Responder for Fixed {
    fn on_negotiated(&mut self, _n: &Negotiated) -> Vec<Message> {
        self.2.clone()
    }
    fn on_message(&mut self, m: &Message) -> Option<Vec<Message>> {
        (m.kind() == self.0).then(|| self.1.clone().unwrap_or_default())
    }
}

#[test]
fn a_chain_asks_each_responder_in_turn() {
    let a = MessageType(0x0401);
    let b = MessageType(0x0201);
    let hello_a = Message::Unknown { kind: a, payload: vec![1] };
    let hello_b = Message::Unknown { kind: b, payload: vec![2] };
    let mut chain = Chain::new()
        .with(Box::new(Fixed(a, Some(vec![hello_a.clone()]), vec![hello_a.clone()])))
        .with(Box::new(Fixed(b, Some(vec![hello_b.clone()]), vec![hello_b.clone()])));
    let n = negotiate(
        &drift_ext::wire::Hello {
            min_version: 1,
            max_version: 1,
            capabilities: Capabilities::NONE,
            agent: "a".into(),
        },
        &drift_ext::wire::Hello {
            min_version: 1,
            max_version: 1,
            capabilities: Capabilities::NONE,
            agent: "b".into(),
        },
    )
    .unwrap();
    assert_eq!(chain.on_negotiated(&n), vec![hello_a.clone(), hello_b.clone()]);
    assert_eq!(chain.on_message(&Message::Unknown { kind: b, payload: vec![] }), Some(vec![hello_b]));
    assert_eq!(chain.on_message(&Message::Unknown { kind: a, payload: vec![] }), Some(vec![hello_a]));
    assert_eq!(chain.on_message(&Message::Unknown { kind: MessageType(0x0301), payload: vec![] }), None);
    let _ = Duration::ZERO;
}
