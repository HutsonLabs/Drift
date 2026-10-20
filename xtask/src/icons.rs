//! The app icon set (tasks M9-4, APP-icon).
//!
//! Drift's icon is an Icon Composer file ([`SOURCE`]). The Mac bundle lists it in
//! `tauri.conf.json`, and Tauri compiles it into `Assets.car` with actool; the iOS Xcode project
//! takes the same file as its `AppIcon`. The flat PNGs and `icon.icns` are rendered from it for
//! places that cannot read an asset catalog. [`audit`] fails on Windows or Android artefacts,
//! and checks that every file `tauri.conf.json` bundles exists with the right size.

use std::path::Path;

/// The Icon Composer source, relative to the workspace root.
pub const SOURCE: &str = "src-tauri/icons/AppIcon.icon";

/// Bundled icons, relative to `src-tauri/`, with the pixel size a PNG must have.
pub const REQUIRED: &[(&str, Option<u32>)] = &[
    ("icons/AppIcon.icon/icon.json", None),
    ("icons/32x32.png", Some(32)),
    ("icons/128x128.png", Some(128)),
    ("icons/128x128@2x.png", Some(256)),
    ("icons/icon.png", Some(512)),
    ("icons/icon.icns", None),
];

/// Icon artefacts of other platforms; plan §0 ships none of them.
pub const FORBIDDEN_SUFFIXES: &[&str] = &[".ico"];
/// Icon file-name prefixes of other platforms (`cargo tauri icon` writes these for Windows).
pub const FORBIDDEN_PREFIXES: &[&str] = &["Square", "StoreLogo"];

/// The pixel size in a PNG's IHDR chunk, or `None` when `bytes` is not a PNG.
#[must_use]
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || !bytes.starts_with(MAGIC) || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

/// Whether `bytes` is an Apple icon image (`icns` magic).
#[must_use]
pub fn is_icns(bytes: &[u8]) -> bool {
    bytes.len() > 8 && bytes.starts_with(b"icns")
}

/// Everything wrong with the icon set under `src_tauri` (empty means it is complete).
#[must_use]
pub fn audit(src_tauri: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    for (relative, size) in REQUIRED {
        let path = src_tauri.join(relative);
        let Ok(bytes) = std::fs::read(&path) else {
            problems.push(format!("missing {relative} (render it from {SOURCE})"));
            continue;
        };
        if bytes.is_empty() {
            problems.push(format!("{relative} is empty"));
            continue;
        }
        match (*size, relative.ends_with(".icns")) {
            (Some(expected), _) => match png_size(&bytes) {
                Some((w, h)) if w == expected && h == expected => {}
                Some((w, h)) => problems.push(format!("{relative} is {w}x{h}, expected {expected} square")),
                None => problems.push(format!("{relative} is not a PNG")),
            },
            (None, true) if !is_icns(&bytes) => problems.push(format!("{relative} is not an icns file")),
            (None, _) => {}
        }
    }
    let dir = src_tauri.join("icons");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if FORBIDDEN_SUFFIXES.iter().any(|s| name.ends_with(s))
                || FORBIDDEN_PREFIXES.iter().any(|p| name.starts_with(p))
            {
                problems.push(format!("icons/{name} is a non-macOS artefact (plan §0: macOS only)"));
            }
        }
    }
    // The bundle must ship exactly the files that exist.
    if let Ok(text) = std::fs::read_to_string(src_tauri.join("tauri.conf.json"))
        && let Ok(config) = serde_json::from_str::<serde_json::Value>(&text)
    {
        let listed: Vec<String> = config
            .get("bundle")
            .and_then(|b| b.get("icon"))
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(ToOwned::to_owned)).collect())
            .unwrap_or_default();
        for entry in &listed {
            if !src_tauri.join(entry).exists() {
                problems.push(format!("tauri.conf.json bundles {entry}, which does not exist"));
            }
        }
        if !listed.iter().any(|l| l == BUNDLED_SOURCE) {
            problems.push(format!("tauri.conf.json does not bundle {BUNDLED_SOURCE}"));
        }
        for (relative, size) in REQUIRED {
            if size.is_some() && !listed.iter().any(|l| l == relative) {
                problems.push(format!("tauri.conf.json does not bundle {relative}"));
            }
        }
    }
    problems
}

