//! HOST-11 Red: the host's CLIPRDR server role, reusing drift-clipboard's sync state. A copy on
//! the host Mac reaches the client, and a copy on the client lands on the host's pasteboard.
//! The host's pasteboard is a fake port: tests never touch the real general pasteboard.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[path = "support_host.rs"]
mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_clipboard::poll::PasteboardPort;
use drift_clipboard::{ClipboardContents, ClipboardItem};
use drift_core::ClipboardPrefs;
use drift_rdp::{SessionCommand, SessionEvent, SessionOptions};
use support::*;

#[derive(Debug, Default)]
struct FakeBoard {
    change_count: i64,
    contents: ClipboardContents,
    writes: Vec<ClipboardContents>,
}

#[derive(Clone, Default)]
struct FakePasteboard(Arc<Mutex<FakeBoard>>);

impl FakePasteboard {
    fn copy(&self, text: &str) {
        let mut b = self.0.lock().unwrap();
        b.change_count += 1;
        b.contents = ClipboardContents { items: vec![ClipboardItem::Text(text.to_owned())] };
    }
    fn written_text(&self) -> Vec<String> {
        self.0.lock().unwrap().writes.iter().filter_map(|c| c.text().map(str::to_owned)).collect()
    }
}

impl PasteboardPort for FakePasteboard {
    fn change_count(&self) -> i64 {
        self.0.lock().unwrap().change_count
    }
    fn read(&self, _level: ClipboardPrefs) -> ClipboardContents {
        self.0.lock().unwrap().contents.clone()
    }
    fn write(&self, contents: &ClipboardContents) -> i64 {
        let mut b = self.0.lock().unwrap();
        b.change_count += 1;
        b.contents = contents.clone();
        b.writes.push(contents.clone());
        b.change_count
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn host_and_client_copies_cross_in_both_directions() {
    let dir = tempfile::tempdir().unwrap();
    let board = FakePasteboard::default();
    let factory_board = board.clone();
    let (host, identity) = start_host(
        dir.path(),
        Some(Box::new(move || Box::new(factory_board.clone()) as Box<dyn PasteboardPort>)),
        None,
    );
    let (handle, mut events) = connect(&host, &identity, SessionOptions::default());
    wait_event(&mut events, "connected", connected).await;
    handle.send(SessionCommand::Focus(true)).unwrap();

    // Host copy → the client fetches it eagerly and reports it for its own pasteboard.
    board.copy("copied on the host Mac");
    let got = wait_event(
        &mut events,
        "the host's copy at the client",
        |e| matches!(e, SessionEvent::ClipboardRemote(c) if c.text() == Some("copied on the host Mac")),
    )
    .await;
    assert!(matches!(got, SessionEvent::ClipboardRemote(_)));

    // Client copy → the host fetches it and writes its pasteboard.
    handle
        .send(SessionCommand::ClipboardLocalChanged(ClipboardContents {
            items: vec![ClipboardItem::Text("copied on the iPad".into())],
        }))
        .unwrap();
    wait_until("the client's copy on the host pasteboard", Duration::from_secs(15), || {
        board.written_text().iter().any(|t| t == "copied on the iPad")
    })
    .await;
    // Our own write is not echoed back to the client as a new host copy.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(board.written_text(), ["copied on the iPad"]);
    assert!(host.stats().clipboard_ready, "{:?}", host.stats());
    let _ = handle.send(SessionCommand::Close);
}
