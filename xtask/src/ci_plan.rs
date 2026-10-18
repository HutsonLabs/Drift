//! The `cargo xtask ci` gate described as data.
//!
//! `main.rs` executes exactly the list [`ci`] returns, so a test can assert what the merge
//! gate covers instead of trusting a hand-written sequence of commands. That matters because
//! three plan "Done" criteria are only met if the gate keeps building code that the default
//! feature set leaves out (plan §10):
//!
//! - `drift-app/recording` — the M8-3 "Debug ▸ Record Session" hook;
//! - `drift-app/macos-ui-tests` — the real-window AppKit tests (M6-2's tab groups, M7-3's
//!   overlays and HUDs);
//! - the vendored IronRDP workspace's own tests — M0-2's Done ("the fork's own tests pass"),
//!   which live in a workspace the root `Cargo.toml` excludes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The `--features` list used for `nextest` (plan M8-2: the encoder's tests are feature-gated).
///
/// Lints use `--all-features` instead; tests cannot, because `drift-app/macos-ui-tests`
/// enables test binaries that open real windows and need a logged-in window server.
pub const NEXTEST_FEATURES: &str = "drift-video/recording";

/// Where `cargo llvm-cov` writes the summary the coverage gate reads (repository-relative).
pub const COVERAGE_JSON: &str = "target/llvm-cov-summary.json";

/// The vendored IronRDP workspace (its own cargo workspace; excluded from the root one).
pub const VENDORED_DIR: &str = "third_party/ironrdp";

/// Drift's patch queue for the vendored workspace.
pub const VENDORED_PATCHES_DIR: &str = "third_party/ironrdp-patches";

/// The program a step runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    /// `cargo` (or `$CARGO`).
    Cargo,
    /// `bun` (or `$BUN`); never npm/yarn/pnpm (plan §0).
    Bun,
}

impl Program {
    /// The name used in a rendered command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Bun => "bun",
        }
    }
}

/// An xtask check that runs in-process rather than as a subprocess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// "Bun, never npm" (plan §0, M0-1).
    NpmBan,
    /// No dev-machine secret appears in a repository file (plan §5.3).
    SecretScan,
    /// `.github/workflows/` matches what the plan requires (M0-6, §5.3).
    Workflows,
    /// Every parser plan M9-2 wants fuzzed has a target that the nightly workflow runs.
    FuzzTargets,
    /// `ui/src/bindings.ts` is regenerated from the current IPC surface.
    BindingsFresh,
    /// The ≥ 85 % line-coverage gate over [`COVERAGE_JSON`].
    CoverageGate,
}

/// What a [`Step`] does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Run an xtask check in-process.
    InProcess(Check),
    /// Run `program args…` in `dir` (repository-relative; empty means the repository root).
    Run {
        /// The program to spawn.
        program: Program,
        /// Its arguments.
        args: Vec<String>,
        /// Working directory, relative to the repository root.
        dir: PathBuf,
    },
}

/// One step of the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Human-readable label, printed as `==> xtask: <name>`.
    pub name: String,
    /// What the step does.
    pub action: Action,
}

impl Step {
    /// The rendered command line (`cargo clippy --workspace …`), or `None` for in-process checks.
    pub fn command_line(&self) -> Option<String> {
        match &self.action {
            Action::InProcess(_) => None,
            Action::Run { program, args, .. } => Some(
                std::iter::once(program.as_str().to_owned())
                    .chain(args.clone())
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        }
    }

    /// The step's working directory, relative to the repository root.
    pub fn dir(&self) -> &Path {
        match &self.action {
            Action::InProcess(_) => Path::new(""),
            Action::Run { dir, .. } => dir,
        }
    }

    /// True when this is a `cargo` step whose command line contains every needle.
    pub fn is_cargo_with(&self, needles: &[&str]) -> bool {
        match &self.action {
            Action::Run { program: Program::Cargo, .. } => {
                let line = self.command_line().unwrap_or_default();
                needles.iter().all(|n| line.contains(n))
            }
            _ => false,
        }
    }
}

/// Which optional parts of the gate to include.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Run nextest under `cargo llvm-cov` and enforce the coverage gate.
    pub coverage: bool,
    /// Run `cargo deny` (needs the advisory database, so off when offline).
    pub deny: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { coverage: true, deny: true }
    }
}

fn run(name: &str, program: Program, dir: &str, args: &[&str]) -> Step {
    Step {
        name: name.to_owned(),
        action: Action::Run {
            program,
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            dir: PathBuf::from(dir),
        },
    }
}

fn check(name: &str, c: Check) -> Step {
    Step { name: name.to_owned(), action: Action::InProcess(c) }
}

