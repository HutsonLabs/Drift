//! STORE-3 follow-up: `cargo xtask testflight` uploads with a build number one above the highest
//! one App Store Connect already has, and the marketing version is `yy.mm`.
#![allow(missing_docs, clippy::unwrap_used)]

use xtask::testflight;

#[test]
fn next_build_number_is_one_above_the_highest() {
    assert_eq!(testflight::next_build_number(&[]), 1);
    assert_eq!(testflight::next_build_number(&["1".into()]), 2);
    assert_eq!(testflight::next_build_number(&["1".into(), "7".into(), "3".into()]), 8);
    // Old dotted build numbers count by their first component.
    assert_eq!(testflight::next_build_number(&["0.2.0".into(), "2".into()]), 3);
    assert_eq!(testflight::next_build_number(&["junk".into()]), 1);
}

#[test]
fn build_versions_come_from_the_builds_response() {
    let json = r#"{"data":[{"type":"builds","id":"a","attributes":{"version":"1"}},
                           {"type":"builds","id":"b","attributes":{"version":"12"}}],"links":{}}"#;
    assert_eq!(testflight::build_versions(json).unwrap(), vec!["1".to_owned(), "12".to_owned()]);
    assert!(testflight::build_versions("not json").is_err());
}

#[test]
fn marketing_version_is_year_dot_month() {
    assert!(testflight::is_marketing_version("26.10"));
    assert!(testflight::is_marketing_version("27.01"));
    assert!(!testflight::is_marketing_version("26.13"));
    assert!(!testflight::is_marketing_version("26.1"));
    assert!(!testflight::is_marketing_version("0.2.0"));
}

#[test]
fn the_ios_project_declares_the_marketing_version_and_the_encryption_answer() {
    let root = xtask::repo::root();
    let plist = std::fs::read_to_string(root.join("src-tauri/gen/apple/drift-app_iOS/Info.plist")).unwrap();
    let version = testflight::plist_string(&plist, "CFBundleShortVersionString").unwrap();
    assert!(testflight::is_marketing_version(&version), "iOS version {version} is not yy.mm");
    let spec = std::fs::read_to_string(root.join("src-tauri/gen/apple/project.yml")).unwrap();
    assert!(spec.contains(&format!("CFBundleShortVersionString: \"{version}\"")), "project.yml disagrees");
    for text in [&plist, &std::fs::read_to_string(root.join("src-tauri/Info.plist")).unwrap()] {
        assert!(text.contains("<key>ITSAppUsesNonExemptEncryption</key>\n\t<false/>"));
    }
    assert!(spec.contains("ITSAppUsesNonExemptEncryption: false"));
}

#[test]
fn the_upload_plan_builds_validates_and_uploads_the_ipa() {
    let root = std::path::Path::new("/repo");
    let plan = testflight::plan(root, 9);
    assert_eq!(
        plan.build_args,
        [
            "tauri",
            "ios",
            "build",
            "--export-method",
            "app-store-connect",
            "--config",
            r#"{"bundle":{"iOS":{"bundleVersion":"9"}}}"#
        ]
    );
    assert_eq!(plan.ipa, root.join("src-tauri/gen/apple/build/arm64/Drift.ipa"));
    assert_eq!(plan.validate_args[0..2], ["altool".to_owned(), "--validate-app".to_owned()]);
    assert_eq!(plan.upload_args[0..2], ["altool".to_owned(), "--upload-app".to_owned()]);
}

#[test]
fn jwt_helpers() {
    assert_eq!(testflight::base64url(b"f"), "Zg");
    assert_eq!(testflight::base64url(b"fo"), "Zm8");
    assert_eq!(testflight::base64url(b"foo"), "Zm9v");
    assert_eq!(testflight::base64url(&[0xfb, 0xff]), "-_8");
    // r = 0x01 (padded to 32 bytes), s = 0x00ff.. with a leading sign byte.
    let mut der = vec![0x30, 0x26, 0x02, 0x01, 0x01, 0x02, 0x21, 0x00];
    der.extend([0xff; 32]);
    let raw = testflight::der_signature_to_raw(&der).unwrap();
    assert_eq!(raw[31], 1);
    assert!(raw[..31].iter().all(|b| *b == 0));
    assert!(raw[32..].iter().all(|b| *b == 0xff));
    assert!(testflight::der_signature_to_raw(&[0x30, 0x02, 0x05, 0x00]).is_none());
    let body = testflight::token_body("KEY", "ISSUER", 100);
    assert_eq!(body.split('.').count(), 2);
}

#[test]
fn the_mac_upload_validates_and_uploads_the_installer_package() {
    let pkg = std::path::Path::new("/repo/target/universal-apple-darwin/release/bundle/mas/Drift.pkg");
    let (validate, upload) = testflight::mac_altool_args(pkg);
    assert_eq!(validate, ["altool", "--validate-app", "-f", pkg.to_str().unwrap(), "-t", "macos"]);
    assert_eq!(upload[1], "--upload-app");
    assert_eq!(upload[5], "macos");
}
