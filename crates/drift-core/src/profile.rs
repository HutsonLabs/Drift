//! Connection profiles and per-profile preferences (plan §3).
//!
//! Profiles are persisted as TOML by the app. **Secrets are never part of a profile**:
//! passwords live in the Keychain keyed by `profile_id` + role (plan §2 decision 7).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// Default RDP port used by every mode unless the profile overrides it.
pub const DEFAULT_RDP_PORT: u16 = 3389;

/// How Drift reaches the GNOME session (plan §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum ConnectMode {
    /// System daemon: NLA leg, then RDSTLS legs through the GDM greeter.
    RemoteLogin,
    /// Persistent per-user headless session with fixed RDP credentials.
    Headless,
    /// The user's Desktop Sharing daemon mirroring an existing session (no DISP channel).
    DesktopSharing,
}

impl ConnectMode {
    /// Every mode, in UI order: Headless (the default for new profiles), Desktop Sharing,
    /// Remote Login (ADR UI-windows-gallery decision 14).
    pub const ALL: [Self; 3] = [Self::Headless, Self::DesktopSharing, Self::RemoteLogin];

    /// Default TCP port for the mode.
    pub const fn default_port(self) -> u16 {
        DEFAULT_RDP_PORT
    }

    /// Whether the server offers the Display Control channel (remote resize) in this mode.
    pub const fn supports_display_control(self) -> bool {
        !matches!(self, Self::DesktopSharing)
    }
}

/// What the Mac Command key is sent as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum CmdAs {
    /// Left/right Windows ("Super") key, scancode `0x5B`/`0x5C` extended. Default.
    #[default]
    Super,
    /// Control, scancode `0x1D`.
    Ctrl,
}

/// Keyboard preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(default)]
pub struct KeyboardPrefs {
    /// Mapping of the Command key.
    pub cmd_as: CmdAs,
    /// "Type using Mac layout": printable keys without Ctrl/Cmd go out as Unicode events.
    pub type_with_mac_layout: bool,
}

/// Display preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(default)]
pub struct DisplayPrefs {
    /// Resize the remote desktop to follow the window (Display Control).
    pub adaptive: bool,
    /// Use physical pixels with `DesktopScaleFactor=200` on Retina displays.
    pub retina: bool,
}

impl Default for DisplayPrefs {
    fn default() -> Self {
        Self { adaptive: true, retina: true }
    }
}

/// How a finger on the picture drives the remote pointer on iPhone and iPad (IOS-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum TouchMode {
    /// The device decides: trackpad on iPhone, direct on iPad. Default.
    #[default]
    Auto,
    /// The screen is a trackpad: relative pointer, taps click where the pointer is.
    Trackpad,
    /// Tap where you touch.
    Direct,
}

impl TouchMode {
    /// Resolves [`TouchMode::Auto`] for the device (`phone`: an iPhone-sized screen).
    pub const fn resolve(self, phone: bool) -> Self {
        match self {
            Self::Auto if phone => Self::Trackpad,
            Self::Auto => Self::Direct,
            other => other,
        }
    }

    /// `true` for [`TouchMode::Auto`] (not written to `profiles.toml`).
    pub const fn is_auto(&self) -> bool {
        matches!(self, Self::Auto)
    }
}

/// Clipboard synchronisation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum ClipboardPrefs {
    /// No clipboard channel traffic.
    Off,
    /// Text only.
    Text,
    /// Text and images (PNG/TIFF/DIB). Default.
    #[default]
    TextAndImages,
}

/// SHA-256 of a server's leaf certificate DER, used for TOFU pinning.
///
/// `Display` and `FromStr` use the `grdctl status` format: 32 lowercase hex byte pairs
/// separated by `:` (for example `f3:e7:a2:…`), so users can compare the two directly.
/// Serialized (serde) as that same string.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "specta", derive(specta::Type), specta(transparent))]
pub struct CertFingerprint(#[cfg_attr(feature = "specta", specta(type = String))] pub [u8; 32]);

impl CertFingerprint {
    /// Wraps a raw SHA-256 digest.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw digest bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// SHA-256 of a certificate's DER encoding (what `grdctl status` fingerprints).
    pub fn of_der(der: &[u8]) -> Self {
        use sha2::Digest as _;
        Self(sha2::Sha256::digest(der).into())
    }

    /// Extracts the `TLS fingerprint:` line from `grdctl [--system|--headless] status` output.
    pub fn from_grdctl_status(output: &str) -> Option<Self> {
        output.lines().find_map(|l| l.trim().strip_prefix("TLS fingerprint:")).and_then(|v| v.parse().ok())
    }
}

impl fmt::Display for CertFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for CertFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CertFingerprint({self})")
    }
}

