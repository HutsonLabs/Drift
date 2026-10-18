//! M9-3 Red: the shipped bundle is sandboxed and hardened, and stays usable.
//!
//! Plan M9-3 asks for "App Sandbox (`network.client`), Hardened Runtime, and
//! `NSLocalNetworkUsageDescription`". Both switches are silent when they are wrong: an app
//! without `com.apple.security.network.client` fails every connection with a bare
//! `Operation not permitted`, and an app that turns the sandbox on without the entitlements
//! its Keychain / config writes need fails only on a signed build, long after `cargo test`.
//! These assertions pin the bundle configuration that the M9-5 bundle step signs with, and
//! the sandbox-compatibility facts that the manual checklist verifies on a real signed build.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures

use std::collections::BTreeMap;
use std::path::Path;

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let p = manifest_dir().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()))
}

fn config() -> serde_json::Value {
    serde_json::from_str(&read("tauri.conf.json")).expect("tauri.conf.json parses")
}

/// The `<key>` → value pairs of a flat XML plist, with `<true/>`/`<false/>` as booleans and
/// everything else as its raw text (arrays become their joined contents).
fn plist_entries(xml: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut rest = xml;
    while let Some(at) = rest.find("<key>") {
        let after = &rest[at + "<key>".len()..];
        let Some(end) = after.find("</key>") else { break };
        let key = after[..end].trim().to_owned();
        let mut value = after[end + "</key>".len()..].trim_start();
        let parsed = if value.starts_with("<true/>") {
            "true".to_owned()
        } else if value.starts_with("<false/>") {
            "false".to_owned()
        } else if value.starts_with("<array>") {
            let end = value.find("</array>").unwrap_or(value.len());
            let body = &value[..end];
            body.replace("<array>", "").replace("<string>", " ").replace("</string>", " ")
        } else if let Some(start) = value.strip_prefix("<string>") {
            value = start;
            value[..value.find("</string>").unwrap_or(value.len())].to_owned()
        } else {
            String::new()
        };
        out.insert(key, parsed.trim().to_owned());
        rest = after;
    }
    out
}

#[test]
fn the_bundle_is_signed_with_the_entitlements_file_and_hardened_runtime() {
    let conf = config();
    let macos = &conf["bundle"]["macOS"];
    assert_eq!(
        macos["entitlements"].as_str(),
        Some("entitlements.plist"),
        "plan M9-3: the bundle must be signed with Drift's entitlements"
    );
    // `hardenedRuntime` defaults to true in tauri 2, but M9-3 makes it a decision, not a
    // default that a future config edit can flip unnoticed.
    assert_eq!(macos["hardenedRuntime"], true, "plan M9-3: Hardened Runtime stays on");
}

#[test]
fn the_bundle_is_signed_with_the_pinned_developer_id_identity() {
    let conf = config();
    // Pinned by SHA-1, not by name: two "Developer ID Application: Andrew Hutson (M9Z3N2R97A)"
    // certificates exist, so the name is ambiguous and codesign refuses it. This one lives in
    // the dedicated term-hut-signing keychain, which unlocks without a GUI prompt (SSH,
    // launchd), so `cargo tauri build` produces a Developer ID–signed .dmg from any session.
    // A stable identity also keeps the Local Network permission across rebuilds (plan §1.8).
    assert_eq!(
        conf["bundle"]["macOS"]["signingIdentity"].as_str(),
        Some("FE9B9ADD91CB67176BFE80FF725F77539D75BD96"),
        "the .dmg must be signed with the Developer ID identity, not ad hoc"
    );
}

#[test]
fn the_entitlements_enable_the_app_sandbox_and_outgoing_network_only() {
    let entries = plist_entries(&read("entitlements.plist"));
    assert_eq!(
        entries.get("com.apple.security.app-sandbox").map(String::as_str),
        Some("true"),
        "plan M9-3: App Sandbox, got {entries:#?}"
    );
    assert_eq!(
        entries.get("com.apple.security.network.client").map(String::as_str),
        Some("true"),
        "plan M9-3: Drift dials RDP hosts, got {entries:#?}"
    );
    // Drift never listens, and never needs a temporary exception: anything else here would
    // widen the sandbox past what the plan allows (§9 rules out file, camera and audio work).
    assert!(
        !entries.contains_key("com.apple.security.network.server"),
        "Drift is a client only: {entries:#?}"
    );
    for key in entries.keys() {
        assert!(
            !key.contains("temporary-exception"),
            "a sandbox temporary exception needs an ADR and a review: {key}"
        );
        assert!(
            !key.contains("cs.allow-unsigned-executable-memory")
                && !key.contains("cs.disable-library-validation"),
            "that entitlement would defeat the Hardened Runtime: {key}"
        );
    }
}

#[test]
fn the_sandboxed_app_can_still_reach_its_keychain_items_and_its_config() {
    // Keychain: sandboxed items live in the app's own access group, `<team>.<bundle id>`, so
    // the service name Drift stores under must be the bundle identifier of the shipped app.
    // A mismatch stores the passwords where the signed app cannot read them back.
    let identifier = config()["identifier"].as_str().expect("bundle identifier").to_owned();
    assert_eq!(
        drift_macos::keychain::Keychain::SERVICE,
        identifier,
        "the Keychain service must be the bundle identifier so the sandbox container matches"
    );

    // Config: the app asks tauri for `app_config_dir()`, which the sandbox redirects into the
    // container. Nothing may hard-code a path outside it (`DRIFT_CONFIG_DIR` is dev-only).
    let lib = read("src/lib.rs");
    assert!(lib.contains("app_config_dir()"), "the config directory comes from tauri's path API");
    let options = drift_app::RunOptions::from_vars(Vec::new());
    assert!(options.config_dir.is_none(), "no config path is baked in");

    // Metal, VideoToolbox and NSPasteboard need no entitlement, but they only keep working
    // because nothing asks for a second process or a helper tool; the manual checklist
    // confirms them on a signed, sandboxed build.
    let acceptance =
        std::fs::read_to_string(manifest_dir().join("../docs/acceptance.md")).expect("docs/acceptance.md");
    let section = acceptance
        .split("## ")
        .find(|s| s.starts_with("M9-3"))
        .unwrap_or_else(|| panic!("docs/acceptance.md has no M9-3 section"));
    for capability in ["Keychain", "Metal", "VideoToolbox", "pasteboard", "config"] {
        assert!(
            section.contains(capability),
            "the sandboxed build's {capability} use must be checked by hand: {section}"
        );
    }
}

#[test]
fn the_local_network_prompt_and_its_settings_link_are_configured() {
    // Without the usage description macOS denies LAN connections with errno 65 and no prompt
    // (plan §1.8); the error screen must link to the pane that grants it.
    let plist = plist_entries(&read("Info.plist"));
    let description = plist.get("NSLocalNetworkUsageDescription").expect("usage description");
    assert!(description.len() > 20, "a real explanation: {description}");
    assert!(
        drift_core::messages::LOCAL_NETWORK_SETTINGS_URL.starts_with("x-apple.systempreferences:"),
        "the errno 65 screen opens System Settings"
    );
    let explanation = drift_core::messages::explain_disconnect(
        &drift_core::DisconnectReason::LocalNetworkDenied,
        drift_core::ConnectMode::Headless,
    );
    assert!(
        explanation.actions.contains(&drift_core::messages::ErrorAction::OpenLocalNetworkSettings),
        "{explanation:?}"
    );
}
