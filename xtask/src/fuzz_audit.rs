//! Audit of the `cargo fuzz` targets the plan requires (M1-2, M9-2).
//!
//! Plan M9-2 asks for "nightly fuzzing of the GFX, ZGFX, redirection, RDSTLS, CLIPRDR and
//! pointer parsers". A fuzz target is invisible to `cargo xtask ci`: the fuzz crates are
//! standalone workspaces (nightly-only sanitizer builds), so nothing in the merge gate would
//! notice a target that was deleted, renamed, or never added to `fuzz-nightly.yml`. This audit
//! closes that hole by checking three things per required parser:
//!
//! 1. the fuzz workspace's `Cargo.toml` declares a `[[bin]]` with that name and path;
//! 2. `fuzz_targets/<name>.rs` exists;
//! 3. `.github/workflows/fuzz-nightly.yml` runs it (its directory and name are in the matrix)
//!    for at least [`MIN_SECONDS`].
//!
//! The checks are textual and pure, the same trade-off as [`crate::workflows`].

use std::path::Path;

use anyhow::{Context as _, Result};

/// Minimum `-max_total_time` the nightly workflow must give each target, in seconds.
///
/// Plan M1-2 asks for five minutes nightly; the package brief asks for at least two minutes
/// per target locally. The gate enforces the smaller of the two so a tuned-down run still
/// counts as fuzzing.
pub const MIN_SECONDS: u32 = 120;

/// The nightly fuzzing workflow (inside `.github/workflows/`).
pub const WORKFLOW: &str = "fuzz-nightly.yml";

/// One parser the plan wants fuzzed, and the target that does it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Required {
    /// The parser, as the plan names it (used in messages).
    pub parser: &'static str,
    /// Fuzz workspace directory, repository-relative.
    pub dir: &'static str,
    /// `cargo fuzz` target name (also the file stem in `fuzz_targets/`).
    pub target: &'static str,
}

/// The parsers plan M9-2 requires fuzzing, in plan order.
pub const REQUIRED: &[Required] = &[
    Required { parser: "GFX", dir: "crates/drift-gfx/fuzz", target: "gfx_pdu_zgfx" },
    Required { parser: "ZGFX", dir: "crates/drift-gfx/fuzz", target: "zgfx" },
    Required { parser: "redirection", dir: "crates/drift-rdp/fuzz", target: "redirection" },
    Required { parser: "RDSTLS", dir: "crates/drift-rdp/fuzz", target: "rdstls" },
    Required { parser: "CLIPRDR", dir: "crates/drift-rdp/fuzz", target: "cliprdr" },
    Required { parser: "pointer", dir: "crates/drift-macos/fuzz", target: "pointer" },
];

/// A fuzz workspace as read from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzCrate {
    /// Repository-relative directory, e.g. `crates/drift-gfx/fuzz`.
    pub dir: String,
    /// Its `Cargo.toml`.
    pub manifest: String,
    /// File stems found in `fuzz_targets/`, e.g. `["zgfx"]`.
    pub targets: Vec<String>,
}

/// Something a required fuzz target does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The target's name.
    pub target: String,
    /// What is missing, and which parser it covers.
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.target, self.message)
    }
}

/// Audits fuzz workspaces and the nightly workflow against [`REQUIRED`]. Pure.
pub fn audit(crates: &[FuzzCrate], workflow: &str) -> Vec<Problem> {
    let mut problems = Vec::new();
    for req in REQUIRED {
        let problem = |message: String| Problem { target: req.target.to_owned(), message };
        let Some(krate) = crates.iter().find(|c| c.dir == req.dir) else {
            problems.push(problem(format!("no fuzz workspace at {} ({} parser)", req.dir, req.parser)));
            continue;
        };
        if !declares_bin(&krate.manifest, req.target) {
            problems.push(problem(format!(
                "{}/Cargo.toml declares no [[bin]] `{}` with path fuzz_targets/{}.rs",
                req.dir, req.target, req.target
            )));
        }
        if !krate.targets.iter().any(|t| t == req.target) {
            problems.push(problem(format!("{}/fuzz_targets/{}.rs is missing", req.dir, req.target)));
        }
        if !workflow.contains(req.dir) || !workflow.contains(req.target) {
            problems.push(problem(format!(
                "{WORKFLOW} does not run it ({} parser, plan M9-2); add `fuzz_dir: {}` + `target: {}`",
                req.parser, req.dir, req.target
            )));
        }
    }
    if let Some(seconds) = max_total_time(workflow) {
        if seconds < MIN_SECONDS {
            problems.push(Problem {
                target: WORKFLOW.to_owned(),
                message: format!("-max_total_time={seconds} is below the {MIN_SECONDS}s minimum"),
            });
        }
    } else {
        problems.push(Problem {
            target: WORKFLOW.to_owned(),
            message: format!("no `-max_total_time=` (each target runs ≥ {MIN_SECONDS}s, plan M9-2)"),
        });
    }
    problems
}

/// Whether `manifest` declares `[[bin]] name = "<target>"` with the conventional path.
fn declares_bin(manifest: &str, target: &str) -> bool {
    let Ok(table) = manifest.parse::<toml::Table>() else { return false };
    let Some(bins) = table.get("bin").and_then(toml::Value::as_array) else { return false };
    bins.iter().any(|bin| {
        bin.get("name").and_then(toml::Value::as_str) == Some(target)
            && bin.get("path").and_then(toml::Value::as_str)
                == Some(format!("fuzz_targets/{target}.rs").as_str())
    })
}

/// The smallest `-max_total_time=<n>` in `workflow`, if any.
fn max_total_time(workflow: &str) -> Option<u32> {
    workflow
        .match_indices("-max_total_time=")
        .filter_map(|(at, needle)| {
            let rest = &workflow[at + needle.len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        })
        .min()
}

/// Reads the fuzz workspaces named by [`REQUIRED`] from `root`.
///
/// A directory that does not exist is simply absent from the result; [`audit`] reports it.
pub fn read_crates(root: &Path) -> Result<Vec<FuzzCrate>> {
    let mut dirs: Vec<&str> = REQUIRED.iter().map(|r| r.dir).collect();
    dirs.sort_unstable();
    dirs.dedup();
    let mut crates = Vec::new();
    for dir in dirs {
        let path = root.join(dir);
        let Ok(manifest) = std::fs::read_to_string(path.join("Cargo.toml")) else { continue };
        let mut targets = Vec::new();
        if let Ok(entries) = std::fs::read_dir(path.join("fuzz_targets")) {
            for entry in entries {
                let file = entry.with_context(|| format!("reading {dir}/fuzz_targets"))?.path();
                if file.extension().is_some_and(|e| e == "rs")
                    && let Some(stem) = file.file_stem()
                {
                    targets.push(stem.to_string_lossy().into_owned());
                }
            }
        }
        targets.sort();
        crates.push(FuzzCrate { dir: dir.to_owned(), manifest, targets });
    }
    Ok(crates)
}

/// Reads the fuzz workspaces and the nightly workflow from `root` and audits them.
pub fn audit_repo(root: &Path) -> Result<Vec<Problem>> {
    let crates = read_crates(root)?;
    let workflow = std::fs::read_to_string(root.join(".github/workflows").join(WORKFLOW))
        .unwrap_or_else(|_| String::new());
    Ok(audit(&crates, &workflow))
}
