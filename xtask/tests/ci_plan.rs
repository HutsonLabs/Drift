//! Red (M6-2, M8-3, M0-2, §10): `cargo xtask ci` must keep building the feature-gated targets
//! and must run the vendored IronRDP workspace's own tests.
//!
//! The gate is described as data by `xtask::ci_plan`, and `main.rs` executes exactly that list,
//! so these assertions are assertions about the real merge gate.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use xtask::ci_plan::{self, Action, Check, Options, Program, Step};

fn root() -> PathBuf {
    xtask::repo::root()
}

fn read(path: &str) -> String {
    let p = root().join(path);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()))
}

/// Every `Cargo.toml` of a root-workspace member, as `(package name, manifest text)`.
fn workspace_manifests() -> Vec<(String, String)> {
    let root = root();
    let manifest = read("Cargo.toml").parse::<toml::Table>().expect("root Cargo.toml parses");
    let members = manifest["workspace"]["members"].as_array().expect("workspace.members").clone();
    members
        .iter()
        .map(|m| {
            let dir = m.as_str().expect("member path");
            let text = std::fs::read_to_string(root.join(dir).join("Cargo.toml"))
                .unwrap_or_else(|e| panic!("reading {dir}/Cargo.toml: {e}"));
            let name = text
                .parse::<toml::Table>()
                .ok()
                .and_then(|t| t["package"]["name"].as_str().map(str::to_owned))
                .unwrap_or_else(|| dir.to_owned());
            (name, text)
        })
        .collect()
}

/// Every declared `(package, feature)` pair in the root workspace, `default` excluded.
fn declared_features() -> Vec<(String, String)> {
    workspace_manifests()
        .into_iter()
        .flat_map(|(name, text)| {
            ci_plan::declared_features(&text).into_iter().map(move |f| (name.clone(), f))
        })
        .collect()
}

#[test]
fn the_gate_builds_every_declared_feature() {
    let features = declared_features();
    assert!(
        features.iter().any(|(p, f)| p == "drift-app" && f == "recording"),
        "M8-3's recording feature should be declared: {features:?}"
    );
    assert!(
        features.iter().any(|(p, f)| p == "drift-app" && f == "macos-ui-tests"),
        "M6-2's macos-ui-tests feature should be declared: {features:?}"
    );
    let gaps = ci_plan::feature_lint_gaps(&features, &ci_plan::ci(Options::default()));
    assert!(gaps.is_empty(), "cargo xtask ci never builds these features: {gaps:?}");
}

#[test]
fn the_fast_check_subset_builds_every_declared_feature() {
    let gaps = ci_plan::feature_lint_gaps(&declared_features(), &ci_plan::check_only());
    assert!(gaps.is_empty(), "cargo xtask check never builds these features: {gaps:?}");
}

#[test]
fn feature_gaps_are_reported_when_the_lint_list_is_narrow() {
    let narrow = vec![Step {
        name: "clippy".into(),
        action: Action::Run {
            program: Program::Cargo,
            args: ["clippy", "--workspace", "--all-targets", "--features", "drift-video/recording"]
                .map(str::to_owned)
                .to_vec(),
            dir: PathBuf::new(),
        },
    }];
    let features = [
        ("drift-app".to_owned(), "recording".to_owned()),
        ("drift-video".to_owned(), "recording".to_owned()),
    ];
    assert_eq!(ci_plan::feature_lint_gaps(&features, &narrow), vec!["drift-app/recording".to_owned()]);
}

#[test]
fn the_gate_runs_the_vendored_ironrdp_tests() {
    let steps = ci_plan::ci(Options::default());
    let vendored: Vec<&Step> = steps.iter().filter(|s| s.dir() == Path::new(ci_plan::VENDORED_DIR)).collect();
    assert!(!vendored.is_empty(), "no cargo xtask ci step runs in {}", ci_plan::VENDORED_DIR);
    assert!(
        vendored.iter().any(|s| s.is_cargo_with(&["test", "-p ironrdp-testsuite-core"])),
        "M0-2 Done is \"the fork's own tests pass\", but no step runs them: {:?}",
        vendored.iter().map(|s| s.command_line()).collect::<Vec<_>>()
    );
    assert!(
        vendored.iter().any(|s| s.is_cargo_with(&["fmt", "--check"])),
        "the vendored workspace is not format-checked: {:?}",
        vendored.iter().map(|s| s.command_line()).collect::<Vec<_>>()
    );
    assert!(
        vendored.iter().any(|s| s.is_cargo_with(&["test", "--locked"])),
        "the vendored tree has its own committed Cargo.lock; tests must run --locked: {:?}",
        vendored.iter().map(|s| s.command_line()).collect::<Vec<_>>()
    );
}

