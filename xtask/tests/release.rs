//! RELEASE Red: `cargo xtask release` as data — version files, the step plan, resume decisions,
//! the manifest, tags, notes and checksums.
#![allow(missing_docs, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use xtask::release::{
    self, Artifact, ArtifactKind, Format, Manifest, Options, StepId, StepRecord, StepStatus, VersionFile,
};

// ---- version files ---------------------------------------------------------------------------

#[test]
fn every_version_file_is_listed_once() {
    let paths: Vec<&str> = release::VERSION_FILES.iter().map(|f| f.path).collect();
    for p in [
        "Cargo.toml",
        "src-tauri/tauri.conf.json",
        "ui/package.json",
        "src-tauri/Info.plist",
        "crates/drift-host-app/Info.plist",
        "src-tauri/gen/apple/drift-app_iOS/Info.plist",
        "src-tauri/gen/apple/project.yml",
    ] {
        assert_eq!(paths.iter().filter(|x| **x == p).count(), 1, "{p}");
    }
}

#[test]
fn the_repository_version_files_agree() {
    let root = xtask::repo::root();
    let found = release::read_versions(&root).unwrap();
    let version = release::check_versions(&found).unwrap();
    assert!(xtask::testflight::is_marketing_version(&version), "{version}");
}

#[test]
fn version_formats_read_and_write() {
    let cargo = "[workspace.package]\nversion = \"26.10.0\"\nedition = \"2024\"\n[dependencies]\nfoo = { version = \"1\" }\n";
    assert_eq!(release::read_version(cargo, Format::CargoWorkspace).as_deref(), Some("26.10.0"));
    let out = release::write_version(cargo, Format::CargoWorkspace, "26.11.0").unwrap();
    assert!(out.contains("version = \"26.11.0\"") && out.contains("foo = { version = \"1\" }"));

    let json = "{\n  \"name\": \"x\",\n  \"version\": \"26.10.0\",\n  \"deps\": {\"version\": \"9\"}\n}\n";
    assert_eq!(release::read_version(json, Format::JsonVersion).as_deref(), Some("26.10.0"));
    assert!(release::write_version(json, Format::JsonVersion, "26.11.0").unwrap().contains("\"version\": \"26.11.0\""));

    let plist = "<dict>\n\t<key>CFBundleShortVersionString</key>\n\t<string>26.10</string>\n</dict>";
    assert_eq!(release::read_version(plist, Format::PlistShort).as_deref(), Some("26.10"));
    assert!(release::write_version(plist, Format::PlistShort, "26.11").unwrap().contains("<string>26.11</string>"));

    let yml = "        CFBundleShortVersionString: \"26.10\"\n        CFBundleVersion: \"1\"\n";
    assert_eq!(release::read_version(yml, Format::YamlShort).as_deref(), Some("26.10"));
    let out = release::write_version(yml, Format::YamlShort, "26.11").unwrap();
    assert!(out.contains("CFBundleShortVersionString: \"26.11\"") && out.contains("CFBundleVersion: \"1\""));
}

#[test]
fn mismatched_or_malformed_versions_are_reported() {
    let file = |path: &'static str, semver: bool| VersionFile { path, format: Format::JsonVersion, semver };
    let ok = vec![(file("a", true), Some("26.10.0".to_owned())), (file("b", false), Some("26.10".to_owned()))];
    assert_eq!(release::check_versions(&ok).unwrap(), "26.10");
    let bad = vec![(file("a", true), Some("26.10.0".to_owned())), (file("b", false), Some("26.11".to_owned()))];
    let err = release::check_versions(&bad).unwrap_err().to_string();
    assert!(err.contains('b') && err.contains("26.11"), "{err}");
    assert!(release::check_versions(&[(file("a", true), None)]).is_err());
    assert!(release::check_versions(&[(file("a", false), Some("0.2.0".to_owned()))]).is_err());
}

