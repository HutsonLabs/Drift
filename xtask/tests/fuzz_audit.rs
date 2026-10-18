//! M9-2 Red: every parser the plan wants fuzzed has a `cargo fuzz` target that the nightly
//! workflow actually runs (plan M9-2: "nightly fuzzing of the GFX, ZGFX, redirection, RDSTLS,
//! CLIPRDR and pointer parsers").

use xtask::fuzz_audit::{self, FuzzCrate, MIN_SECONDS, REQUIRED};

fn krate(dir: &str, targets: &[&str]) -> FuzzCrate {
    let manifest = targets
        .iter()
        .map(|t| format!("[[bin]]\nname = \"{t}\"\npath = \"fuzz_targets/{t}.rs\"\ntest = false\n"))
        .collect::<String>();
    FuzzCrate {
        dir: dir.to_owned(),
        manifest: format!("[package]\nname = \"x\"\n\n{manifest}"),
        targets: targets.iter().map(|t| (*t).to_owned()).collect(),
    }
}

/// A workflow that runs every required target for five minutes.
fn complete_workflow() -> String {
    let matrix: String = REQUIRED
        .iter()
        .map(|r| format!("          - fuzz_dir: {}\n            target: {}\n", r.dir, r.target))
        .collect();
    format!("{matrix}      run: cargo +nightly fuzz run -- -max_total_time=300\n")
}

fn complete_crates() -> Vec<FuzzCrate> {
    let mut dirs: Vec<&str> = REQUIRED.iter().map(|r| r.dir).collect();
    dirs.sort_unstable();
    dirs.dedup();
    dirs.into_iter()
        .map(|dir| {
            let targets: Vec<&str> = REQUIRED.iter().filter(|r| r.dir == dir).map(|r| r.target).collect();
            krate(dir, &targets)
        })
        .collect()
}

#[test]
fn the_plans_six_parsers_and_the_drift_channel_are_all_required() {
    // Plan M9-2's six parsers, plus the `Drift::Mac::v1` framing (EXT-1).
    let parsers: Vec<&str> = REQUIRED.iter().map(|r| r.parser).collect();
    assert_eq!(parsers, ["GFX", "ZGFX", "redirection", "RDSTLS", "CLIPRDR", "pointer", "Drift::Mac::v1"]);
}

#[test]
fn a_complete_set_has_no_problems() {
    assert_eq!(fuzz_audit::audit(&complete_crates(), &complete_workflow()), Vec::new());
}

#[test]
fn a_missing_workspace_target_or_matrix_entry_is_reported() {
    // No fuzz workspace at all.
    let problems = fuzz_audit::audit(&[], &complete_workflow());
    assert_eq!(problems.len(), REQUIRED.len(), "{problems:?}");

    // The file exists but the manifest forgot the [[bin]], and vice versa.
    let mut crates = complete_crates();
    crates[0].manifest = "[package]\nname = \"x\"\n".to_owned();
    let problems = fuzz_audit::audit(&crates, &complete_workflow());
    assert!(problems.iter().any(|p| p.message.contains("[[bin]]")), "{problems:?}");

    let mut crates = complete_crates();
    crates[0].targets.clear();
    let problems = fuzz_audit::audit(&crates, &complete_workflow());
    assert!(problems.iter().any(|p| p.message.contains("is missing")), "{problems:?}");

    // The target exists but nothing runs it nightly.
    let problems = fuzz_audit::audit(&complete_crates(), "");
    assert!(problems.iter().any(|p| p.message.contains("fuzz-nightly.yml does not run it")), "{problems:?}");
}

#[test]
fn a_too_short_nightly_run_is_reported() {
    let short = complete_workflow().replace("300", "30");
    let problems = fuzz_audit::audit(&complete_crates(), &short);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].message.contains(&MIN_SECONDS.to_string()), "{problems:?}");
}

/// The real repository: this is the M9-2 Red test.
#[test]
fn the_repository_has_every_required_fuzz_target() {
    let problems = fuzz_audit::audit_repo(&xtask::repo::root()).expect("reading the fuzz workspaces");
    assert!(problems.is_empty(), "plan M9-2 fuzz gaps:\n{}", {
        problems.iter().map(|p| format!("  {p}\n")).collect::<String>()
    });
}
