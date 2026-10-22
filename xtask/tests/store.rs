//! STORE-1..4: App Store and Developer ID packaging, described as data and checked here.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use xtask::store::{self, TEAM_ID};

fn read(rel: &str) -> String {
    let p = xtask::repo::root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()))
}

/// The value after `<key>{key}</key>` in a plist (`<string>`, `<true/>` or `<false/>`).
fn plist_value(plist: &str, key: &str) -> Option<String> {
    let rest = plist.split(&format!("<key>{key}</key>")).nth(1)?.trim_start();
    if rest.starts_with("<true/>") {
        return Some("true".into());
    }
    if rest.starts_with("<false/>") {
        return Some("false".into());
    }
    let s = rest.strip_prefix("<string>")?;
    Some(s.split("</string>").next()?.to_owned())
}

// ---- STORE-1: Mac App Store --------------------------------------------------------------------

#[test]
fn mas_entitlements_keep_the_sandbox_and_name_the_app_and_team() {
    let e = read("src-tauri/entitlements.mas.plist");
    assert_eq!(plist_value(&e, "com.apple.security.app-sandbox").as_deref(), Some("true"));
    assert_eq!(plist_value(&e, "com.apple.security.network.client").as_deref(), Some("true"));
    assert_eq!(plist_value(&e, "com.apple.security.network.server"), None, "the client never listens");
    assert_eq!(
        plist_value(&e, "com.apple.application-identifier").as_deref(),
        Some(format!("{TEAM_ID}.com.hutsonlabs.drift").as_str())
    );
    assert_eq!(plist_value(&e, "com.apple.developer.team-identifier").as_deref(), Some(TEAM_ID));
}

/// The release signs and preflights the DMG with `store::DEVELOPER_ID`; Tauri signs the app with
/// the identity pinned in `tauri.conf.json`. They must be the same certificate.
#[test]
fn the_release_and_the_bundle_pin_the_same_developer_id() {
    let conf: serde_json::Value = serde_json::from_str(&read("src-tauri/tauri.conf.json")).unwrap();
    assert_eq!(conf["bundle"]["macOS"]["signingIdentity"], store::DEVELOPER_ID);
    assert_eq!(store::DEVELOPER_ID, "DABE738AA854E6CE4FA33E69C1F0AFC5830C8646");
}

#[test]
fn mas_config_signs_with_apple_distribution_and_the_mas_entitlements() {
    let conf: serde_json::Value = serde_json::from_str(&read("src-tauri/tauri.mas.conf.json")).unwrap();
    let mac = &conf["bundle"]["macOS"];
    assert_eq!(mac["signingIdentity"], store::APPLE_DISTRIBUTION);
    assert_eq!(mac["entitlements"], "entitlements.mas.plist");
    assert_eq!(conf["bundle"]["targets"], serde_json::json!(["app"]));
}

#[test]
fn the_short_description_names_gnome() {
    let conf: serde_json::Value = serde_json::from_str(&read("src-tauri/tauri.conf.json")).unwrap();
    let d = conf["bundle"]["shortDescription"].as_str().unwrap();
    assert!(d.contains("GNOME") && !d.contains("Drift Host"), "{d}");
}

#[test]
fn the_mas_package_is_universal_and_signed_by_the_installer_identity() {
    let p = store::mas_plan(Path::new("/repo"), None, None);
    let line = p.tauri_args.join(" ");
    assert!(line.contains("--target universal-apple-darwin"), "{line}");
    assert!(line.contains("--bundles app"), "{line}");
    assert!(line.contains("--config tauri.mas.conf.json"), "{line}");
    assert_eq!(p.app, Path::new("/repo/target/universal-apple-darwin/release/bundle/macos/Drift.app"));
    assert_eq!(p.pkg, Path::new("/repo/target/universal-apple-darwin/release/bundle/mas/Drift.pkg"));
    let pb = p.productbuild_args.join(" ");
    assert!(pb.starts_with("--component "), "{pb}");
    assert!(pb.contains(" /Applications "), "{pb}");
    assert!(pb.contains(&format!("--sign {}", store::MAC_INSTALLER)), "{pb}");
    assert!(p.embed_profile.is_none(), "no profile given, none embedded");
}

#[test]
fn a_given_profile_is_embedded_in_the_app() {
    let p = store::mas_plan(Path::new("/repo"), Some(Path::new("/profiles/drift.provisionprofile")), None);
    let (from, to) = p.embed_profile.unwrap();
    assert_eq!(from, Path::new("/profiles/drift.provisionprofile"));
    assert_eq!(
        to,
        Path::new(
            "/repo/target/universal-apple-darwin/release/bundle/macos/Drift.app/Contents/embedded.provisionprofile"
        )
    );
}

// ---- STORE-2: review risk ------------------------------------------------------------------------

