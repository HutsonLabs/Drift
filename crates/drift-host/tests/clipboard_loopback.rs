//! HOST-11 Red: the host's CLIPRDR server role, reusing drift-clipboard's sync state. A copy on
//! the host Mac reaches the client, and a copy on the client lands on the host's pasteboard.
//! The host's pasteboard is a fake port: tests never touch the real general pasteboard.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[path = "support/host.rs"]
mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_clipboard::poll::PasteboardPort;
use drift_clipboard::{ClipboardContents, ClipboardItem};
use drift_core::ClipboardPrefs;
use drift_rdp::{SessionCommand, SessionEvent, SessionOptions};
use support::*;

/// A 2×2 RGBA PNG (red, green): a valid image for the PNG/DIB path. Test data only.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00,
    0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xb6, 0x0d, 0x24, 0x00,
    0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x0a, 0x21,
    0x14, 0x00, 0x45, 0xcc, 0x07, 0xf9, 0x66, 0xcb, 0xff, 0x39, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

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
    fn copy_image(&self) {
        let mut b = self.0.lock().unwrap();
        b.change_count += 1;
        b.contents = ClipboardContents { items: vec![ClipboardItem::Png(TINY_PNG.to_vec())] };
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

    // An image copied on the host arrives as an image (PNG, or DIB converted back to PNG).
    board.copy_image();
    wait_event(&mut events, "the host's image at the client", |e| {
        matches!(e, SessionEvent::ClipboardRemote(c) if c.items.iter().any(|i| matches!(i, ClipboardItem::Png(_))))
    })
    .await;

    // Client copy → the host fetches it and writes its pasteboard. Wait out the echo window
    // first: a text list within 500 ms of our own text advertisement counts as its echo (M5-1).
    tokio::time::sleep(drift_clipboard::sync::ECHO_WINDOW + Duration::from_millis(200)).await;
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
