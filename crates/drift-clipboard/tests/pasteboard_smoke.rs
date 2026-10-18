//! M5-3 Red: smoke test of the NSPasteboard adapter on a private
//! `NSPasteboard pasteboardWithUniqueName` (never touches the user's clipboard).
#![cfg(all(feature = "macos", target_os = "macos"))]

use drift_clipboard::pasteboard::NsPasteboard;
use drift_clipboard::poll::{PasteboardPort, PasteboardWatcher};
use drift_clipboard::{ClipboardContents, ClipboardItem};
use drift_core::ClipboardPrefs;

const PNG: &[u8] = include_bytes!("fixtures/remote_clip_d011.png");

#[test]
fn text_round_trip_and_change_count() {
    let pb = NsPasteboard::unique();
    let before = pb.change_count();
    let c = ClipboardContents { items: vec![ClipboardItem::Text("Grüße ✓ 𝄞\nline2".into())] };
    let after = pb.write(&c);
    assert!(after > before, "changeCount must advance ({before} -> {after})");
    assert_eq!(pb.change_count(), after);
    assert_eq!(pb.read(ClipboardPrefs::TextAndImages), c);
    assert_eq!(pb.read(ClipboardPrefs::Off), ClipboardContents::empty());
}

#[test]
fn png_round_trip_also_offers_tiff() {
    let pb = NsPasteboard::unique();
    let c = ClipboardContents {
        items: vec![ClipboardItem::Text("cap".into()), ClipboardItem::Png(PNG.to_vec())],
    };
    pb.write(&c);
    let read = pb.read(ClipboardPrefs::TextAndImages);
    assert_eq!(read.text(), Some("cap"));
    assert!(
        read.items.contains(&ClipboardItem::Png(PNG.to_vec())),
        "PNG bytes unchanged: {:?}",
        kinds(&read)
    );
    // Text-only prefs never read images.
    assert_eq!(
        pb.read(ClipboardPrefs::Text),
        ClipboardContents { items: vec![ClipboardItem::Text("cap".into())] }
    );
}

#[test]
fn tiff_only_pasteboard_is_converted_to_png() {
    let pb = NsPasteboard::unique();
    // Write PNG, take the TIFF representation the adapter added for Mac apps, then write
    // TIFF alone.
    pb.write(&ClipboardContents { items: vec![ClipboardItem::Png(PNG.to_vec())] });
    // SAFETY: AppKit constant, initialised before main.
    let tiff_type = unsafe { objc2_app_kit::NSPasteboardTypeTIFF };
    let tiff = pb.raw().dataForType(tiff_type).expect("adapter offers TIFF alongside PNG").to_vec();
    pb.write(&ClipboardContents { items: vec![ClipboardItem::Tiff(tiff.clone())] });
    let read = pb.read(ClipboardPrefs::TextAndImages);
    let png = read
        .items
        .iter()
        .find_map(|i| if let ClipboardItem::Png(p) = i { Some(p.clone()) } else { None })
        .expect("TIFF-only pasteboard yields PNG");
    let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).expect("valid png");
    assert_eq!((img.width(), img.height()), (320, 200));
    assert!(read.items.contains(&ClipboardItem::Tiff(tiff)));
}

#[test]
fn watcher_on_real_pasteboard() {
    let pb = NsPasteboard::unique();
    let mut w = PasteboardWatcher::new(pb.change_count());
    assert_eq!(w.poll(&pb, ClipboardPrefs::TextAndImages), None);
    pb.write(&ClipboardContents { items: vec![ClipboardItem::Text("x".into())] });
    let change = w.poll(&pb, ClipboardPrefs::TextAndImages).expect("change detected");
    assert_eq!(change.contents.text(), Some("x"));
}

#[test]
fn general_pasteboard_is_reachable() {
    // Read-only: must not modify the user's clipboard.
    let pb = NsPasteboard::general();
    let _ = pb.change_count();
}

fn kinds(c: &ClipboardContents) -> Vec<&'static str> {
    c.items
        .iter()
        .map(|i| match i {
            ClipboardItem::Text(_) => "text",
            ClipboardItem::Png(_) => "png",
            ClipboardItem::Tiff(_) => "tiff",
        })
        .collect()
}
