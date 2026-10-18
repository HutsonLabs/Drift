//! EXT-3 Red: the host's side of menu mirroring — turning an Accessibility menu tree into a
//! snapshot, answering the client's polls and presses only while the connection is authorized,
//! reading a real app's menu bar, and the whole loop with Drift's client on loopback.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_core::{ConnectMode, ConnectionProfile, SessionState, Size, SystemClock};
use drift_ext::channel::Responder;
use drift_ext::menus::{MenuItem, MenuMessage, MenuSnapshot, Modifiers};
use drift_ext::wire::{Capabilities, Message};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::menus::{AxNode, MenuResponder, MenuSource, StaticMenus, snapshot_from_ax};
use drift_host::server::{Host, HostOptions};
use drift_rdp::{SessionCommand, SessionEvent, SessionOptions, SessionSecrets};
use drift_testkit::{PresentMode, RecordingFrameSink};

fn node(role: &str, title: &str, children: Vec<AxNode>) -> AxNode {
    AxNode {
        role: role.into(),
        title: title.into(),
        enabled: true,
        marked: false,
        cmd_char: None,
        cmd_modifiers: None,
        children,
    }
}

fn menu(children: Vec<AxNode>) -> AxNode {
    node("AXMenu", "", children)
}

fn text_edit_bar() -> AxNode {
    let mut save = node("AXMenuItem", "Save", vec![]);
    save.cmd_char = Some("S".into());
    save.cmd_modifiers = Some(0);
    let mut save_as = node("AXMenuItem", "Save As…", vec![]);
    save_as.cmd_char = Some("S".into());
    save_as.cmd_modifiers = Some(1);
    let mut separator = node("AXMenuItem", "", vec![]);
    separator.enabled = false;
    let mut revert =
        node("AXMenuItem", "Revert To", vec![menu(vec![node("AXMenuItem", "Last Saved", vec![])])]);
    revert.enabled = true;
    let mut ruler = node("AXMenuItem", "Show Ruler", vec![]);
    ruler.marked = true;
    node(
        "AXMenuBar",
        "",
        vec![
            node("AXMenuBarItem", "Apple", vec![menu(vec![node("AXMenuItem", "About This Mac", vec![])])]),
            node("AXMenuBarItem", "TextEdit", vec![menu(vec![node("AXMenuItem", "About TextEdit", vec![])])]),
            node("AXMenuBarItem", "File", vec![menu(vec![save, save_as, separator, revert])]),
            node("AXMenuBarItem", "Format", vec![menu(vec![ruler])]),
        ],
    )
}

#[test]
fn an_ax_tree_becomes_a_snapshot_without_the_apple_menu() {
    let (snap, paths) = snapshot_from_ax("TextEdit", &text_edit_bar(), 3);
    assert_eq!(snap.revision, 3);
    assert_eq!(snap.app, "TextEdit");
    let titles: Vec<_> = snap.menus.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(titles, ["TextEdit", "File", "Format"], "the Apple menu belongs to the iPad, not the Mac app");
    let file = &snap.menus[1];
    assert_eq!(file.children.len(), 4);
    let save = &file.children[0];
    assert_eq!(save.key.as_ref().map(|k| k.label()), Some("⌘S".to_owned()));
    assert_eq!(
        file.children[1].key.as_ref().map(|k| k.modifiers),
        Some(Modifiers::COMMAND | Modifiers::SHIFT)
    );
    assert!(file.children[2].separator, "an untitled disabled item is a separator");
    assert_eq!(file.children[3].children.len(), 1, "submenus keep their items");
    assert!(snap.menus[2].children[0].checked);
    // Ids are unique, and every id has a path back to its element for pressing.
    let mut ids = Vec::new();
    collect_ids(&snap.menus, &mut ids);
    let unique: std::collections::BTreeSet<_> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len());
    for id in ids {
        assert!(paths.contains_key(&id), "no element path for item {id}");
    }
}

fn collect_ids(items: &[MenuItem], out: &mut Vec<u32>) {
    for i in items {
        out.push(i.id);
        collect_ids(&i.children, out);
    }
}

fn snapshot(revision: u32) -> MenuSnapshot {
    snapshot_from_ax("TextEdit", &text_edit_bar(), revision).0
}

fn request(known: u32) -> Message {
    MenuMessage::Request { known_revision: known }.to_message()
}

fn reply(r: Option<Vec<Message>>) -> Vec<MenuMessage> {
    r.expect("menu messages are consumed")
        .iter()
        .map(|m| MenuMessage::from_message(m).unwrap().unwrap())
        .collect()
}

