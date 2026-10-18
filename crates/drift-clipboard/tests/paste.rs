//! IOS-10 Red: what one explicit paste on iOS hands to the session.
//!
//! iOS never lets Drift read the pasteboard on its own (that shows the paste prompt), so the
//! local → remote direction only happens when the user pastes into Drift (`UIPasteControl` or
//! the edit menu). The pasted item providers yield text and/or a PNG; the profile's clipboard
//! preference decides what is passed on, exactly as `PasteboardPort::read` does on macOS.
#![allow(missing_docs)]

use drift_clipboard::{ClipboardContents, ClipboardItem};
use drift_core::ClipboardPrefs;

#[test]
fn text_and_image_with_images_allowed() {
    let c = ClipboardContents::from_paste(
        Some("hello".into()),
        Some(vec![0x89, b'P']),
        ClipboardPrefs::TextAndImages,
    );
    assert_eq!(c.items, vec![ClipboardItem::Text("hello".into()), ClipboardItem::Png(vec![0x89, b'P'])]);
}

#[test]
fn text_only_preference_drops_the_image() {
    let c = ClipboardContents::from_paste(Some("hi".into()), Some(vec![1]), ClipboardPrefs::Text);
    assert_eq!(c.items, vec![ClipboardItem::Text("hi".into())]);
    let c = ClipboardContents::from_paste(None, Some(vec![1]), ClipboardPrefs::Text);
    assert!(c.is_empty());
}

#[test]
fn off_passes_nothing() {
    let c = ClipboardContents::from_paste(Some("hi".into()), Some(vec![1]), ClipboardPrefs::Off);
    assert!(c.is_empty());
}

#[test]
fn empty_text_and_empty_images_are_not_items() {
    let c =
        ClipboardContents::from_paste(Some(String::new()), Some(Vec::new()), ClipboardPrefs::TextAndImages);
    assert!(c.is_empty());
}

#[test]
fn crlf_is_normalised_to_lf() {
    let c = ClipboardContents::from_paste(Some("a\r\nb".into()), None, ClipboardPrefs::Text);
    assert_eq!(c.text(), Some("a\nb"));
}
