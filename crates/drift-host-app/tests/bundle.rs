//! HOST-9 Red: the Drift Host bundle's Info.plist — a menu-bar agent with its own bundle id,
//! the usage strings macOS shows, and the workspace version.
#![allow(missing_docs, clippy::unwrap_used)]

fn plist() -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Info.plist")).unwrap()
}

fn value(plist: &str, key: &str) -> Option<String> {
    let marker = format!("<key>{key}</key>");
    let rest = plist[plist.find(&marker)? + marker.len()..].trim_start();
    if rest.starts_with("<true/>") {
        return Some("true".into());
    }
    let start = rest.find('>')? + 1;
    let end = rest.find("</")?;
    Some(rest[start..end].to_owned())
}

#[test]
fn bundle_is_a_menu_bar_agent_with_its_own_identity() {
    let p = plist();
    assert_eq!(value(&p, "CFBundleIdentifier").as_deref(), Some(drift_host_app::BUNDLE_ID));
    assert_eq!(drift_host_app::BUNDLE_ID, "com.hutsonlabs.drift.host");
    assert_eq!(value(&p, "LSUIElement").as_deref(), Some("true"), "no Dock icon");
    assert_eq!(value(&p, "CFBundleExecutable").as_deref(), Some(drift_host_app::EXECUTABLE));
    assert_eq!(value(&p, "CFBundleName").as_deref(), Some("Drift Host"));
}

#[test]
fn bundle_version_is_the_workspace_version() {
    // Marketing version `yy.mm` (Cargo carries it as `yy.mm.0`); the build is a plain integer.
    let p = plist();
    let short = env!("CARGO_PKG_VERSION").strip_suffix(".0").unwrap();
    assert_eq!(value(&p, "CFBundleShortVersionString").as_deref(), Some(short));
    let build = value(&p, "CFBundleVersion").unwrap();
    assert!(build.parse::<u32>().is_ok_and(|n| n > 0), "CFBundleVersion {build} is not a build number");
}

#[test]
fn bundle_explains_local_network_use() {
    let text = value(&plist(), "NSLocalNetworkUsageDescription").unwrap();
    assert!(text.contains("Drift Host") && text.len() > 30, "{text}");
}
