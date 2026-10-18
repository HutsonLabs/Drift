//! IOS-8 Red: the iPhone layout (picture on top, trackpad and key row below in portrait; full
//! screen with auto-hiding controls in landscape) and the soft key row's key events.
#![allow(clippy::unwrap_used)]

use drift_app::phone::{
    AUTO_HIDE_SECS, AutoHide, Frame, Insets, KEY_ROW_HEIGHT, PhoneLayout, SoftKey, chord_events,
    phone_layout, text_events,
};
use drift_core::{DesktopSize, InputEvent, Size};

/// iPhone 17 Pro portrait: 402×874 points, 62 pt status area, 34 pt home indicator.
const PORTRAIT: Size<f64> = Size { width: 402.0, height: 874.0 };
const PORTRAIT_SAFE: Insets = Insets { top: 62.0, left: 0.0, bottom: 34.0, right: 0.0 };
const LANDSCAPE: Size<f64> = Size { width: 874.0, height: 402.0 };
const LANDSCAPE_SAFE: Insets = Insets { top: 0.0, left: 62.0, bottom: 21.0, right: 62.0 };
const DESKTOP: DesktopSize = DesktopSize { width: 1600, height: 1000 };

fn frame(x: f64, y: f64, width: f64, height: f64) -> Frame {
    Frame { x, y, width, height }
}

#[test]
fn portrait_puts_the_picture_on_top_and_the_pad_and_keys_below() {
    let l = phone_layout(PORTRAIT, PORTRAIT_SAFE, Some(DESKTOP), true);
    // Full width, desktop aspect (402 × 1000/1600 = 251.25), just below the status area.
    assert_eq!(l.picture, frame(0.0, 62.0, 402.0, 251.25));
    let keys = l.keys.expect("key row in portrait");
    assert_eq!(keys, frame(0.0, 874.0 - 34.0 - KEY_ROW_HEIGHT, 402.0, KEY_ROW_HEIGHT));
    let pad = l.pad.expect("trackpad in portrait");
    assert_eq!(pad, frame(0.0, 62.0 + 251.25, 402.0, keys.y - (62.0 + 251.25)));
}

#[test]
fn portrait_picture_never_takes_more_than_sixty_percent() {
    let tall = DesktopSize { width: 800, height: 1600 };
    let l = phone_layout(PORTRAIT, PORTRAIT_SAFE, Some(tall), true);
    let usable = 874.0 - 62.0 - 34.0;
    assert!((l.picture.height - usable * 0.6).abs() < 1e-9, "{:?}", l.picture);
    assert!(l.pad.unwrap().height > 100.0);
}

#[test]
fn portrait_without_a_desktop_uses_sixteen_by_ten() {
    let l = phone_layout(PORTRAIT, PORTRAIT_SAFE, None, true);
    assert!((l.picture.height - 402.0 * 10.0 / 16.0).abs() < 1e-9);
}

#[test]
fn portrait_keeps_the_key_row_even_when_controls_are_hidden() {
    // Auto-hide is a landscape behaviour; in portrait the controls are the layout.
    let l = phone_layout(PORTRAIT, PORTRAIT_SAFE, Some(DESKTOP), false);
    assert!(l.keys.is_some() && l.pad.is_some());
}

#[test]
fn landscape_is_full_screen_with_controls_floating_at_the_bottom() {
    let shown = phone_layout(LANDSCAPE, LANDSCAPE_SAFE, Some(DESKTOP), true);
    assert_eq!(shown.picture, frame(0.0, 0.0, 874.0, 402.0));
    assert_eq!(shown.pad, None);
    assert_eq!(shown.keys, Some(frame(62.0, 402.0 - 21.0 - KEY_ROW_HEIGHT, 874.0 - 124.0, KEY_ROW_HEIGHT)));
    let hidden = phone_layout(LANDSCAPE, LANDSCAPE_SAFE, Some(DESKTOP), false);
    assert_eq!(hidden, PhoneLayout { picture: shown.picture, pad: None, keys: None });
}

#[test]
fn degenerate_sizes_do_not_panic() {
    let l = phone_layout(Size { width: 0.0, height: 0.0 }, Insets::default(), None, true);
    assert!(l.picture.width >= 0.0 && l.picture.height >= 0.0);
}

fn key(code: u8, extended: bool, down: bool) -> InputEvent {
    InputEvent::Key { scancode: code, extended, down }
}

#[test]
fn chords_press_in_order_and_release_in_reverse() {
    assert_eq!(
        chord_events(&[SoftKey::Control, SoftKey::Alt, SoftKey::Delete]),
        vec![
            key(0x1D, false, true),
            key(0x38, false, true),
            key(0x53, true, true),
            key(0x53, true, false),
            key(0x38, false, false),
            key(0x1D, false, false),
        ]
    );
    assert_eq!(chord_events(&[SoftKey::Escape]), vec![key(0x01, false, true), key(0x01, false, false)]);
    assert_eq!(chord_events(&[]), vec![]);
}

#[test]
fn every_soft_key_has_a_scancode() {
    use SoftKey::*;
    let expected = [
        (Escape, 0x01, false),
        (Tab, 0x0F, false),
        (Control, 0x1D, false),
        (Alt, 0x38, false),
        (Super, 0x5B, true),
        (Shift, 0x2A, false),
        (Up, 0x48, true),
        (Down, 0x50, true),
        (Left, 0x4B, true),
        (Right, 0x4D, true),
        (Backspace, 0x0E, false),
        (Enter, 0x1C, false),
        (Delete, 0x53, true),
    ];
    for (k, code, ext) in expected {
        assert_eq!(chord_events(&[k])[0], key(code, ext, true), "{k:?}");
    }
}

#[test]
fn typed_text_goes_out_as_unicode_with_newlines_as_return() {
    assert_eq!(
        text_events("a\né"),
        vec![
            InputEvent::Unicode { ch: u16::from(b'a'), down: true },
            InputEvent::Unicode { ch: u16::from(b'a'), down: false },
            key(0x1C, false, true),
            key(0x1C, false, false),
            InputEvent::Unicode { ch: 0xE9, down: true },
            InputEvent::Unicode { ch: 0xE9, down: false },
        ]
    );
    // Outside the BMP: a surrogate pair, each unit pressed and released.
    assert_eq!(text_events("😀").len(), 4);
}

#[test]
fn landscape_controls_hide_after_a_quiet_spell() {
    let mut a = AutoHide::default();
    assert!(!a.visible(0.0));
    a.touch(10.0);
    assert!(a.visible(10.0 + AUTO_HIDE_SECS - 0.1));
    assert!(!a.visible(10.0 + AUTO_HIDE_SECS + 0.1));
    a.touch(13.0);
    assert!(a.visible(16.0), "use keeps them up");
    a.hide();
    assert!(!a.visible(13.5));
}
