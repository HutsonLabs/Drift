//! `cargo xtask testflight`: build, validate and upload the iOS app (STORE-3, ADR
//! `STORE-3-ios-app-store`).
//!
//! The marketing version is `yy.mm` (`CFBundleShortVersionString`, set by hand in the iOS
//! `Info.plist` and `project.yml`); the build number grows by one with every upload. It is one
//! above the highest build App Store Connect already has for Drift, so it never repeats and
//! never needs a commit.
//!
//! The decisions are pure and tested here; `main.rs` signs the App Store Connect token with the
//! `openssl` CLI, talks to the API with `curl` and runs `cargo tauri` and `altool`.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// Drift's App Store Connect app id ("Drift - Screen sharing", `com.hutsonlabs.drift`).
pub const APP_ID: &str = "6824351800";

/// The highest build number in `existing`, plus one (1 when there is none). A dotted build
/// number counts by its first component; anything unparsable is ignored.
pub fn next_build_number(existing: &[String]) -> u32 {
    existing
        .iter()
        .filter_map(|v| v.split('.').next().and_then(|first| first.parse::<u32>().ok()))
        .max()
        .map_or(1, |max| max.saturating_add(1))
}

/// The `version` (build number) of every build in an App Store Connect `GET /v1/builds`
/// response.
pub fn build_versions(json: &str) -> Result<Vec<String>> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("App Store Connect sent invalid JSON")?;
    let data = value["data"].as_array().context("the builds response has no data")?;
    Ok(data.iter().filter_map(|b| b["attributes"]["version"].as_str().map(str::to_owned)).collect())
}

/// Whether `version` is a `yy.mm` marketing version (two digits, a dot, a month 01–12).
pub fn is_marketing_version(version: &str) -> bool {
    let Some((year, month)) = version.split_once('.') else { return false };
    let digits = |s: &str| s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit());
    digits(year) && digits(month) && matches!(month.parse::<u8>(), Ok(1..=12))
}

/// The `<string>` value of `key` in a property list's text.
pub fn plist_string(plist: &str, key: &str) -> Option<String> {
    let rest = &plist[plist.find(&format!("<key>{key}</key>"))?..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")? + start;
    Some(rest[start..end].to_owned())
}

/// What `cargo xtask testflight` runs for build number `build`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// `cargo` arguments that archive and export the App Store `.ipa` (run in `src-tauri`).
    pub build_args: Vec<String>,
    /// The exported `.ipa`.
    pub ipa: PathBuf,
    /// `xcrun` arguments that validate it (the API key arguments are appended at run time).
    pub validate_args: Vec<String>,
    /// `xcrun` arguments that upload it (the API key arguments are appended at run time).
    pub upload_args: Vec<String>,
}

/// The plan for build number `build` in the repository at `root`.
pub fn plan(root: &Path, build: u32) -> Plan {
    let ipa = root.join("src-tauri/gen/apple/build/arm64/Drift.ipa");
    let altool = |action: &str| {
        vec![
            "altool".to_owned(),
            action.to_owned(),
            "-f".to_owned(),
            ipa.display().to_string(),
            "-t".to_owned(),
            "ios".to_owned(),
        ]
    };
    Plan {
        // Tauri appends `--build-number` to `bundleVersion` ("1.2"), so the build number replaces
        // `bundleVersion` through a config override instead.
        build_args: ["tauri", "ios", "build", "--export-method", "app-store-connect", "--config"]
            .iter()
            .map(|s| (*s).to_owned())
            .chain(std::iter::once(format!(r#"{{"bundle":{{"iOS":{{"bundleVersion":"{build}"}}}}}}"#)))
            .collect(),
        validate_args: altool("--validate-app"),
        upload_args: altool("--upload-app"),
        ipa,
    }
}

/// URL-safe base64 without padding (JWT segments).
pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
        }
    }
    out
}

/// An ES256 signature as `openssl dgst -sign` writes it (DER `SEQUENCE { r, s }`) turned into
/// the 64-byte `r ‖ s` a JWT carries.
pub fn der_signature_to_raw(der: &[u8]) -> Option<[u8; 64]> {
    let mut i = match *der.get(1)? {
        len if len < 0x80 => 2,
        len => 2 + usize::from(len & 0x7f),
    };
    let mut raw = [0u8; 64];
    for half in 0..2 {
        if *der.get(i)? != 2 {
            return None;
        }
        let len = usize::from(*der.get(i + 1)?);
        let int = der.get(i + 2..i + 2 + len)?;
        let int = &int[int.iter().take_while(|b| **b == 0).count()..];
        if int.len() > 32 {
            return None;
        }
        raw[half * 32 + 32 - int.len()..(half + 1) * 32].copy_from_slice(int);
        i += 2 + len;
    }
    Some(raw)
}

/// The unsigned `header.claims` part of an App Store Connect token valid for 15 minutes from
/// `now` (seconds since the epoch).
pub fn token_body(key_id: &str, issuer: &str, now: u64) -> String {
    let header = format!(r#"{{"alg":"ES256","kid":"{key_id}","typ":"JWT"}}"#);
    let claims =
        format!(r#"{{"iss":"{issuer}","iat":{now},"exp":{},"aud":"appstoreconnect-v1"}}"#, now + 900);
    format!("{}.{}", base64url(header.as_bytes()), base64url(claims.as_bytes()))
}