impl FromStr for CertFingerprint {
    type Err = ProfileError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut out = [0u8; 32];
        let mut parts = s.trim().split(':');
        for byte in &mut out {
            let part = parts.next().ok_or(ProfileError::InvalidFingerprint)?;
            if part.len() != 2 || !part.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err(ProfileError::InvalidFingerprint);
            }
            *byte = u8::from_str_radix(part, 16).map_err(|_| ProfileError::InvalidFingerprint)?;
        }
        if parts.next().is_some() {
            return Err(ProfileError::InvalidFingerprint);
        }
        Ok(Self(out))
    }
}

impl Serialize for CertFingerprint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CertFingerprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// A saved connection (plan §3). Contains no secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ConnectionProfile {
    /// Stable identifier; also the Keychain account key prefix.
    pub id: Uuid,
    /// Display name (window title).
    pub name: String,
    /// Host name or IP address.
    pub host: String,
    /// TCP port (RemoteLogin default 3389).
    pub port: u16,
    /// Connection mode.
    pub mode: ConnectMode,
    /// RDP user: system credentials (RemoteLogin) or daemon credentials (Headless/Sharing).
    pub rdp_username: String,
    /// RemoteLogin only: the Linux user, shown in the greeter hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux_username: Option<String>,
    /// TOFU-pinned certificate fingerprint of the first leg; for a paired Mac host, the pin
    /// taken from the pairing (never trust-on-first-use).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_pin: Option<CertFingerprint>,
    /// The Drift Host this profile was paired with (IOS-9). `None` for GNOME hosts, which keep
    /// TOFU; absent from `profiles.toml` when `None`, so older files and readers are unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paired_host: Option<Uuid>,
    /// Keyboard preferences.
    #[serde(default)]
    pub keyboard: KeyboardPrefs,
    /// Display preferences.
    #[serde(default)]
    pub display: DisplayPrefs,
    /// Clipboard preferences.
    #[serde(default)]
    pub clipboard: ClipboardPrefs,
    /// Touch on iPhone and iPad (IOS-7); ignored on the Mac.
    #[serde(default, skip_serializing_if = "TouchMode::is_auto")]
    pub touch: TouchMode,
}

impl ConnectionProfile {
    /// Creates a profile with a fresh id, the mode's default port and default preferences.
    pub fn new(name: impl Into<String>, host: impl Into<String>, mode: ConnectMode) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            host: host.into(),
            port: mode.default_port(),
            mode,
            rdp_username: String::new(),
            linux_username: None,
            cert_pin: None,
            paired_host: None,
            keyboard: KeyboardPrefs::default(),
            display: DisplayPrefs::default(),
            clipboard: ClipboardPrefs::default(),
            touch: TouchMode::default(),
        }
    }

    /// Serializes the profile as a TOML document.
    pub fn to_toml(&self) -> Result<String, ProfileError> {
        toml::to_string(self).map_err(|e| ProfileError::Toml(e.to_string()))
    }

    /// Parses a profile from a TOML document.
    pub fn from_toml(s: &str) -> Result<Self, ProfileError> {
        toml::from_str(s).map_err(|e| ProfileError::Toml(e.to_string()))
    }
}

/// Errors from profile (de)serialization and parsing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    /// The string is not a `grdctl`-style SHA-256 fingerprint.
    #[error("invalid certificate fingerprint: expected 32 colon-separated hex bytes")]
    InvalidFingerprint,
    /// TOML (de)serialization failed.
    #[error("profile TOML error: {0}")]
    Toml(String),
}

// ---------------------------------------------------------------------------------------------
// Validation (task M1-6)
// ---------------------------------------------------------------------------------------------

