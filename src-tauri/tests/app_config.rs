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

// ---- IOS-5: the iOS app project -------------------------------------------------------------

/// The iOS `Info.plist` and the xcodegen spec that regenerates it carry the same keys: Local
/// Network usage (else iOS denies LAN connections), multiple scenes (one per session window on
/// iPad, IOS-1/IOS-11) and ProMotion above 60 Hz on iPhone (IOS-4).
#[test]
fn ios_info_plist_and_project_spec_declare_the_ios_keys() {
    let plist = read("gen/apple/drift-app_iOS/Info.plist");
    let spec = read("gen/apple/project.yml");
    for key in [
        "NSLocalNetworkUsageDescription",
        "UIApplicationSupportsMultipleScenes",
        "CADisableMinimumFrameDurationOnPhone",
        // IOS-6: iPad pointer clicks arrive as indirect-pointer touches.
        "UIApplicationSupportsIndirectInputEvents",
    ] {
        assert!(plist.contains(&format!("<key>{key}</key>")), "Info.plist lacks {key}");
        assert!(spec.contains(&format!("{key}:")), "project.yml lacks {key}; xcodegen would drop it");
    }
}

/// Xcode 27 rejects deployment targets below iOS 15 (IOS-1); Drift asks for 17 (scene
/// activation requests).
#[test]
fn ios_minimum_system_version_is_17() {
    let conf: serde_json::Value = serde_json::from_str(&read("tauri.conf.json")).unwrap();
    assert_eq!(conf["bundle"]["iOS"]["minimumSystemVersion"], "17.0");
    assert!(read("gen/apple/project.yml").contains("iOS: 17.0"));
}

/// A static library carries no link directives, so every framework Drift's crates need is listed
/// in the Xcode project (the first iOS link failed on VideoToolbox, CoreMedia and libz).
#[test]
fn ios_project_links_the_frameworks_the_crates_need() {
    let spec = read("gen/apple/project.yml");
    for framework in [
        "VideoToolbox.framework",
        "CoreMedia.framework",
        "CoreVideo.framework",
        "IOSurface.framework",
        "Network.framework",
        "GameController.framework",
        "Metal.framework",
        "libz.tbd",
        "AVFoundation.framework",
    ] {
        assert!(spec.contains(framework), "project.yml does not link {framework}");
    }
}

/// IOS-9: Drift browses `_drift-host._tcp` (iOS refuses to browse a type the app did not
/// declare) and scans pairing QR codes with the camera.
#[test]
fn ios_declares_bonjour_and_camera_use() {
    let plist = read("gen/apple/drift-app_iOS/Info.plist");
    let spec = read("gen/apple/project.yml");
    assert!(
        plist.contains("<key>NSBonjourServices</key>") && plist.contains("<string>_drift-host._tcp</string>")
    );
    assert!(spec.contains("NSBonjourServices:") && spec.contains("_drift-host._tcp"));
    assert!(
        plist.contains("<key>NSCameraUsageDescription</key>"),
        "Info.plist lacks NSCameraUsageDescription"
    );
    assert!(spec.contains("NSCameraUsageDescription:"), "project.yml lacks NSCameraUsageDescription");
    // macOS browses too (typed-code pairing): the Mac's Info.plist declares the same type.
    assert!(read("Info.plist").contains("_drift-host._tcp"));
}

/// IOS-9: the Mac's QR code is a `drift-pair:` link, so the system Camera (or a click on a Mac)
/// opens Drift straight into "Pair a Mac". Both apps register the scheme.
#[test]
fn both_apps_register_the_drift_pair_url_scheme() {
    for file in ["Info.plist", "gen/apple/drift-app_iOS/Info.plist"] {
        let plist = read(file);
        assert!(
            plist.contains("<key>CFBundleURLSchemes</key>") && plist.contains("<string>drift-pair</string>"),
            "{file}"
        );
    }
    assert!(read("gen/apple/project.yml").contains("CFBundleURLSchemes: [drift-pair]"));
}

/// The generated Xcode project links every SDK `project.yml` lists. xcodegen can fail without
/// rewriting it (a missing `assets` folder), which once left GameController out of the link
/// after a merge (IOS-6 + IOS-9).
#[test]
fn xcode_project_is_regenerated_from_project_spec() {
    let spec = read("gen/apple/project.yml");
    let pbxproj = read("gen/apple/drift-app.xcodeproj/project.pbxproj");
    for line in spec.lines() {
        let Some(sdk) = line.trim().strip_prefix("- sdk: ") else { continue };
        assert!(pbxproj.contains(sdk), "project.pbxproj lacks {sdk}; run `xcodegen generate` in gen/apple");
    }
}
