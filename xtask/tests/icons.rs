//! M9-4 Red: the app icon. APP-icon: the source is the Icon Composer file
//! `src-tauri/icons/AppIcon.icon`, bundled by the Mac and iOS apps; the flat PNGs and `icon.icns`
//! are rendered from it, and no Windows or Android icon artefacts are committed.
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

/// APP-icon: the Icon Composer file is the one source of the icon, and every layer it draws is
/// committed with it.
#[test]
fn the_icon_composer_file_is_the_source_of_the_icon() {
    let source = xtask::repo::root().join(icons::SOURCE);
    assert!(source.ends_with("AppIcon.icon"), "the source is {}", source.display());
    let problems = icons::icon_composer_audit(&source);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn an_icon_composer_file_without_its_layers_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let icon = dir.path().join("AppIcon.icon");
    std::fs::create_dir_all(icon.join("Assets")).unwrap();
    std::fs::write(
        icon.join("icon.json"),
        r#"{"groups":[{"layers":[{"image-name":"waves.svg","name":"waves"}]}]}"#,
    )
    .unwrap();
    let problems = icons::icon_composer_audit(&icon);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("waves.svg"), "{problems:?}");
    assert!(!icons::icon_composer_audit(&dir.path().join("missing.icon")).is_empty());
}

/// APP-icon: the iPhone and iPad app use the same Icon Composer file as the Mac (actool renders
/// the flat fallbacks), and no PNG appiconset of the same name competes with it.
#[test]
fn the_ios_app_icon_is_the_icon_composer_file() {
    let problems = icons::ios_audit(&xtask::repo::root().join("src-tauri"));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