/// [`SOURCE`] as `tauri.conf.json` lists it, relative to `src-tauri/`.
pub const BUNDLED_SOURCE: &str = "icons/AppIcon.icon";

/// Everything wrong with an Icon Composer file (empty means it is complete): `icon.json` parses
/// and every layer image it names is in `Assets/`.
#[must_use]
pub fn icon_composer_audit(icon: &Path) -> Vec<String> {
    let name = icon.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let Some(document) = std::fs::read_to_string(icon.join("icon.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    else {
        return vec![format!("{name}/icon.json is missing or not JSON")];
    };
    let mut problems = Vec::new();
    let layers: Vec<&str> = document
        .get("groups")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| g.get("layers").and_then(serde_json::Value::as_array))
        .flatten()
        .filter_map(|l| l.get("image-name").and_then(serde_json::Value::as_str))
        .collect();
    if layers.is_empty() {
        problems.push(format!("{name}/icon.json draws no layers"));
    }
    for image in layers {
        if !icon.join("Assets").join(image).is_file() {
            problems.push(format!("{name}/Assets/{image} is missing"));
        }
    }
    problems
}

/// The Xcode project spec, relative to `src-tauri/`; it adds [`SOURCE`] to the iOS target.
pub const IOS_PROJECT_SPEC: &str = "gen/apple/project.yml";

/// The generated Xcode project, relative to `src-tauri/` (IOS-5: tracked).
pub const IOS_PROJECT: &str = "gen/apple/drift-app.xcodeproj/project.pbxproj";

/// The PNG icon set `cargo tauri ios init` writes, relative to `src-tauri/`. It carried Tauri's
/// logo into TestFlight 26.10 (4), and an appiconset named `AppIcon` would compete with
/// `AppIcon.icon`, so it must not exist.
pub const IOS_PNG_SET: &str = "gen/apple/Assets.xcassets/AppIcon.appiconset";

/// Everything wrong with the iOS app icon under `src_tauri` (empty means it is right): the
/// project spec and the generated project both take [`SOURCE`] as a resource, the target's app
/// icon is `AppIcon`, and no PNG appiconset is left.
#[must_use]
pub fn ios_audit(src_tauri: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    match std::fs::read_to_string(src_tauri.join(IOS_PROJECT_SPEC)) {
        Ok(spec) if spec.contains("path: ../../icons/AppIcon.icon") => {}
        Ok(_) => problems.push(format!("{IOS_PROJECT_SPEC} does not add ../../icons/AppIcon.icon")),
        Err(_) => problems.push(format!("missing {IOS_PROJECT_SPEC}")),
    }
    match std::fs::read_to_string(src_tauri.join(IOS_PROJECT)) {
        Ok(project) => {
            if !project.contains("/* AppIcon.icon in Resources */,") {
                problems.push(format!("{IOS_PROJECT} does not copy AppIcon.icon (run xcodegen)"));
            }
            if !project.contains("ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon;") {
                problems.push(format!("{IOS_PROJECT}: the app icon is not AppIcon"));
            }
        }
        Err(_) => problems.push(format!("missing {IOS_PROJECT}")),
    }
    if src_tauri.join(IOS_PNG_SET).exists() {
        problems.push(format!("{IOS_PNG_SET} competes with AppIcon.icon; delete it"));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_size_needs_the_magic_and_the_ihdr() {
        let mut png = Vec::from(b"\x89PNG\r\n\x1a\n");
        png.extend_from_slice(&13_u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&64_u32.to_be_bytes());
        png.extend_from_slice(&32_u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((64, 32)));
        png[1] = b'X';
        assert_eq!(png_size(&png), None);
    }

    #[test]
    fn an_empty_directory_reports_every_file() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let problems = audit(dir.path());
        assert_eq!(problems.len(), REQUIRED.len(), "{problems:?}");
        assert!(problems.iter().all(|p| p.starts_with("missing ")), "{problems:?}");
    }
}