/// Every vendored crate that Drift both patches and consumes must have its tests run, so a
/// re-applied patch queue or a rev bump cannot silently break it.
///
/// `ironrdp-client` and `ironrdp-web` are patched for upstream compatibility only: Drift does
/// not depend on them and they do not build on a macOS host (libopus / wasm-bindgen), see
/// `docs/adr/M0-6-ci-gate-coverage.md`.
#[test]
fn the_vendored_test_step_covers_every_patched_crate_drift_depends_on() {
    let dir = root().join(ci_plan::VENDORED_PATCHES_DIR);
    let mut patched = BTreeSet::new();
    for entry in std::fs::read_dir(&dir).expect("patch queue directory") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "patch") {
            patched.extend(ci_plan::patched_crates(&std::fs::read_to_string(&path).expect("patch")));
        }
    }
    assert!(patched.contains("ironrdp-pdu"), "patch queue should touch ironrdp-pdu: {patched:?}");

    let consumed = ci_plan::vendored_dependencies(&read("Cargo.toml"));
    assert!(consumed.contains("ironrdp-session"), "Drift depends on ironrdp-session: {consumed:?}");

    let steps = ci_plan::ci(Options::default());
    let lines: Vec<String> = steps
        .iter()
        .filter(|s| s.dir() == Path::new(ci_plan::VENDORED_DIR))
        .filter_map(Step::command_line)
        .filter(|l| l.contains(" test"))
        .collect();
    let mut want: BTreeSet<&String> = patched.intersection(&consumed).collect();
    let testsuite = "ironrdp-testsuite-core".to_owned();
    want.insert(&testsuite);
    for krate in want {
        assert!(
            lines.iter().any(|l| l.contains(&format!("-p {krate}"))),
            "Drift patches and depends on {krate}, but no vendored test step runs its tests: {lines:?}"
        );
    }
}

