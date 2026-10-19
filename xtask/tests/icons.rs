//! M9-4 Red: the app icon. Drift ships a hand-made drift/wave glyph, generated from a vector
//! source with `cargo tauri icon`, and — plan §0, macOS only — no Windows, Android or iOS
//! icon artefacts.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use xtask::icons;

#[test]
fn png_size_reads_the_ihdr() {
    let png = std::fs::read(xtask::repo::root().join("src-tauri/icons/32x32.png")).unwrap();
    assert_eq!(icons::png_size(&png), Some((32, 32)));
    assert_eq!(icons::png_size(b"not a png"), None);
    assert_eq!(icons::png_size(&png[..20]), None, "a truncated header is not a size");
}

#[test]
fn icns_files_are_recognised_by_their_magic() {
    let icns = std::fs::read(xtask::repo::root().join("src-tauri/icons/icon.icns")).unwrap();
    assert!(icons::is_icns(&icns));
    assert!(!icons::is_icns(b"icn"));
    assert!(!icons::is_icns(b"PNG-not-icns"));
}

#[test]
fn the_bundled_icon_set_is_complete_and_macos_only() {
    let root = xtask::repo::root();
    let problems = icons::audit(&root.join("src-tauri"));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_vector_source_is_committed_so_the_icons_can_be_regenerated() {
    // `cargo tauri icon src-tauri/icons/icon.svg` must reproduce the bundled set.
    let svg = std::fs::read_to_string(xtask::repo::root().join(icons::SOURCE)).expect("the icon source");
    assert!(svg.contains("<svg"), "the source is an SVG");
    assert!(svg.len() > 200, "the source is a real drawing, not a placeholder");
}

/// IOS-fix: the iPhone and iPad icon is Drift's waves (`icon-ios.svg`, full bleed), opaque, at
/// every size `Contents.json` lists, and never Tauri's default logo again (TestFlight 26.10 (4)).
#[test]
fn the_ios_app_icon_is_drifts_opaque_waves() {
    let problems = icons::ios_audit(&xtask::repo::root().join("src-tauri"));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