#[test]
fn set_version_rewrites_every_file() {
    let files = vec![
        (VersionFile { path: "Cargo.toml", format: Format::CargoWorkspace, semver: true }, "version = \"26.10.0\"\n".to_owned()),
        (
            VersionFile { path: "x.plist", format: Format::PlistShort, semver: false },
            "<key>CFBundleShortVersionString</key>\n<string>26.10</string>".to_owned(),
        ),
    ];
    let out = release::set_version(&files, "26.11").unwrap();
    assert_eq!(out[0].1, "version = \"26.11.0\"\n");
    assert!(out[1].1.contains("<string>26.11</string>"));
    assert!(release::set_version(&files, "26.13").is_err(), "not yy.mm");
}

// ---- the plan ----------------------------------------------------------------------------------

#[test]
fn the_default_plan_builds_everything_and_publishes_nothing() {
    let steps = release::steps(Options::default()).unwrap();
    assert_eq!(
        steps,
        [
            StepId::Preflight,
            StepId::Gate,
            StepId::BuildNumber,
            StepId::Clean,
            StepId::Ios,
            StepId::MacAppStore,
            StepId::MacDmg,
            StepId::HostDmg,
            StepId::Notarize,
            StepId::Checksums,
        ]
    );
}

#[test]
fn dry_run_skips_notarization_and_publish_flags_add_steps() {
    let dry = release::steps(Options { dry_run: true, ..Options::default() }).unwrap();
    assert!(!dry.contains(&StepId::Notarize));
    assert!(dry.contains(&StepId::Ios) && dry.contains(&StepId::Checksums));

    let all = release::steps(Options { testflight: true, github: true, ..Options::default() }).unwrap();
    let tail = &all[all.len() - 3..];
    assert_eq!(tail, [StepId::Testflight, StepId::WhatsNew, StepId::Github]);

    assert!(release::steps(Options { dry_run: true, testflight: true, ..Options::default() }).is_err());
    assert!(release::steps(Options { dry_run: true, github: true, ..Options::default() }).is_err());
}

#[test]
fn only_network_and_publish_steps_retry() {
    for s in [StepId::Notarize, StepId::Testflight, StepId::WhatsNew, StepId::Github, StepId::BuildNumber] {
        assert!(s.retries(), "{s:?}");
    }
    for s in [StepId::Clean, StepId::Ios, StepId::Checksums, StepId::Preflight] {
        assert!(!s.retries(), "{s:?}");
    }
    assert_eq!(release::backoff(0), Duration::from_secs(10));
    assert_eq!(release::backoff(1), Duration::from_secs(30));
    assert_eq!(release::backoff(5), Duration::from_secs(90), "capped");
}

// ---- resume ------------------------------------------------------------------------------------

fn record(step: StepId, status: StepStatus, outputs: Vec<Artifact>) -> StepRecord {
    StepRecord { step, status, started: 1, finished: Some(2), outputs, error: None }
}

fn manifest(steps: Vec<StepRecord>) -> Manifest {
    Manifest {
        version: "26.10".into(),
        build: 4,
        git_sha: "abc".into(),
        source_date_epoch: 1,
        toolchain: "1.97.1".into(),
        lockfiles: BTreeMap::new(),
        dry_run: true,
        steps,
        artifacts: Vec::new(),
    }
}

#[test]
fn resume_skips_a_done_step_whose_outputs_are_intact() {
    let art = Artifact { kind: ArtifactKind::IosIpa, file: "Drift.ipa".into(), sha256: "h1".into(), size: 3 };
    let m = manifest(vec![record(StepId::Ios, StepStatus::Done, vec![art])]);
    assert!(release::should_skip(StepId::Ios, &m, |_| Some("h1".into())));
    assert!(!release::should_skip(StepId::Ios, &m, |_| Some("other".into())), "changed output");
    assert!(!release::should_skip(StepId::Ios, &m, |_| None), "missing output");
    assert!(!release::should_skip(StepId::MacDmg, &m, |_| Some("h1".into())), "never ran");
    let failed = manifest(vec![record(StepId::Ios, StepStatus::Failed, vec![])]);
    assert!(!release::should_skip(StepId::Ios, &failed, |_| None));
    // Steps without outputs are redone unless they are pure bookkeeping.
    let pre = manifest(vec![record(StepId::Preflight, StepStatus::Done, vec![])]);
    assert!(!release::should_skip(StepId::Preflight, &pre, |_| None), "preflight always runs");
    let bn = manifest(vec![record(StepId::BuildNumber, StepStatus::Done, vec![])]);
    assert!(release::should_skip(StepId::BuildNumber, &bn, |_| None), "the build number is reused");
}

