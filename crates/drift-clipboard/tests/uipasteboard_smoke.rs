//! IOS-10 Red: smoke test of the UIPasteboard adapter on a private, uniquely named pasteboard
//! (never the user's general pasteboard). Runs in an iOS simulator via `xcrun simctl spawn`.
#![cfg(target_os = "ios")]
#![allow(missing_docs, clippy::unwrap_used)]

use drift_clipboard::poll::PasteboardPort as _;
use drift_clipboard::uipasteboard::UiPasteboard;
use drift_clipboard::{ClipboardContents, ClipboardItem};
use drift_core::ClipboardPrefs;

/// A 1×1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00,
    0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05,
    0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42,
    0x60, 0x82,
];

#[test]
fn text_round_trips_and_bumps_the_change_count() {
    let pb = UiPasteboard::unique();
    let before = pb.change_count();
    let after = pb.write(&ClipboardContents { items: vec![ClipboardItem::Text("fake text ü".into())] });
    assert!(after > before);
    assert_eq!(pb.read(ClipboardPrefs::Text).text(), Some("fake text ü"));
    assert!(pb.has_contents(ClipboardPrefs::Text));
}

#[test]
fn png_round_trips_only_when_images_are_wanted() {
    let pb = UiPasteboard::unique();
    pb.write(&ClipboardContents { items: vec![ClipboardItem::Png(PNG.to_vec())] });
    let read = pb.read(ClipboardPrefs::TextAndImages);
    assert!(read.items.iter().any(|i| matches!(i, ClipboardItem::Png(p) if p.starts_with(b"\x89PNG"))));
    assert!(pb.read(ClipboardPrefs::Text).is_empty());
    assert!(pb.has_contents(ClipboardPrefs::TextAndImages));
    assert!(!pb.has_contents(ClipboardPrefs::Text));
}

#[test]
fn off_reads_nothing() {
    let pb = UiPasteboard::unique();
    pb.write(&ClipboardContents { items: vec![ClipboardItem::Text("x".into())] });
    assert!(pb.read(ClipboardPrefs::Off).is_empty());
    assert!(!pb.has_contents(ClipboardPrefs::Off));
}
