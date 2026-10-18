//! App Store and Developer ID packaging (STORE-1..4). Not yet implemented.

/// The Apple Developer team.
pub const TEAM_ID: &str = "";
/// The Apple Distribution signing identity (SHA-1).
pub const APPLE_DISTRIBUTION: &str = "";
/// The Mac installer signing identity (SHA-1).
pub const MAC_INSTALLER: &str = "";

/// What `cargo xtask mas-package` runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasPlan {
    /// `cargo tauri build` arguments.
    pub tauri_args: Vec<String>,
    /// The built app.
    pub app: std::path::PathBuf,
    /// The installer package.
    pub pkg: std::path::PathBuf,
    /// `productbuild` arguments.
    pub productbuild_args: Vec<String>,
    /// `(profile, destination)` when a provisioning profile is embedded.
    pub embed_profile: Option<(std::path::PathBuf, std::path::PathBuf)>,
}

/// Not yet implemented.
pub fn mas_plan(_root: &std::path::Path, _profile: Option<&std::path::Path>) -> MasPlan {
    MasPlan {
        tauri_args: Vec::new(),
        app: std::path::PathBuf::new(),
        pkg: std::path::PathBuf::new(),
        productbuild_args: Vec::new(),
        embed_profile: None,
    }
}
