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
