//! `cargo xtask host-bundle` (task **HOST-9**): lays out `Drift Host.app` from a cargo build.
//!
//! The bundle holds the menu-bar app (its executable) and the `drift-host` command next to it.
//! It is signed ad hoc so it runs locally; Developer ID signing, notarization and the DMG are
//! STORE-4.

use std::path::{Path, PathBuf};

/// The bundle's directory name.
pub const APP_NAME: &str = "Drift Host.app";

/// What to build and where each file goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundlePlan {
    /// `target/<profile>/bundle/Drift Host.app`.
    pub app: PathBuf,
    /// `cargo` arguments that build both executables.
    pub build_args: Vec<String>,
    /// `(from, to)` copies, in order.
    pub copies: Vec<(PathBuf, PathBuf)>,
}

/// The plan for `profile` (`release` or `debug`) in the repository at `root`.
pub fn plan(root: &Path, profile: &str) -> BundlePlan {
    let target = root.join("target").join(profile);
    let app = target.join("bundle").join(APP_NAME);
    let contents = app.join("Contents");
    let mut build_args = vec!["build".to_owned()];
    if profile == "release" {
        build_args.push("--release".to_owned());
    }
    build_args.extend(["-p", "drift-host-app", "-p", "drift-host", "--locked"].map(str::to_owned));
    let mut copies = vec![
        (root.join("crates/drift-host-app/Info.plist"), contents.join("Info.plist")),
        (target.join("drift-host-app"), contents.join("MacOS/drift-host-app")),
        (target.join("drift-host"), contents.join("MacOS/drift-host")),
    ];
    // HOST-13: the launchd services the app registers (or an administrator installs).
    for (name, folder) in [
        ("com.hutsonlabs.drift.host.daemon.plist", "LaunchDaemons"),
        ("com.hutsonlabs.drift.host.agent.plist", "LaunchAgents"),
        ("com.hutsonlabs.drift.host.loginwindow.plist", "LaunchAgents"),
    ] {
        copies.push((
            root.join("crates/drift-host-app/launchd").join(name),
            contents.join("Library").join(folder).join(name),
        ));
    }
    BundlePlan { app, build_args, copies }
}
