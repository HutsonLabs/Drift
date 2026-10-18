//! M1-6 / M6-2 / M9-3 Red: launch options and bundle configuration.
#![allow(clippy::unwrap_used, clippy::expect_used)] // test fixtures

use std::path::PathBuf;

use drift_app::RunOptions;

fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

#[test]
fn autoconnect_and_config_dir_come_from_the_environment() {
    let o = RunOptions::from_vars(vars(&[
        ("DRIFT_AUTOCONNECT", "Homelab headless"),
        ("DRIFT_CONFIG_DIR", "/tmp/drift-smoke"),
        ("HOME", "/Users/x"),
    ]));
    assert_eq!(o.autoconnect.as_deref(), Some("Homelab headless"));
    assert_eq!(o.config_dir, Some(PathBuf::from("/tmp/drift-smoke")));
    assert!(!o.memory_secrets, "the environment can never switch off the Keychain");
    assert!(o.on_ready.is_none());
}

#[test]
fn blank_or_missing_variables_are_ignored() {
    let o = RunOptions::from_vars(vars(&[("DRIFT_AUTOCONNECT", "  "), ("DRIFT_CONFIG_DIR", "")]));
    assert!(o.autoconnect.is_none() && o.config_dir.is_none());
    let o = RunOptions::from_vars(vars(&[("DRIFT_MEMORY_SECRETS", "1")]));
    assert!(!o.memory_secrets);
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap()
}

#[test]
fn info_plist_declares_local_network_usage() {
    // Plan §1.8 / M9-3: without it macOS denies LAN connections (errno 65) with no prompt.
    let plist = read("Info.plist");
    let key = plist.find("<key>NSLocalNetworkUsageDescription</key>").expect("key present");
    let rest = &plist[key..];
    let value = rest.split("<string>").nth(1).and_then(|s| s.split("</string>").next()).unwrap();
    assert!(value.len() > 20, "a real explanation: {value}");
}

/// UI-windows decisions 1 and 5: two templates, both created by the app — Connections
/// (1100×720, `index.html`) and the session template (1280×800, `session.html`) with the
/// traffic lights centred in the 52-point title bar.
#[test]
fn windows_are_built_by_the_app_from_two_templates() {
    let conf: serde_json::Value = serde_json::from_str(&read("tauri.conf.json")).unwrap();
    let windows = conf["app"]["windows"].as_array().cloned().unwrap_or_default();
    let labels: Vec<_> = windows.iter().filter_map(|w| w["label"].as_str()).collect();
    assert_eq!(labels, [drift_app::windows::CONNECTIONS_TEMPLATE, drift_app::windows::SESSION_TEMPLATE]);
    assert_eq!(drift_app::windows::CONNECTIONS_TEMPLATE, drift_app::connections::CONNECTIONS_WINDOW);
    for w in &windows {
        assert_eq!(w["create"], false, "{}: the app creates its windows itself", w["label"]);
        assert_eq!(w["titleBarStyle"], "Overlay", "{}", w["label"]);
        assert_eq!(w["hiddenTitle"], true, "{}", w["label"]);
    }
    let (connections, session) = (&windows[0], &windows[1]);
    assert_eq!((connections["width"].as_f64(), connections["height"].as_f64()), (Some(1100.0), Some(720.0)));
    assert_eq!(connections["url"], "index.html");
    assert_eq!((session["width"].as_f64(), session["height"].as_f64()), (Some(1280.0), Some(800.0)));
    assert_eq!(session["url"], "session.html");
    let lights = &session["trafficLightPosition"];
    // The standard inset; `y` is whatever centres the lights in the 52-pt bar, which the real
    // window test `windows_ui` measures.
    assert_eq!(lights["x"].as_f64(), Some(20.0));
    assert!(lights["y"].as_f64().is_some_and(|y| y > 20.0 && y < 40.0), "{lights}");
    assert_eq!(conf["bundle"]["macOS"]["minimumSystemVersion"], "27.0");
    let caps: serde_json::Value = serde_json::from_str(&read("capabilities/default.json")).unwrap();
    let labels: Vec<_> = caps["windows"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    assert!(labels.contains(&"session-*") && labels.contains(&"connections"), "{labels:?}");
    assert_eq!(drift_app::windows::window_label(7), "session-7");
    assert_eq!(drift_app::windows::titlebar_label("session-7"), "session-7-titlebar", "covered by session-*");
}

#[test]
fn the_webview_is_transparent_so_overlays_can_be_drawn_over_the_picture() {
    // M7-3 keeps "the last frame dimmed under an overlay" and M1's stats HUD floats over the
    // live picture. Both need a non-opaque WKWebView, which wry only does under Tauri's
    // `macos-private-api` feature plus the matching config flag
    // (docs/adr/M7-3-overlays-over-the-live-picture.md).
    let conf: serde_json::Value = serde_json::from_str(&read("tauri.conf.json")).unwrap();
    assert_eq!(conf["app"]["macOSPrivateApi"], true, "the CSS transparency rules are dead without it");
    let manifest = read("Cargo.toml");
    let tauri_dep =
        manifest.lines().find(|l| l.trim_start().starts_with("tauri = ")).expect("a `tauri` dependency line");
    assert!(
        tauri_dep.contains("macos-private-api"),
        "the config flag alone does nothing; wry needs the cargo feature: {tauri_dep}"
    );
}
