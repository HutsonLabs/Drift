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

#[test]
fn a_debug_bundle_uses_the_debug_build() {
    let p = plan(Path::new("/repo"), "debug");
    assert_eq!(p.app, Path::new("/repo/target/debug/bundle/Drift Host.app"));
    assert!(!p.build_args.contains(&"--release".to_owned()));
}