/// Vendored packages whose own tests `cargo xtask ci` runs (plan M0-2 Done: "the fork's own
/// tests pass"). These are the crates Drift both patches and depends on, plus the fork's test
/// suite, which holds M0-2's four Red tests.
///
/// `ironrdp-client` and `ironrdp-web` are patched for upstream compatibility only: Drift does
/// not depend on them, and they do not build on a macOS host (libopus, wasm-bindgen). The whole
/// vendored workspace cannot be tested for the same reason — see
/// `docs/adr/M0-6-ci-gate-coverage.md`.
pub const VENDORED_TEST_PACKAGES: &[&str] =
    &["ironrdp-testsuite-core", "ironrdp-cliprdr", "ironrdp-connector", "ironrdp-pdu", "ironrdp-session"];

/// The iOS targets the iOS crates must build for (ADR `IOS-0`, decision 8).
pub const IOS_TARGETS: &[&str] = &["aarch64-apple-ios", "aarch64-apple-ios-sim"];

/// The crates that build for iOS: platform-neutral ones plus the Apple-framework ones.
pub const IOS_CRATES: &[&str] =
    &["drift-core", "drift-rdp", "drift-gfx", "drift-codec", "drift-input", "drift-video", "drift-render"];

/// The lint pass: formatting and clippy over every target and every feature.
///
/// `--all-features` rather than a hand-maintained list: a new cargo feature is then covered by
/// the gate the moment it is declared. Tests cannot do the same (see [`NEXTEST_FEATURES`]).
fn lint_steps() -> Vec<Step> {
    vec![
        run("cargo fmt --check", Program::Cargo, "", &["fmt", "--all", "--", "--check"]),
        run(
            "cargo clippy -D warnings (--all-features)",
            Program::Cargo,
            "",
            &["clippy", "--workspace", "--all-targets", "--all-features", "--locked", "--", "-D", "warnings"],
        ),
    ]
}

/// The iOS crates for each iOS target: clippy over every target, then the simulator's test
/// binaries are linked (clippy never links, and a stray `-framework AppKit` only fails there).
///
/// The tests are not run: that needs a booted simulator, which the gate does not assume.
fn ios_steps() -> Vec<Step> {
    let packages: Vec<&str> = IOS_CRATES.iter().flat_map(|p| ["-p", p]).collect();
    let mut steps: Vec<Step> = IOS_TARGETS
        .iter()
        .map(|target| {
            let mut args = vec!["clippy", "--target", target, "--all-targets", "--locked"];
            args.extend(&packages);
            args.extend(["--", "-D", "warnings"]);
            run(&format!("cargo clippy -D warnings ({target})"), Program::Cargo, "", &args)
        })
        .collect();
    let mut args = vec!["test", "--no-run", "--target", "aarch64-apple-ios-sim", "--locked"];
    args.extend(&packages);
    steps.push(run("link the iOS crates' tests (aarch64-apple-ios-sim)", Program::Cargo, "", &args));
    steps
}

/// The vendored IronRDP workspace: format check plus the fork's own tests (plan M0-2).
fn vendored_steps() -> Vec<Step> {
    // `--locked`: the vendored tree has its own committed Cargo.lock, and re-vendoring a new
    // upstream rev must update it rather than resolve differently on every machine.
    let mut test_args = vec!["test", "--locked"];
    for p in VENDORED_TEST_PACKAGES {
        test_args.push("-p");
        test_args.push(p);
    }
    vec![
        run(
            "vendored ironrdp: cargo fmt --check",
            Program::Cargo,
            VENDORED_DIR,
            &["fmt", "--all", "--", "--check"],
        ),
        run("vendored ironrdp: cargo test", Program::Cargo, VENDORED_DIR, &test_args),
    ]
}

/// The full merge gate, in order.
pub fn ci(options: Options) -> Vec<Step> {
    let mut steps = vec![
        check("npm-ban", Check::NpmBan),
        check("secret-scan", Check::SecretScan),
        check("workflows", Check::Workflows),
        check("fuzz targets", Check::FuzzTargets),
        run("bun install --frozen-lockfile", Program::Bun, "ui", &["install", "--frozen-lockfile"]),
        run("bun test", Program::Bun, "ui", &["test"]),
        run("bun run typecheck", Program::Bun, "ui", &["run", "typecheck"]),
        run("bun run build", Program::Bun, "ui", &["run", "build"]),
        check("bindings freshness", Check::BindingsFresh),
    ];
    steps.extend(lint_steps());
    steps.extend(ios_steps());
    steps.extend(vendored_steps());
    if options.coverage {
        steps.push(run(
            "nextest under llvm-cov",
            Program::Cargo,
            "",
            &[
                "llvm-cov",
                "nextest",
                "--workspace",
                "--features",
                NEXTEST_FEATURES,
                "--json",
                "--summary-only",
                "--output-path",
                COVERAGE_JSON,
            ],
        ));
        steps.push(check("coverage gate", Check::CoverageGate));
    } else {
        steps.push(run(
            "nextest",
            Program::Cargo,
            "",
            &["nextest", "run", "--workspace", "--features", NEXTEST_FEATURES],
        ));
    }
    if options.deny {
        steps.push(run("cargo deny", Program::Cargo, "", &["deny", "--workspace", "check"]));
    }
    steps
}