#[test]
fn the_responder_answers_polls_and_presses_only_when_authorized() {
    let source = Arc::new(StaticMenus::new(Some(snapshot(5))));
    let authorized = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&authorized);
    let mut responder = MenuResponder::new(source.clone(), Arc::new(move || flag.load(Ordering::SeqCst)));

    assert_eq!(reply(responder.on_message(&request(0))), [], "a stranger learns nothing");
    assert_eq!(reply(responder.on_message(&MenuMessage::Press { revision: 5, id: 4 }.to_message())), []);
    assert!(source.presses().is_empty(), "a stranger presses nothing");

    authorized.store(true, Ordering::SeqCst);
    assert_eq!(reply(responder.on_message(&request(0))), [MenuMessage::Snapshot(snapshot(5))]);
    assert_eq!(reply(responder.on_message(&request(5))), [MenuMessage::Unchanged { revision: 5 }]);
    assert_eq!(reply(responder.on_message(&MenuMessage::Press { revision: 5, id: 4 }.to_message())), []);
    assert_eq!(source.presses(), [(5, 4)]);

    source.set(None);
    assert_eq!(reply(responder.on_message(&request(5))), [MenuMessage::Empty]);
    // Not a menu message: left for someone else.
    assert!(
        responder
            .on_message(&Message::Unknown { kind: drift_ext::wire::MessageType(0x0401), payload: vec![] })
            .is_none()
    );
}

#[test]
fn the_frontmost_apps_menu_bar_reads_through_accessibility() {
    if !drift_host::menus::ax::is_trusted() {
        eprintln!("skipped: this process has no Accessibility permission (docs/acceptance.md, EXT-3)");
        return;
    }
    let Some(reading) = drift_host::menus::ax::read_frontmost(1) else {
        eprintln!("skipped: no frontmost app with a menu bar");
        return;
    };
    let snap = reading.snapshot;
    eprintln!("frontmost: {} with {} menus, {} items", snap.app, snap.menus.len(), snap.item_count());
    assert!(!snap.app.is_empty());
    assert!(!snap.menus.is_empty(), "every app has at least its app menu");
    assert!(snap.menus.iter().all(|m| !m.title.is_empty()));
    assert!(MenuMessage::Snapshot(snap).try_to_message().is_ok(), "a real menu bar fits in one message");
}

const USER: &str = "drift-host-test";
const PASS: &str = "fake-menus-password";

#[tokio::test(flavor = "multi_thread")]
async fn the_client_mirrors_the_hosts_menus_and_presses_an_item() {
    let dir = tempfile::tempdir().unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let source = Arc::new(StaticMenus::new(Some(snapshot(9))));
    let host = Host::start(HostOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        identity: identity.clone(),
        username: USER.into(),
        password: zeroize::Zeroizing::new(PASS.to_owned()),
        desktop: Size::new(1280, 800),
        frames: Box::new(|| Box::new(TestPattern::new())),
        live: None,
        input: Box::new(RecordingInput::default()),
        clipboard: None,
        audio: None,
        displays: Box::new(drift_host::display::PhysicalOnly),
        pairing: None,
        menus: Some(source.clone() as Arc<dyn MenuSource>),
    })
    .unwrap();

    let mut p = ConnectionProfile::new("Drift Host", "127.0.0.1", ConnectMode::Headless);
    p.port = host.port();
    USER.clone_into(&mut p.rdp_username);
    p.cert_pin = Some(identity.fingerprint());
    let (handle, mut events) = drift_rdp::spawn_session(
        p,
        SessionSecrets::new(PASS),
        Box::new(RecordingFrameSink::new(PresentMode::Immediate).0),
        Arc::new(SystemClock),
        SessionOptions {
            connect_timeout: Duration::from_secs(15),
            reconnect_seed: Some(5),
            drift_mac: Some(drift_ext::channel::Link::new()),
            menus: true,
            ..SessionOptions::default()
        },
    );
    let _ = handle.send(SessionCommand::Focus(true));

    let seen = Arc::new(Mutex::new(None));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SessionEvent::Menu(Some(snap)))) => {
                *seen.lock().unwrap() = Some(snap);
                break;
            }
            Ok(Some(SessionEvent::State(SessionState::Failed { reason }))) => panic!("failed: {reason:?}"),
            Ok(Some(_)) => {}
            other => panic!("no menu snapshot: {other:?}; host {:?}", host.stats()),
        }
    }
    let snap = seen.lock().unwrap().clone().unwrap();
    assert_eq!(snap.app, "TextEdit");
    let save = snap.menus[1].children[0].id;
    let _ = handle.send(SessionCommand::MenuPress(save));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while source.presses().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "the press never reached the host");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(source.presses(), [(9, save)]);

    // The Mac's frontmost app changes its menus: the client gets the new revision.
    let mut next = snapshot(10);
    next.app = "Safari".into();
    source.set(Some(next));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SessionEvent::Menu(Some(snap)))) if snap.app == "Safari" => break,
            Ok(Some(_)) => {}
            other => panic!("no updated snapshot: {other:?}"),
        }
    }
    let _ = Capabilities::MENUS;
    let _ = handle.send(SessionCommand::Close);
}