/// Maximum length of a profile name, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// Maximum length of a DNS host name (RFC 1035).
pub const MAX_HOST_LEN: usize = 253;
/// Maximum length of a user name, in characters.
pub const MAX_USERNAME_CHARS: usize = 256;

/// A profile field that can fail validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum ProfileField {
    /// [`ConnectionProfile::name`].
    Name,
    /// [`ConnectionProfile::host`].
    Host,
    /// [`ConnectionProfile::port`].
    Port,
    /// [`ConnectionProfile::rdp_username`].
    RdpUsername,
    /// [`ConnectionProfile::linux_username`].
    LinuxUsername,
}

/// What is wrong with a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum ProfileProblem {
    /// Required but empty (or only whitespace).
    Empty,
    /// Longer than allowed.
    TooLong,
    /// Leading or trailing whitespace.
    SurroundingWhitespace,
    /// Contains control characters or characters not allowed for this field.
    InvalidCharacters,
    /// The host is not a valid IPv4/IPv6 address or DNS name.
    InvalidHost,
    /// The host contains a port (`host:3389`); use the port field.
    PortInHost,
    /// The host contains a URL scheme (`rdp://`).
    SchemeInHost,
    /// Port 0 is not a valid TCP port.
    InvalidPort,
    /// The field does not apply to the profile's mode.
    NotApplicable,
}

/// One validation failure, with a user-facing message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ProfileIssue {
    /// The offending field.
    pub field: ProfileField,
    /// What is wrong.
    pub problem: ProfileProblem,
    /// A sentence for the UI, shown next to the field.
    pub message: String,
}

impl ProfileIssue {
    fn new(field: ProfileField, problem: ProfileProblem) -> Self {
        Self { field, problem, message: issue_message(field, problem) }
    }
}

fn field_label(field: ProfileField) -> &'static str {
    match field {
        ProfileField::Name => "Name",
        ProfileField::Host => "Host",
        ProfileField::Port => "Port",
        ProfileField::RdpUsername => "RDP user name",
        ProfileField::LinuxUsername => "Linux user name",
    }
}

fn issue_message(field: ProfileField, problem: ProfileProblem) -> String {
    let label = field_label(field);
    match problem {
        ProfileProblem::Empty => format!("{label} is required."),
        ProfileProblem::TooLong => {
            let max = match field {
                ProfileField::Name => MAX_NAME_CHARS,
                ProfileField::Host => MAX_HOST_LEN,
                _ => MAX_USERNAME_CHARS,
            };
            format!("{label} must be at most {max} characters.")
        }
        ProfileProblem::SurroundingWhitespace => format!("{label} must not start or end with a space."),
        ProfileProblem::InvalidCharacters => format!("{label} contains characters that are not allowed."),
        ProfileProblem::InvalidHost => {
            "Enter a host name (gnome.local) or an IP address (192.168.1.20 or fe80::1).".into()
        }
        ProfileProblem::PortInHost => "Put the port number in the Port field, not in the host.".into(),
        ProfileProblem::SchemeInHost => "Enter just the host name, without rdp:// or another prefix.".into(),
        ProfileProblem::InvalidPort => "Port must be between 1 and 65535.".into(),
        ProfileProblem::NotApplicable => format!("{label} only applies to Remote Login profiles."),
    }
}

/// Validates a profile before it is saved or used (plan M1-6).
///
/// Returns every problem found (not just the first), in field order, so the form can mark all
/// offending fields at once.
pub fn validate(profile: &ConnectionProfile) -> Result<(), Vec<ProfileIssue>> {
    let checks = [
        (ProfileField::Name, check_text(&profile.name, MAX_NAME_CHARS)),
        (ProfileField::Host, check_host(&profile.host)),
        (ProfileField::Port, (profile.port == 0).then_some(ProfileProblem::InvalidPort)),
        (ProfileField::RdpUsername, check_text(&profile.rdp_username, MAX_USERNAME_CHARS)),
        (ProfileField::LinuxUsername, check_linux_username(profile.mode, profile.linux_username.as_deref())),
    ];
    let issues: Vec<ProfileIssue> =
        checks.into_iter().filter_map(|(field, p)| p.map(|p| ProfileIssue::new(field, p))).collect();
    if issues.is_empty() { Ok(()) } else { Err(issues) }
}

