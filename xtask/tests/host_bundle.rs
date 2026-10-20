//! HOST-9: `cargo xtask host-bundle` lays out `Drift Host.app` from the release build.
#![allow(missing_docs)]

use std::path::Path;

use xtask::host_bundle::{APP_NAME, plan};

#[test]
fn the_bundle_holds_the_app_and_the_command_line_host() {
    let p = plan(Path::new("/repo"), "release");
    assert_eq!(APP_NAME, "Drift Host.app");
    assert_eq!(p.app, Path::new("/repo/target/release/bundle/Drift Host.app"));
    let pairs: Vec<(String, String)> =
        p.copies.iter().map(|(from, to)| (from.display().to_string(), to.display().to_string())).collect();
    assert_eq!(
        pairs,
        [
            (
                "/repo/crates/drift-host-app/Info.plist".to_owned(),
                "/repo/target/release/bundle/Drift Host.app/Contents/Info.plist".to_owned()
            ),
            (
                "/repo/target/release/drift-host-app".to_owned(),
                "/repo/target/release/bundle/Drift Host.app/Contents/MacOS/drift-host-app".to_owned()
            ),
            (
                "/repo/target/release/drift-host".to_owned(),
                "/repo/target/release/bundle/Drift Host.app/Contents/MacOS/drift-host".to_owned()
            ),
            // HOST-13: launchd services registered through SMAppService.
            (
                "/repo/crates/drift-host-app/launchd/com.hutsonlabs.drift.host.daemon.plist".to_owned(),
                "/repo/target/release/bundle/Drift Host.app/Contents/Library/LaunchDaemons/com.hutsonlabs.drift.host.daemon.plist".to_owned()
            ),
            (
                "/repo/crates/drift-host-app/launchd/com.hutsonlabs.drift.host.agent.plist".to_owned(),
                "/repo/target/release/bundle/Drift Host.app/Contents/Library/LaunchAgents/com.hutsonlabs.drift.host.agent.plist".to_owned()
            ),
            (
                "/repo/crates/drift-host-app/launchd/com.hutsonlabs.drift.host.loginwindow.plist".to_owned(),
                "/repo/target/release/bundle/Drift Host.app/Contents/Library/LaunchAgents/com.hutsonlabs.drift.host.loginwindow.plist".to_owned()
            ),
        ]
    );
    assert_eq!(p.build_args, ["build", "--release", "-p", "drift-host-app", "-p", "drift-host", "--locked"]);
}

/// HOST-icon: the Icon Composer file next to the host's Info.plist is compiled into the bundle's
/// `Assets.car` (and a flat `AppIcon.icns`), and the Info.plist names it.
#[test]
fn the_bundle_compiles_the_host_app_icon() {
    let p = plan(Path::new("/repo"), "release");
    let args = p.actool_args.join(" ");
    assert!(args.starts_with("actool /repo/crates/drift-host-app/AppIcon.icon "), "{args}");
    assert!(args.contains("--compile /repo/target/release/bundle/Drift Host.app/Contents/Resources"), "{args}");
    assert!(args.contains("--app-icon AppIcon"), "{args}");
    assert!(args.contains("--platform macosx"), "{args}");
    assert!(args.contains("--minimum-deployment-target 27.0"), "{args}");
    assert!(
        !args.contains("--output-partial-info-plist /repo/target/release/bundle/Drift Host.app/"),
        "the partial Info.plist stays outside the bundle: {args}"
    );

    let root = xtask::repo::root();
    let plist = std::fs::read_to_string(root.join("crates/drift-host-app/Info.plist")).unwrap();
    for key in ["CFBundleIconName", "CFBundleIconFile"] {
        assert!(plist.contains(&format!("<key>{key}</key>\n\t<string>AppIcon</string>")), "{key} is not AppIcon");
    }
    let problems = xtask::icons::icon_composer_audit(&root.join(xtask::icons::HOST_SOURCE));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_debug_bundle_uses_the_debug_build() {
    let p = plan(Path::new("/repo"), "debug");
    assert_eq!(p.app, Path::new("/repo/target/debug/bundle/Drift Host.app"));
    assert!(!p.build_args.contains(&"--release".to_owned()));
}