#[test]
fn the_risk_scan_names_private_webview_keys_found_in_a_binary() {
    let binary = b"\x00\x01allowsPictureInPictureMediaPlaybackdrawsBackgroundfullScreenEnabled\x00setTitlebarAppearsTransparent:\x00";
    let hits = store::review_risks(binary);
    let names: Vec<&str> = hits.iter().map(|r| r.needle).collect();
    assert_eq!(names, ["drawsBackground"], "{hits:?}");
    assert!(hits[0].why.contains("transparent"), "{}", hits[0].why);
}

#[test]
fn a_clean_binary_has_no_review_risks() {
    assert!(store::review_risks(b"setTitlebarAppearsTransparent: NSVisualEffectView").is_empty());
}

// ---- STORE-3: iOS App Store ---------------------------------------------------------------------

/// Apple's required-reason API categories and the reason codes Drift declares for them.
const REQUIRED_REASONS: &[(&str, &str)] = &[
    // Rust std / tokio read file metadata (`stat`, `fstat`) for files in the app container.
    ("NSPrivacyAccessedAPICategoryFileTimestamp", "C617.1"),
    // `systemUptime` / `Instant`: elapsed time only (event timestamps, frame pacing).
    ("NSPrivacyAccessedAPICategorySystemBootTime", "35F9.1"),
    // The app's own standard defaults (`standardUserDefaults`).
    ("NSPrivacyAccessedAPICategoryUserDefaults", "CA92.1"),
    // `activeInputModes`: the UI adapts to the active keyboard (text input over the picture).
    ("NSPrivacyAccessedAPICategoryActiveKeyboards", "54BD.1"),
];

#[test]
fn the_ios_app_ships_a_privacy_manifest_without_tracking() {
    let m = read("src-tauri/gen/apple/drift-app_iOS/PrivacyInfo.xcprivacy");
    assert_eq!(plist_value(&m, "NSPrivacyTracking").as_deref(), Some("false"));
    assert!(m.contains("<key>NSPrivacyTrackingDomains</key>\n\t<array/>"), "no tracking domains");
    assert!(m.contains("<key>NSPrivacyCollectedDataTypes</key>\n\t<array/>"), "Drift collects no data");
    for (category, reason) in REQUIRED_REASONS {
        let entry = m.split(category).nth(1).unwrap_or_else(|| panic!("{category} not declared"));
        let reasons = entry.split("</array>").next().unwrap_or_default();
        assert!(reasons.contains(&format!("<string>{reason}</string>")), "{category} lacks reason {reason}");
    }
    let pbxproj = read("src-tauri/gen/apple/drift-app.xcodeproj/project.pbxproj");
    assert!(pbxproj.contains("PrivacyInfo.xcprivacy in Resources"), "the app must bundle the manifest");
}

#[test]
fn the_rust_static_library_is_linked_not_bundled() {
    // xcodegen bundled `Externals/*/libapp.a` (900 MB) as a resource whenever the project was
    // regenerated after a build.
    let pbxproj = read("src-tauri/gen/apple/drift-app.xcodeproj/project.pbxproj");
    assert!(pbxproj.contains("libapp.a in Frameworks"));
    assert!(!pbxproj.contains("libapp.a in Resources"), "libapp.a is copied into the app bundle");
}

#[test]
fn ios_export_options_export_for_app_store_connect_and_never_upload() {
    let e = read("src-tauri/gen/apple/ExportOptions.plist");
    assert_eq!(plist_value(&e, "method").as_deref(), Some("app-store-connect"));
    assert_eq!(plist_value(&e, "destination").as_deref(), Some("export"), "validate only: never upload");
    assert_eq!(plist_value(&e, "teamID").as_deref(), Some(TEAM_ID));
}

#[test]
fn the_private_transparency_key_is_a_macos_only_feature() {
    // STORE-2: Tauri's `macos-private-api` turns on wry's private `drawsBackground` key; iOS makes
    // the page see-through with public API instead (drift-ios), so iOS builds must not enable it.
    let manifest = read("src-tauri/Cargo.toml").parse::<toml::Table>().unwrap();
    let common = manifest["dependencies"]["tauri"]["features"].as_array().unwrap();
    assert!(!common.iter().any(|f| f.as_str() == Some("macos-private-api")), "{common:?}");
    // drift-macos, a macOS-only dependency of the app, turns it on through its `tauri` feature.
    let mac_deps = &manifest["target"]["cfg(target_os = \"macos\")"]["dependencies"];
    assert!(
        mac_deps["drift-macos"]["features"].as_array().unwrap().iter().any(|f| f.as_str() == Some("tauri"))
    );
    let platform = read("crates/drift-macos/Cargo.toml").parse::<toml::Table>().unwrap();
    let features = platform["dependencies"]["tauri"]["features"].as_array().unwrap();
    assert!(features.iter().any(|f| f.as_str() == Some("macos-private-api")));
}

#[test]
fn mas_plan_sets_the_build_number_through_a_config_override() {
    let p = store::mas_plan(Path::new("/repo"), None, Some(3));
    let tail = &p.tauri_args[p.tauri_args.len() - 2..];
    assert_eq!(tail, ["--config", r#"{"bundle":{"macOS":{"bundleVersion":"3"}}}"#]);
}