/// Required free text: non-blank, bounded, no control characters, no surrounding whitespace.
fn check_text(value: &str, max_chars: usize) -> Option<ProfileProblem> {
    if value.trim().is_empty() {
        Some(ProfileProblem::Empty)
    } else if value.chars().count() > max_chars {
        Some(ProfileProblem::TooLong)
    } else if value.chars().any(char::is_control) {
        Some(ProfileProblem::InvalidCharacters)
    } else if value.trim() != value {
        Some(ProfileProblem::SurroundingWhitespace)
    } else {
        None
    }
}

fn check_host(host: &str) -> Option<ProfileProblem> {
    use std::net::{Ipv4Addr, Ipv6Addr};

    if host.trim().is_empty() {
        return Some(ProfileProblem::Empty);
    }
    if host.trim() != host {
        return Some(ProfileProblem::SurroundingWhitespace);
    }
    if host.len() > MAX_HOST_LEN {
        return Some(ProfileProblem::TooLong);
    }
    if host.contains("://") {
        return Some(ProfileProblem::SchemeInHost);
    }
    if host.parse::<Ipv4Addr>().is_ok() || host.parse::<Ipv6Addr>().is_ok() {
        return None;
    }
    // `name:port`, `1.2.3.4:port` or `[v6]:port`.
    if let Some((name, port)) = host.rsplit_once(':')
        && !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
        && (is_dns_name(name) || name.parse::<Ipv4Addr>().is_ok() || is_bracketed_v6(name))
    {
        return Some(ProfileProblem::PortInHost);
    }
    if is_dns_name(host) { None } else { Some(ProfileProblem::InvalidHost) }
}

fn is_bracketed_v6(s: &str) -> bool {
    s.strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .is_some_and(|s| s.parse::<std::net::Ipv6Addr>().is_ok())
}

/// RFC 1123 host name: dot-separated labels of 1–63 ASCII letters, digits or hyphens, not
/// starting or ending with a hyphen; one trailing dot allowed; not purely numeric (that would
/// be a malformed IPv4 address).
fn is_dns_name(s: &str) -> bool {
    let s = s.strip_suffix('.').unwrap_or(s);
    if s.is_empty() || s.len() > MAX_HOST_LEN {
        return false;
    }
    let labels_ok = s.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    labels_ok && !s.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

fn check_linux_username(mode: ConnectMode, user: Option<&str>) -> Option<ProfileProblem> {
    let user = user?;
    if mode != ConnectMode::RemoteLogin {
        return Some(ProfileProblem::NotApplicable);
    }
    if let Some(problem) = check_text(user, MAX_USERNAME_CHARS) {
        return Some(problem);
    }
    // Characters that can never be part of a Linux account name.
    let bad = |c: char| c.is_whitespace() || matches!(c, ':' | '/' | ',');
    if user.chars().any(bad) || user.starts_with('-') {
        return Some(ProfileProblem::InvalidCharacters);
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Secrets addressing (plan §2 decision 7)
// ---------------------------------------------------------------------------------------------

/// Which password of a profile a Keychain item holds (plan §2 decision 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum SecretRole {
    /// Remote Login: the system daemon's RDP credentials.
    RdpSystem,
    /// Headless / Desktop Sharing: the daemon's RDP credentials.
    RdpUser,
    /// Remote Login: the Linux password typed into the greeter (explicit opt-in only).
    LinuxLogin,
}

impl SecretRole {
    /// Role name as used in Keychain account strings.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RdpSystem => "rdp-system",
            Self::RdpUser => "rdp-user",
            Self::LinuxLogin => "linux-login",
        }
    }

    /// The role holding the RDP password for `mode`.
    pub const fn rdp_for(mode: ConnectMode) -> Self {
        match mode {
            ConnectMode::RemoteLogin => Self::RdpSystem,
            ConnectMode::Headless | ConnectMode::DesktopSharing => Self::RdpUser,
        }
    }

    /// Keychain account name for this role of `profile_id`: `<uuid>/<role>`.
    pub fn account(self, profile_id: Uuid) -> String {
        format!("{profile_id}/{}", self.as_str())
    }
}
