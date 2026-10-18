//! App Store and Developer ID packaging (tasks **STORE-1..4**), as data the tests check.
//!
//! * `cargo xtask mas-package` ([`mas_plan`]): the Mac App Store build. A universal
//!   (`aarch64` + `x86_64`) `Drift.app` signed with Apple Distribution and the sandboxed
//!   `entitlements.mas.plist`, with its provisioning profile embedded, wrapped by `productbuild`
//!   in a `.pkg` signed with the Mac installer identity.
//!
//! Identities are pinned by SHA-1, like the Developer ID identity in `tauri.conf.json`: the
//! names are shared by several certificates in the signing keychain.

use std::path::{Path, PathBuf};

/// The Apple Developer team.
pub const TEAM_ID: &str = "M9Z3N2R97A";
/// The Apple Distribution signing identity (SHA-1).
pub const APPLE_DISTRIBUTION: &str = "732EB0D94375A915CFEDA49F2650594538D75E04";
/// The "3rd Party Mac Developer Installer" identity that signs the `.pkg` (SHA-1).
pub const MAC_INSTALLER: &str = "CB613372302DED68460709B0A9C4E562FBF2A705";
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

/// The Mac App Store plan for the repository at `root`, embedding `profile` if given.
///
/// The profile is copied in *before* the app is re-signed (see `main.rs`), because the
/// signature seals `Contents/`.
pub fn mas_plan(root: &Path, profile: Option<&Path>) -> MasPlan {
    let bundle = root.join("target").join(UNIVERSAL).join("release").join("bundle");
    let app = bundle.join("macos").join("Drift.app");
    let pkg = bundle.join("mas").join("Drift.pkg");
    let tauri_args =
        ["tauri", "build", "--target", UNIVERSAL, "--bundles", "app", "--config", "tauri.mas.conf.json"]
            .map(str::to_owned)
            .to_vec();
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

/// The Developer ID Application identity Drift Host is signed with (SHA-1; the one pinned in
/// `tauri.conf.json` for Drift).
pub const DEVELOPER_ID: &str = "FE9B9ADD91CB67176BFE80FF725F77539D75BD96";

/// What `cargo xtask host-release` runs (STORE-4): sign `Drift Host.app` with Developer ID and
/// the Hardened Runtime from the inside out, wrap it in a DMG, sign that, notarize it and staple
/// the ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostReleasePlan {
    /// The bundle (built by `cargo xtask host-bundle`).
    pub app: PathBuf,
    /// `codesign` argument lists for the bundle's code, nested executables first.
    pub sign: Vec<Vec<String>>,
    /// The folder the DMG is made from: the app and an `Applications` link.
    pub staging: PathBuf,
    /// The disk image.
    pub dmg: PathBuf,
    /// `hdiutil` arguments.
    pub hdiutil: Vec<String>,
    /// `codesign` arguments for the disk image.
    pub sign_dmg: Vec<String>,
    /// `xcrun` arguments for notarization; the App Store Connect key is appended at run time.
    pub notarize_args: Vec<String>,
    /// `xcrun` arguments for stapling the ticket to the DMG.
    pub staple_args: Vec<String>,
}

/// The release plan for version `version` in the repository at `root`.
pub fn host_release_plan(root: &Path, version: &str) -> HostReleasePlan {
    let bundle = root.join("target/release/bundle");
    let app = bundle.join(crate::host_bundle::APP_NAME);
    let entitlements = root.join("crates/drift-host-app/entitlements.plist");
    let sign_one = |path: &Path| {
        vec![
            "--force".to_owned(),
            "--options".to_owned(),
            "runtime".to_owned(),
            "--timestamp".to_owned(),
            "--sign".to_owned(),
            DEVELOPER_ID.to_owned(),
            "--entitlements".to_owned(),
            entitlements.display().to_string(),
            path.display().to_string(),
        ]
    };
    let sign = vec![sign_one(&app.join("Contents/MacOS/drift-host")), sign_one(&app)];
    let staging = bundle.join("dmg-staging");
    let dmg = bundle.join("dmg").join(format!("Drift Host_{version}_aarch64.dmg"));
    let hdiutil = [
        "create",
        "-volname",
        "Drift Host",
        "-srcfolder",
        &staging.display().to_string(),
        "-ov",
        "-format",
        "UDZO",
        &dmg.display().to_string(),
    ]
    .map(str::to_owned)
    .to_vec();
    let sign_dmg = ["--force", "--timestamp", "--sign", DEVELOPER_ID, &dmg.display().to_string()]
        .map(str::to_owned)
        .to_vec();
    let notarize_args =
        ["notarytool", "submit", &dmg.display().to_string(), "--wait"].map(str::to_owned).to_vec();
    let staple_args = ["stapler", "staple", &dmg.display().to_string()].map(str::to_owned).to_vec();
    HostReleasePlan { app, sign, staging, dmg, hdiutil, sign_dmg, notarize_args, staple_args }
}
