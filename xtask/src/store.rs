//! App Store and Developer ID packaging (tasks **STORE-1..4**), as data the tests check.
//!
//! * `cargo xtask mas-package` ([`mas_plan`]): the Mac App Store build. A universal
//!   (`aarch64` + `x86_64`) `Drift.app` signed with Apple Distribution and the sandboxed
//!   `entitlements.mas.plist`, with its provisioning profile embedded, wrapped by `productbuild`
//!   in a `.pkg` signed with the Mac installer identity.
//!
//! Identities are pinned by SHA-1, like the Developer ID identity in `tauri.conf.json`: a
//! name can be shared by several certificates in a signing keychain.
//!
//! The team ID, identity hashes and bundle ID (`com.example.drift`) in this repository are
//! placeholders. To sign and ship your own builds, replace them with your Apple Developer
//! team's values here, in `src-tauri/tauri.conf.json`, `src-tauri/tauri.mas.conf.json`, the
//! entitlements and `src-tauri/gen/apple`.

use std::path::{Path, PathBuf};

/// The Apple Developer team.
pub const TEAM_ID: &str = "TEAMID0000";
/// The Apple Distribution signing identity (SHA-1).
pub const APPLE_DISTRIBUTION: &str = "0000000000000000000000000000000000000002";
/// The "3rd Party Mac Developer Installer" identity that signs the `.pkg` (SHA-1).
pub const MAC_INSTALLER: &str = "0000000000000000000000000000000000000003";
/// The universal macOS target.
pub const UNIVERSAL: &str = "universal-apple-darwin";

/// What `cargo xtask mas-package` runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasPlan {
    /// `cargo tauri build` arguments (run in `src-tauri/`).
    pub tauri_args: Vec<String>,
    /// The built app.
    pub app: PathBuf,
    /// The installer package.
    pub pkg: PathBuf,
    /// `productbuild` arguments.
    pub productbuild_args: Vec<String>,
    /// `(profile, destination)` when a provisioning profile is embedded.
    pub embed_profile: Option<(PathBuf, PathBuf)>,
}

/// The Mac App Store plan for the repository at `root`, embedding `profile` if given, with
/// build number `build` (`CFBundleVersion`) when given.
///
/// The profile is copied in *before* the app is re-signed (see `main.rs`), because the
/// signature seals `Contents/`.
pub fn mas_plan(root: &Path, profile: Option<&Path>, build: Option<u32>) -> MasPlan {
    let bundle = root.join("target").join(UNIVERSAL).join("release").join("bundle");
    let app = bundle.join("macos").join("Drift.app");
    let pkg = bundle.join("mas").join("Drift.pkg");
    let mut tauri_args =
        ["tauri", "build", "--target", UNIVERSAL, "--bundles", "app", "--config", "tauri.mas.conf.json"]
            .map(str::to_owned)
            .to_vec();
    if let Some(build) = build {
        tauri_args.push("--config".to_owned());
        tauri_args.push(format!(r#"{{"bundle":{{"macOS":{{"bundleVersion":"{build}"}}}}}}"#));
    }
    let productbuild_args = vec![
        "--component".to_owned(),
        app.display().to_string(),
        "/Applications".to_owned(),
        "--sign".to_owned(),
        MAC_INSTALLER.to_owned(),
        pkg.display().to_string(),
    ];
    let embed_profile =
        profile.map(|p| (p.to_owned(), app.join("Contents").join("embedded.provisionprofile")));
    MasPlan { tauri_args, app, pkg, productbuild_args, embed_profile }
}

/// A string in a built binary that App Review's static analysis may read as private API use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRisk {
    /// The string found.
    pub needle: &'static str,
    /// Where it comes from and why it matters.
    pub why: &'static str,
}

/// Private keys and selectors Drift's dependencies are known to use, and why.
///
/// Only strings that name private API belong here.
pub const PRIVATE_API_STRINGS: &[ReviewRisk] = &[
    ReviewRisk {
        needle: "drawsBackground",
        why: "wry sets WKWebView's private `drawsBackground` key (KVC) for transparent webviews; \
              Drift's overlays over the live picture need it (ADR M7-3, IOS-0 open question 7)",
    },
    ReviewRisk {
        needle: "_setDrawsTransparentBackground:",
        why: "private WKWebView selector for transparent backgrounds",
    },
];

/// The [`PRIVATE_API_STRINGS`] found in `binary`, in table order. Matching is case-sensitive,
/// so public look-alikes (`setDrawsBackground:` on `NSScrollView`) do not match.
pub fn review_risks(binary: &[u8]) -> Vec<ReviewRisk> {
    PRIVATE_API_STRINGS
        .iter()
        .filter(|r| memchr::memmem::find(binary, r.needle.as_bytes()).is_some())
        .cloned()
        .collect()
}

/// The Developer ID Application identity Drift's DMG is signed with (SHA-1; the one pinned in
/// `tauri.conf.json`).
pub const DEVELOPER_ID: &str = "0000000000000000000000000000000000000001";
