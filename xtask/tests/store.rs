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

#[test]
fn mas_config_signs_with_apple_distribution_and_the_mas_entitlements() {
    let conf: serde_json::Value = serde_json::from_str(&read("src-tauri/tauri.mas.conf.json")).unwrap();
    let mac = &conf["bundle"]["macOS"];
    assert_eq!(mac["signingIdentity"], store::APPLE_DISTRIBUTION);
    assert_eq!(mac["entitlements"], "entitlements.mas.plist");
    assert_eq!(conf["bundle"]["targets"], serde_json::json!(["app"]));
}

#[test]
fn the_short_description_names_both_kinds_of_host() {
    let conf: serde_json::Value = serde_json::from_str(&read("src-tauri/tauri.conf.json")).unwrap();
    let d = conf["bundle"]["shortDescription"].as_str().unwrap();
    assert!(d.contains("GNOME") && d.contains("Mac"), "{d}");
}

#[test]
fn the_mas_package_is_universal_and_signed_by_the_installer_identity() {
    let p = store::mas_plan(Path::new("/repo"), None);
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
    let p = store::mas_plan(Path::new("/repo"), Some(Path::new("/profiles/drift.provisionprofile")));
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