#[test]
fn vendored_dependencies_are_the_ironrdp_path_deps() {
    let manifest = "\
[workspace.dependencies]
ironrdp-pdu = { path = \"third_party/ironrdp/crates/ironrdp-pdu\" }
ironrdp-server = { path = \"third_party/ironrdp/crates/ironrdp-server\", default-features = false }
tokio = { version = \"1\" }
drift-core = { path = \"crates/drift-core\" }
";
    assert_eq!(
        ci_plan::vendored_dependencies(manifest),
        ["ironrdp-pdu".to_owned(), "ironrdp-server".to_owned()].into_iter().collect::<BTreeSet<_>>()
    );
}

/// M0-2's four listed Red tests live in these files; a rev bump must not drop them.
#[test]
fn the_m0_2_red_test_files_exist_in_the_vendored_testsuite() {
    for f in [
        "crates/ironrdp-testsuite-core/tests/pdu/server_redirection.rs",
        "crates/ironrdp-testsuite-core/tests/connector/rdstls.rs",
        "crates/ironrdp-testsuite-core/tests/clipboard/temporary_directory.rs",
    ] {
        let p = root().join(ci_plan::VENDORED_DIR).join(f);
        assert!(p.is_file(), "missing M0-2 Red test file {}", p.display());
    }
}

#[test]
fn every_step_runs_in_a_directory_that_exists() {
    for step in ci_plan::ci(Options::default()).iter().chain(ci_plan::check_only().iter()) {
        let dir = root().join(step.dir());
        assert!(dir.is_dir(), "step `{}` runs in missing directory {}", step.name, dir.display());
    }
}

#[test]
fn options_toggle_coverage_and_deny() {
    let full = ci_plan::ci(Options::default());
    assert!(full.iter().any(|s| s.is_cargo_with(&["llvm-cov", "nextest"])));
    assert!(full.iter().any(|s| s.action == Action::InProcess(Check::CoverageGate)));
    assert!(full.iter().any(|s| s.is_cargo_with(&["deny"])));

    let lean = ci_plan::ci(Options { coverage: false, deny: false });
    assert!(!lean.iter().any(|s| s.is_cargo_with(&["llvm-cov"])));
    assert!(!lean.iter().any(|s| s.action == Action::InProcess(Check::CoverageGate)));
    assert!(!lean.iter().any(|s| s.is_cargo_with(&["deny"])));
    assert!(lean.iter().any(|s| s.is_cargo_with(&["nextest", "run"])), "tests still run without coverage");
}

#[test]
fn the_gate_starts_with_the_hygiene_checks_and_never_uses_npm() {
    let steps = ci_plan::ci(Options::default());
    assert_eq!(steps[0].action, Action::InProcess(Check::NpmBan));
    assert_eq!(steps[1].action, Action::InProcess(Check::SecretScan));
    assert!(steps.iter().any(|s| s.action == Action::InProcess(Check::Workflows)), "workflows are audited");
    let ui_steps: Vec<&Step> = steps.iter().filter(|s| s.dir() == Path::new("ui")).collect();
    assert!(!ui_steps.is_empty(), "the UI is built and tested");
    for step in ui_steps {
        assert!(
            matches!(step.action, Action::Run { program: Program::Bun, .. }),
            "`{}` must use bun, never npm (plan §0)",
            step.name
        );
    }
}

#[test]
fn patched_crates_reads_diff_headers() {
    let patch = "\
diff --git a/crates/ironrdp-pdu/src/lib.rs b/crates/ironrdp-pdu/src/lib.rs
--- a/crates/ironrdp-pdu/src/lib.rs
+++ b/crates/ironrdp-pdu/src/lib.rs
@@
diff --git a/crates/ironrdp-testsuite-core/tests/pdu/mod.rs b/crates/ironrdp-testsuite-core/tests/pdu/mod.rs
+++ b/crates/ironrdp-testsuite-core/tests/pdu/mod.rs
+++ b/README.md
";
    let crates = ci_plan::patched_crates(patch);
    assert_eq!(
        crates,
        ["ironrdp-pdu".to_owned(), "ironrdp-testsuite-core".to_owned()].into_iter().collect::<BTreeSet<_>>()
    );
}

#[test]
fn declared_features_skips_default() {
    let manifest = "[package]\nname = \"x\"\n[features]\ndefault = [\"a\"]\na = []\nb = [\"dep:c\"]\n";
    assert_eq!(ci_plan::declared_features(manifest), vec!["a".to_owned(), "b".to_owned()]);
    assert!(ci_plan::declared_features("[package]\nname = \"x\"\n").is_empty());
    assert!(ci_plan::declared_features("not toml {{{").is_empty());
}

// --- IOS-2: the platform-neutral and Apple-framework crates keep building for iOS ---

/// The crates ADR `IOS-0` decision 8 requires to build for iOS.
/// IOS-3 adds the shared Apple layer and the UIKit layer.
const IOS_CRATES: &[&str] = &[
    "drift-core",
    "drift-rdp",
    "drift-gfx",
    "drift-codec",
    "drift-input",
    "drift-video",
    "drift-render",
    "drift-apple",
    "drift-ios",
];

#[test]
fn the_gate_lints_the_ios_crates_for_both_ios_targets() {
    assert_eq!(ci_plan::IOS_TARGETS, ["aarch64-apple-ios", "aarch64-apple-ios-sim"]);
    for crate_name in IOS_CRATES {
        assert!(ci_plan::IOS_CRATES.contains(crate_name), "{crate_name} is not in ci_plan::IOS_CRATES");
    }
    for steps in [ci_plan::ci(Options::default()), ci_plan::ci(Options { coverage: false, deny: false })] {
        for target in ci_plan::IOS_TARGETS {
            let step = steps
                .iter()
                .find(|s| s.is_cargo_with(&["clippy", "--target", target, "--all-targets"]))
                .unwrap_or_else(|| panic!("no clippy step for {target}"));
            let line = step.command_line().unwrap_or_default();
            assert!(line.contains("-D warnings"), "`{line}` must deny warnings");
            for crate_name in IOS_CRATES {
                assert!(line.contains(&format!("-p {crate_name}")), "`{line}` does not lint {crate_name}");
            }
        }
    }
}

#[test]
fn the_gate_links_the_ios_crates_tests_for_the_simulator() {
    // clippy never links, so a stray `-framework AppKit` only shows up when a binary is linked.
    let steps = ci_plan::ci(Options::default());
    let step = steps
        .iter()
        .find(|s| s.is_cargo_with(&["test", "--no-run", "--target", "aarch64-apple-ios-sim"]))
        .expect("no step links test binaries for aarch64-apple-ios-sim");
    let line = step.command_line().unwrap_or_default();
    for crate_name in IOS_CRATES {
        assert!(line.contains(&format!("-p {crate_name}")), "`{line}` does not link {crate_name}");
    }
}

#[test]
fn the_toolchain_file_installs_the_ios_targets() {
    let toolchain = read("rust-toolchain.toml").parse::<toml::Table>().expect("rust-toolchain.toml parses");
    let targets: Vec<&str> = toolchain["toolchain"]["targets"]
        .as_array()
        .expect("toolchain.targets")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect();
    for target in ci_plan::IOS_TARGETS {
        assert!(targets.contains(target), "rust-toolchain.toml does not install {target}: {targets:?}");
    }
}

#[test]
fn appkit_is_only_a_macos_target_dependency() {
    // ADR IOS-0 decision 8: every AppKit use is gated on `target_os = "macos"`, starting with
    // the manifests, so no iOS build ever links AppKit.
    for (name, text) in workspace_manifests() {
        let manifest = text.parse::<toml::Table>().expect("manifest parses");
        for table in ["dependencies", "dev-dependencies", "build-dependencies"] {
            let deps = manifest.get(table).and_then(toml::Value::as_table);
            assert!(
                deps.is_none_or(|d| !d.contains_key("objc2-app-kit")),
                "{name}: objc2-app-kit must live under [target.'cfg(target_os = \"macos\")'.{table}]"
            );
        }
        if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
            for (cfg, spec) in targets {
                let has_appkit = ["dependencies", "dev-dependencies"].iter().any(|t| {
                    spec.get(t)
                        .and_then(toml::Value::as_table)
                        .is_some_and(|d| d.contains_key("objc2-app-kit"))
                });
                assert!(
                    !has_appkit || cfg == "cfg(target_os = \"macos\")",
                    "{name}: objc2-app-kit is declared under `{cfg}`"
                );
            }
        }
    }
}

#[test]
fn uikit_is_only_an_ios_target_dependency() {
    // IOS-3: the mirror image of the AppKit rule; UIKit (`objc2-ui-kit`) never reaches macOS.
    for (name, text) in workspace_manifests() {
        let manifest = text.parse::<toml::Table>().expect("manifest parses");
        for table in ["dependencies", "dev-dependencies", "build-dependencies"] {
            let deps = manifest.get(table).and_then(toml::Value::as_table);
            assert!(
                deps.is_none_or(|d| !d.contains_key("objc2-ui-kit")),
                "{name}: objc2-ui-kit must live under [target.'cfg(target_os = \"ios\")'.{table}]"
            );
        }
        if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
            for (cfg, spec) in targets {
                let has_uikit = spec
                    .get("dependencies")
                    .and_then(toml::Value::as_table)
                    .is_some_and(|d| d.contains_key("objc2-ui-kit"));
                assert!(!has_uikit || cfg == "cfg(target_os = \"ios\")", "{name}: objc2-ui-kit under `{cfg}`");
            }
        }
    }
}

#[test]
fn drift_ios_uses_uikit_and_drift_apple_does_not() {
    let by_name: std::collections::BTreeMap<String, String> = workspace_manifests().into_iter().collect();
    let ios = by_name.get("drift-ios").expect("drift-ios is a workspace member");
    assert!(ios.contains("objc2-ui-kit"), "drift-ios is the UIKit layer");
    let apple = by_name.get("drift-apple").expect("drift-apple is a workspace member");
    for framework in ["objc2-ui-kit", "objc2-app-kit"] {
        assert!(!apple.contains(framework), "drift-apple is shared by macOS and iOS; no {framework}");
    }
    let macos = by_name.get("drift-macos").expect("drift-macos");
    assert!(macos.contains("drift-apple"), "drift-macos builds on drift-apple");
}