/// The fast local subset (`cargo xtask check`): fmt, clippy, nextest.
pub fn check_only() -> Vec<Step> {
    let mut steps = lint_steps();
    steps.push(run(
        "nextest",
        Program::Cargo,
        "",
        &["nextest", "run", "--workspace", "--features", NEXTEST_FEATURES],
    ));
    steps
}

/// Feature names declared by a `Cargo.toml`'s `[features]` table, `default` excluded.
pub fn declared_features(manifest: &str) -> Vec<String> {
    let Ok(value) = manifest.parse::<toml::Table>() else {
        return Vec::new();
    };
    let Some(table) = value.get("features").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    table.keys().filter(|k| k.as_str() != "default").cloned().collect()
}

/// Declared features that no step in `steps` builds, as `pkg/feature` strings.
///
/// A step covers every feature when it lints the whole workspace with `--all-features` and
/// `--all-targets`; otherwise only the features named in its own `--features` list count.
/// This is what stops a narrow feature list from silently dropping `drift-app/recording`
/// (M8-3) or `drift-app/macos-ui-tests` (M6-2, M7-3) out of the gate.
pub fn feature_lint_gaps(features: &[(String, String)], steps: &[Step]) -> Vec<String> {
    let covers_all =
        steps.iter().any(|s| s.is_cargo_with(&["clippy", "--workspace", "--all-targets", "--all-features"]));
    if covers_all {
        return Vec::new();
    }
    let mut explicit = BTreeSet::new();
    for step in steps {
        let Action::Run { program: Program::Cargo, args, .. } = &step.action else { continue };
        let lints = args.iter().any(|a| a == "clippy" || a == "check");
        // A step that does not build every target cannot vouch for a feature: the test binaries
        // that feature gates (M6-2's `tabs_ui`, M7-3's `overlays_ui`) would never be compiled.
        if !lints || !args.iter().any(|a| a == "--all-targets") {
            continue;
        }
        let package = package_of(args);
        for (i, arg) in args.iter().enumerate() {
            if arg != "--features" {
                continue;
            }
            let Some(list) = args.get(i + 1) else { continue };
            for feature in list.split(',') {
                match feature.split_once('/') {
                    Some((pkg, feat)) => {
                        explicit.insert(format!("{pkg}/{feat}"));
                    }
                    None => {
                        if let Some(pkg) = &package {
                            explicit.insert(format!("{pkg}/{feature}"));
                        }
                    }
                }
            }
        }
    }
    features.iter().map(|(pkg, feat)| format!("{pkg}/{feat}")).filter(|f| !explicit.contains(f)).collect()
}

/// The `-p <name>` argument of a cargo command line, if there is exactly one.
fn package_of(args: &[String]) -> Option<String> {
    let mut found = None;
    for (i, arg) in args.iter().enumerate() {
        if arg == "-p" || arg == "--package" {
            if found.is_some() {
                return None;
            }
            found = args.get(i + 1).cloned();
        }
    }
    found
}

/// Vendored IronRDP crates the root workspace depends on, from its `[workspace.dependencies]`
/// path entries under [`VENDORED_DIR`].
pub fn vendored_dependencies(root_manifest: &str) -> BTreeSet<String> {
    let prefix = format!("{VENDORED_DIR}/crates/");
    let Ok(table) = root_manifest.parse::<toml::Table>() else {
        return BTreeSet::new();
    };
    let Some(deps) =
        table.get("workspace").and_then(|w| w.get("dependencies")).and_then(toml::Value::as_table)
    else {
        return BTreeSet::new();
    };
    deps.iter()
        .filter_map(|(name, spec)| {
            let path = spec.get("path")?.as_str()?;
            path.strip_prefix(&prefix).map(|_| name.clone())
        })
        .collect()
}

/// Crate names a unified-diff patch touches, from its `+++ b/crates/<name>/…` headers.
pub fn patched_crates(patch: &str) -> BTreeSet<String> {
    patch
        .lines()
        .filter_map(|l| l.strip_prefix("+++ b/crates/").or_else(|| l.strip_prefix("--- a/crates/")))
        .filter_map(|rest| rest.split('/').next())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}