#[test]
fn a_resume_must_match_version_and_commit() {
    let m = manifest(vec![]);
    assert!(release::resumable(&m, "26.10", "abc").is_ok());
    assert!(release::resumable(&m, "26.11", "abc").is_err());
    assert!(release::resumable(&m, "26.10", "def").is_err());
}

#[test]
fn the_manifest_round_trips_and_marks_failures() {
    let mut m = manifest(vec![record(StepId::Clean, StepStatus::Done, vec![])]);
    m.lockfiles.insert("Cargo.lock".into(), "aa".into());
    m.fail(StepId::Ios, 5, "xcodebuild failed");
    let json = m.to_json().unwrap();
    let back = Manifest::from_json(&json).unwrap();
    assert_eq!(back, m);
    let last = back.steps.last().unwrap();
    assert_eq!((last.step, last.status), (StepId::Ios, StepStatus::Failed));
    assert_eq!(last.error.as_deref(), Some("xcodebuild failed"));
    // Recording a step again replaces the earlier record.
    let mut m = back;
    m.done(StepId::Ios, 6, 7, vec![]);
    assert_eq!(m.steps.iter().filter(|r| r.step == StepId::Ios).count(), 1);
    assert_eq!(m.steps.last().unwrap().status, StepStatus::Done);
}

// ---- names, tags, notes, checksums -------------------------------------------------------------

#[test]
fn artifact_names_and_the_output_directory() {
    let root = Path::new("/repo");
    assert_eq!(release::out_dir(root, "26.10", 4), PathBuf::from("/repo/target/release-out/26.10-4"));
    assert_eq!(release::artifact_name(ArtifactKind::IosIpa, "26.10", 4), "Drift_26.10.4_iOS.ipa");
    assert_eq!(release::artifact_name(ArtifactKind::MacAppStorePkg, "26.10", 4), "Drift_26.10.4_MacAppStore.pkg");
    assert_eq!(release::artifact_name(ArtifactKind::MacDmg, "26.10", 4), "Drift_26.10.4_aarch64.dmg");
    assert_eq!(release::artifact_name(ArtifactKind::HostDmg, "26.10", 4), "Drift-Host_26.10.4_aarch64.dmg");
    assert_eq!(release::tag("26.10", 4), "v26.10.4");
    assert_eq!(release::receipt_path(root, "abc"), PathBuf::from("/repo/target/ci-receipts/abc"));
}

#[test]
fn notes_must_be_filled_in() {
    assert!(release::validate_notes("").is_err());
    assert!(release::validate_notes(release::NOTES_TEMPLATE).is_err(), "the template is not notes");
    let notes = "# Drift 26.10\n\nNew Mac host.\n\n## What to Test\n\nPair an iPad.\n";
    release::validate_notes(notes).unwrap();
    assert_eq!(release::whats_new(notes), "Pair an iPad.");
    let plain = "# Drift 26.10\n\nNew Mac host.\n";
    assert_eq!(release::whats_new(plain), "New Mac host.");
    let long = format!("# Drift\n\n{}\n", "x".repeat(5000));
    assert!(release::whats_new(&long).chars().count() <= 4000);
    assert_eq!(release::notes_path(Path::new("/r"), "26.10"), PathBuf::from("/r/release/notes/26.10.md"));
}

#[test]
fn checksums_and_github_notes() {
    let sums = release::sha256sums(&[("b.dmg".into(), "22".into()), ("a.dmg".into(), "11".into())]);
    assert_eq!(sums, "11  a.dmg\n22  b.dmg\n");
    let notes = release::github_notes("# Drift 26.10\n\nHello.\n", &sums);
    assert!(notes.starts_with("# Drift 26.10"));
    assert!(notes.contains("SHA-256") && notes.contains("11  a.dmg"));
}
