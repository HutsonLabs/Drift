//! `drift-connect:` links (ADR `GNOME-only` decision 8): a GNOME computer's connection
//! settings, as `host/drift-gnome-setup` prints and opens them, for a prefilled New Connection
//! form. A link never carries a password.
//!
//! ```text
//! drift-connect:?host=box.local&port=3390&mode=headless&user=me&name=Box&pin=f3:e7:…
//! ```
//!
//! `host` is required; `name` defaults to the host, `mode` to Headless, `port` to the mode's
//! default. `linux_user` is the Remote Login greeter's user. Values are percent-encoded; unknown
//! parameters are ignored so older Drifts open newer links.

use crate::profile::{CertFingerprint, ConnectMode, ConnectionProfile};

/// The URL scheme both apps register.
pub const SCHEME: &str = "drift-connect";

/// Why a link was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    /// Not a `drift-connect:` link.
    #[error("not a drift-connect: link")]
    NotALink,
    /// The link has no `host`.
    #[error("the link names no host")]
    NoHost,
    /// A parameter's value is not valid (percent-encoding, port, mode or fingerprint).
    #[error("the link's {0} is not valid")]
    Invalid(&'static str),
}

/// Whether `url` is a `drift-connect:` link (any case for the scheme).
pub fn is_connect_link(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(SCHEME))
}

/// Parses a link into a new, unsaved profile (fresh id, default preferences).
pub fn parse(url: &str) -> Result<ConnectionProfile, LinkError> {
    if !is_connect_link(url) {
        return Err(LinkError::NotALink);
    }
    let rest = &url[SCHEME.len() + 1..];
    let query = rest.split_once('?').map_or("", |(_, q)| q);
    let query = query.split_once('#').map_or(query, |(q, _)| q);

    let (mut name, mut host, mut port, mut mode, mut user, mut linux_user, mut pin) =
        (None, None, None, None, None, None, None);
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let (slot, field) = match key {
            "name" => (&mut name, "name"),
            "host" => (&mut host, "host"),
            "port" => (&mut port, "port"),
            "mode" => (&mut mode, "mode"),
            "user" => (&mut user, "user"),
            "linux_user" => (&mut linux_user, "linux_user"),
            "pin" => (&mut pin, "pin"),
            _ => continue,
        };
        let value = decode(value).ok_or(LinkError::Invalid(field))?;
        *slot = Some(value.trim().to_owned()).filter(|v| !v.is_empty());
    }

    let host = host.ok_or(LinkError::NoHost)?;
    let mode = match mode.as_deref() {
        None | Some("headless") => ConnectMode::Headless,
        Some("remote-login") => ConnectMode::RemoteLogin,
        Some("desktop-sharing") => ConnectMode::DesktopSharing,
        Some(_) => return Err(LinkError::Invalid("mode")),
    };
    let mut profile = ConnectionProfile::new(name.unwrap_or_else(|| host.clone()), host, mode);
    if let Some(port) = port {
        profile.port = port.parse().ok().filter(|p| *p != 0).ok_or(LinkError::Invalid("port"))?;
    }
    profile.rdp_username = user.unwrap_or_default();
    profile.linux_username = linux_user.filter(|_| mode == ConnectMode::RemoteLogin);
    profile.cert_pin = match pin {
        Some(pin) => Some(pin.parse::<CertFingerprint>().map_err(|_| LinkError::Invalid("pin"))?),
        None => None,
    };
    Ok(profile)
}

/// The link for `profile`'s connection settings (the inverse of [`parse`]).
pub fn format(profile: &ConnectionProfile) -> String {
    let mode = match profile.mode {
        ConnectMode::Headless => "headless",
        ConnectMode::RemoteLogin => "remote-login",
        ConnectMode::DesktopSharing => "desktop-sharing",
    };
    let mut out = format!(
        "{SCHEME}:?name={}&host={}&port={}&mode={mode}",
        encode(&profile.name),
        encode(&profile.host),
        profile.port
    );
    if !profile.rdp_username.is_empty() {
        out.push_str(&std::format!("&user={}", encode(&profile.rdp_username)));
    }
    if let Some(linux) = &profile.linux_username {
        out.push_str(&std::format!("&linux_user={}", encode(linux)));
    }
    if let Some(pin) = &profile.cert_pin {
        out.push_str(&std::format!("&pin={pin}"));
    }
    out
}

/// Percent-decoding (`+` is a space, as in form encoding); `None` for bad escapes or UTF-8.
fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                let hex = std::str::from_utf8(hex).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Percent-encoding of everything but unreserved characters (RFC 3986).
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&std::format!("%{b:02X}"));
        }
    }
    out
}
