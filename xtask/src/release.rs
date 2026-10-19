//! `cargo xtask release` (ADR `RELEASE-pipeline`): one pipeline from a clean `main` to every
//! release artifact, with named steps, a manifest, resume and explicit publish flags.
//!
//! This module holds the decisions as data (version files, the step plan, resume rules, the
//! manifest, names, notes and checksums) and is tested in `xtask/tests/release.rs`; `main.rs`
//! runs the steps.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::testflight::is_marketing_version;

// ---- version files -----------------------------------------------------------------------------

/// How a file stores its version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The first line that starts with `version = "…"` (the `[workspace.package]` version).
    CargoWorkspace,
    /// The first `"version": "…"` in a JSON file.
    JsonVersion,
    /// The `<string>` after `<key>CFBundleShortVersionString</key>` in a property list.
    PlistShort,
    /// `CFBundleShortVersionString: "…"` in xcodegen's `project.yml`.
    YamlShort,
}

/// A file that carries Drift's version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionFile {
    /// Repository-relative path.
    pub path: &'static str,
    /// Where the version sits in it.
    pub format: Format,
    /// `yy.mm.0` (Cargo, Tauri, bun need three parts) rather than the `yy.mm` marketing version.
    pub semver: bool,
}

/// Every file that carries the version. `xtask release --set-version` rewrites all of them.
pub const VERSION_FILES: &[VersionFile] = &[
    VersionFile { path: "Cargo.toml", format: Format::CargoWorkspace, semver: true },
    VersionFile { path: "src-tauri/tauri.conf.json", format: Format::JsonVersion, semver: true },
    VersionFile { path: "ui/package.json", format: Format::JsonVersion, semver: true },
    VersionFile { path: "src-tauri/Info.plist", format: Format::PlistShort, semver: false },
    VersionFile { path: "crates/drift-host-app/Info.plist", format: Format::PlistShort, semver: false },
    VersionFile {
        path: "src-tauri/gen/apple/drift-app_iOS/Info.plist",
        format: Format::PlistShort,
        semver: false,
    },
    VersionFile { path: "src-tauri/gen/apple/project.yml", format: Format::YamlShort, semver: false },
];

/// Byte range of the version value in `text`.
fn locate(text: &str, format: Format) -> Option<(usize, usize)> {
    let after = |start: usize, close: &str| text[start..].find(close).map(|end| (start, start + end));
    match format {
        Format::CargoWorkspace => {
            let mut offset = 0;
            for line in text.split_inclusive('\n') {
                if let Some(rest) = line.strip_prefix("version = \"") {
                    let start = offset + line.len() - rest.len();
                    return after(start, "\"");
                }
                offset += line.len();
            }
            None
        }
        Format::JsonVersion => {
            let key = "\"version\": \"";
            text.find(key).and_then(|i| after(i + key.len(), "\""))
        }
        Format::PlistShort => {
            let key = text.find("<key>CFBundleShortVersionString</key>")?;
            let open = text[key..].find("<string>")? + key + "<string>".len();
            after(open, "</string>")
        }
        Format::YamlShort => {
            let key = "CFBundleShortVersionString: \"";
            text.find(key).and_then(|i| after(i + key.len(), "\""))
        }
    }
}

/// The version stored in `text`.
pub fn read_version(text: &str, format: Format) -> Option<String> {
    locate(text, format).map(|(a, b)| text[a..b].to_owned())
}

/// `text` with its version replaced by `value`.
pub fn write_version(text: &str, format: Format, value: &str) -> Option<String> {
    let (a, b) = locate(text, format)?;
    Some(format!("{}{value}{}", &text[..a], &text[b..]))
}

/// The version of every [`VERSION_FILES`] entry in the repository at `root`.
pub fn read_versions(root: &Path) -> Result<Vec<(VersionFile, Option<String>)>> {
    VERSION_FILES
        .iter()
        .map(|f| {
            let text =
                std::fs::read_to_string(root.join(f.path)).with_context(|| format!("reading {}", f.path))?;
            Ok((*f, read_version(&text, f.format)))
        })
        .collect()
}

/// The marketing version all files agree on, or an error naming every file that disagrees.
pub fn check_versions(found: &[(VersionFile, Option<String>)]) -> Result<String> {
    let marketing = found
        .iter()
        .find_map(|(f, v)| v.as_deref().map(|v| if f.semver { v.strip_suffix(".0").unwrap_or(v) } else { v }))
        .context("no version file has a version")?
        .to_owned();
    if !is_marketing_version(&marketing) {
        bail!("the version {marketing} is not yy.mm");
    }
    let mut wrong = Vec::new();
    for (f, v) in found {
        let want = if f.semver { semver(&marketing) } else { marketing.clone() };
        match v {
            Some(v) if *v == want => {}
            Some(v) => wrong.push(format!("{} has {v} (expected {want})", f.path)),
            None => wrong.push(format!("{} has no version", f.path)),
        }
    }
    if !wrong.is_empty() {
        bail!("version files disagree: {}", wrong.join("; "));
    }
    Ok(marketing)
}

/// `yy.mm` → `yy.mm.0`.
pub fn semver(marketing: &str) -> String {
    format!("{marketing}.0")
}

/// Every file's text with the version set to `marketing` (`yy.mm`).
pub fn set_version(files: &[(VersionFile, String)], marketing: &str) -> Result<Vec<(&'static str, String)>> {
    if !is_marketing_version(marketing) {
        bail!("{marketing} is not yy.mm");
    }
    files
        .iter()
        .map(|(f, text)| {
            let value = if f.semver { semver(marketing) } else { marketing.to_owned() };
            let out = write_version(text, f.format, &value)
                .with_context(|| format!("{} has no version", f.path))?;
            Ok((f.path, out))
        })
        .collect()
}

// ---- the plan ----------------------------------------------------------------------------------

/// What the user asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Build and validate everything, but never notarize, upload or publish.
    pub dry_run: bool,
    /// Upload the iOS and Mac App Store builds to TestFlight and set "What to Test".
    pub testflight: bool,
    /// Tag the commit and publish the two DMGs as a GitHub release.
    pub github: bool,
}

/// One named step of the pipeline, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepId {
    /// Branch, tree, toolchain, credentials, identities, keychain, profile, disk, versions, notes.
    Preflight,
    /// `cargo xtask ci` passed on HEAD (receipt), or run it.
    Gate,
    /// App Store Connect's highest build number + 1, chosen once per release.
    BuildNumber,
    /// Remove earlier bundle outputs.
    Clean,
    /// The App Store `.ipa`, validated by `altool`.
    Ios,
    /// The Mac App Store `.pkg`, validated by `altool`.
    MacAppStore,
    /// Drift for Mac, Developer ID, in a signed DMG.
    MacDmg,
    /// Drift Host, Developer ID, in a signed DMG.
    HostDmg,
    /// Notarize and staple both DMGs.
    Notarize,
    /// `SHA256SUMS` over the artifacts.
    Checksums,
    /// Upload the `.ipa` and `.pkg` (skipped when App Store Connect already has the build).
    Testflight,
    /// "What to Test" on both TestFlight builds.
    WhatsNew,
    /// Tag, push the tag, `gh release create`.
    Github,
}

impl StepId {
    /// Whether the step talks to the network and is retried with backoff.
    pub fn retries(self) -> bool {
        matches!(self, Self::BuildNumber | Self::Notarize | Self::Testflight | Self::WhatsNew | Self::Github)
    }

    /// How long one attempt may take before it is stopped.
    pub fn timeout(self) -> Duration {
        Duration::from_secs(match self {
            Self::Notarize => 3600,
            Self::Testflight | Self::WhatsNew => 3600,
            Self::Ios | Self::MacAppStore | Self::MacDmg | Self::HostDmg => 3 * 3600,
            Self::Gate => 3 * 3600,
            _ => 1800,
        })
    }
}

/// The steps for `options`, in order. A dry run cannot publish.
pub fn steps(options: Options) -> Result<Vec<StepId>> {
    if options.dry_run && (options.testflight || options.github) {
        bail!("--dry-run cannot be combined with --testflight or --github");
    }
    let mut steps = vec![
        StepId::Preflight,
        StepId::Gate,
        StepId::BuildNumber,
        StepId::Clean,
        StepId::Ios,
        StepId::MacAppStore,
        StepId::MacDmg,
        StepId::HostDmg,
    ];
    if !options.dry_run {
        steps.push(StepId::Notarize);
    }
    steps.push(StepId::Checksums);
    if options.testflight {
        steps.extend([StepId::Testflight, StepId::WhatsNew]);
    }
    if options.github {
        steps.push(StepId::Github);
    }
    Ok(steps)
}

/// The wait before retry `attempt` (0-based): 10 s, 30 s, then 90 s.
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0 => 10,
        1 => 30,
        _ => 90,
    })
}

/// Attempts per retried step.
pub const ATTEMPTS: u32 = 3;

// ---- the manifest ------------------------------------------------------------------------------

/// What an artifact is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    /// The App Store `.ipa`.
    IosIpa,
    /// The Mac App Store installer package.
    MacAppStorePkg,
    /// Drift for Mac, Developer ID DMG.
    MacDmg,
    /// Drift Host, Developer ID DMG.
    HostDmg,
    /// `SHA256SUMS`.
    Checksums,
}

/// One file in the output directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// What it is.
    pub kind: ArtifactKind,
    /// File name inside the output directory.
    pub file: String,
    /// Lower-case hex SHA-256.
    pub sha256: String,
    /// Bytes.
    pub size: u64,
}

/// How a step ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepStatus {
    /// Finished.
    Done,
    /// Failed; `error` says why.
    Failed,
    /// Skipped on resume (outputs intact) or because publishing found it already done.
    Skipped,
}

/// One step's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRecord {
    /// Which step.
    pub step: StepId,
    /// How it ended.
    pub status: StepStatus,
    /// Unix seconds.
    pub started: u64,
    /// Unix seconds.
    pub finished: Option<u64>,
    /// Files it produced.
    pub outputs: Vec<Artifact>,
    /// Why it failed.
    pub error: Option<String>,
}

/// `manifest.json` in the output directory: what was built from what, and what ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Marketing version (`yy.mm`).
    pub version: String,
    /// Build number shared by every artifact.
    pub build: u32,
    /// The commit.
    pub git_sha: String,
    /// The commit time, used as `SOURCE_DATE_EPOCH`.
    pub source_date_epoch: u64,
    /// `rustc --version`.
    pub toolchain: String,
    /// Lockfile → SHA-256.
    pub lockfiles: BTreeMap<String, String>,
    /// Whether this was a dry run.
    pub dry_run: bool,
    /// Step records in the order they last ran.
    pub steps: Vec<StepRecord>,
    /// Final artifacts.
    pub artifacts: Vec<Artifact>,
}

impl Manifest {
    fn record(&mut self, record: StepRecord) {
        self.steps.retain(|r| r.step != record.step);
        self.steps.push(record);
    }

    /// Records a finished step.
    pub fn done(&mut self, step: StepId, started: u64, finished: u64, outputs: Vec<Artifact>) {
        self.record(StepRecord {
            step,
            status: StepStatus::Done,
            started,
            finished: Some(finished),
            outputs,
            error: None,
        });
    }

    /// Records a skipped step, keeping the outputs of its earlier run.
    pub fn skipped(&mut self, step: StepId, at: u64) {
        let outputs = self.outputs_of(step);
        self.record(StepRecord {
            step,
            status: StepStatus::Skipped,
            started: at,
            finished: Some(at),
            outputs,
            error: None,
        });
    }

    /// Records a failed step.
    pub fn fail(&mut self, step: StepId, at: u64, error: &str) {
        self.record(StepRecord {
            step,
            status: StepStatus::Failed,
            started: at,
            finished: Some(at),
            outputs: Vec::new(),
            error: Some(error.to_owned()),
        });
    }

    /// The outputs recorded for `step`.
    pub fn outputs_of(&self, step: StepId) -> Vec<Artifact> {
        self.steps.iter().find(|r| r.step == step).map(|r| r.outputs.clone()).unwrap_or_default()
    }

    /// Pretty JSON.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)? + "\n")
    }

    /// Parses `manifest.json`.
    pub fn from_json(text: &str) -> Result<Self> {
        serde_json::from_str(text).context("manifest.json is not a release manifest")
    }
}

/// Whether `--resume` may skip `step`: it finished (or was skipped) before and every output it
/// recorded still has the recorded hash (`hash_of` gets the file name; `None` = missing).
/// Preflight and the gate always run; the build number and the clean step are kept once done.
pub fn should_skip(step: StepId, manifest: &Manifest, hash_of: impl Fn(&Path) -> Option<String>) -> bool {
    if matches!(step, StepId::Preflight | StepId::Gate) {
        return false;
    }
    let Some(record) = manifest.steps.iter().find(|r| r.step == step) else { return false };
    if !matches!(record.status, StepStatus::Done | StepStatus::Skipped) {
        return false;
    }
    record.outputs.iter().all(|a| hash_of(Path::new(&a.file)).as_deref() == Some(a.sha256.as_str()))
}

/// Whether an earlier manifest belongs to the same release (version and commit).
pub fn resumable(manifest: &Manifest, version: &str, git_sha: &str) -> Result<()> {
    if manifest.version != version {
        bail!("the earlier run is for {}, not {version}", manifest.version);
    }
    if manifest.git_sha != git_sha {
        bail!("the earlier run built commit {}, not {git_sha}; start a new release", manifest.git_sha);
    }
    Ok(())
}

// ---- names, notes, checksums -------------------------------------------------------------------

/// `target/release-out/<yy.mm>-<build>`.
pub fn out_dir(root: &Path, version: &str, build: u32) -> PathBuf {
    root.join("target/release-out").join(format!("{version}-{build}"))
}

/// The file name of an artifact.
pub fn artifact_name(kind: ArtifactKind, version: &str, build: u32) -> String {
    match kind {
        ArtifactKind::IosIpa => format!("Drift_{version}.{build}_iOS.ipa"),
        ArtifactKind::MacAppStorePkg => format!("Drift_{version}.{build}_MacAppStore.pkg"),
        ArtifactKind::MacDmg => format!("Drift_{version}.{build}_aarch64.dmg"),
        ArtifactKind::HostDmg => format!("Drift-Host_{version}.{build}_aarch64.dmg"),
        ArtifactKind::Checksums => "SHA256SUMS".to_owned(),
    }
}

/// The git tag of a release: `v<yy.mm>.<build>` (three parts, like the earlier `v0.2.0`).
pub fn tag(version: &str, build: u32) -> String {
    format!("v{version}.{build}")
}

/// Where `cargo xtask ci` leaves its receipt for commit `sha` (written only from a clean tree).
pub fn receipt_path(root: &Path, sha: &str) -> PathBuf {
    root.join("target/ci-receipts").join(sha)
}

/// `release/notes/<yy.mm>.md`.
pub fn notes_path(root: &Path, version: &str) -> PathBuf {
    root.join("release/notes").join(format!("{version}.md"))
}

/// The template `release/notes/TEMPLATE.md` (kept in sync by a test).
pub const NOTES_TEMPLATE: &str = include_str!("../../release/notes/TEMPLATE.md");

/// Notes must exist, have a title and text, and not be the untouched template.
pub fn validate_notes(text: &str) -> Result<()> {
    if text.trim().is_empty() {
        bail!("the release notes are empty");
    }
    if text.contains("TODO") || text.trim() == NOTES_TEMPLATE.trim() {
        bail!("the release notes still contain the template's TODOs");
    }
    if !text.trim_start().starts_with("# ") {
        bail!("the release notes must start with a `# ` title");
    }
    if text.lines().skip(1).all(|l| l.trim().is_empty()) {
        bail!("the release notes have a title but no text");
    }
    Ok(())
}

/// TestFlight's "What to Test": the `## What to Test` section, else everything after the title;
/// at most 4000 characters.
pub fn whats_new(notes: &str) -> String {
    let body = match notes.find("## What to Test") {
        Some(i) => {
            let rest = &notes[i + "## What to Test".len()..];
            rest.find("\n## ").map_or(rest, |end| &rest[..end])
        }
        None => notes.split_once('\n').map_or("", |(_, rest)| rest),
    };
    body.trim().chars().take(4000).collect()
}

/// `SHA256SUMS` lines (`<hash>  <file>`), sorted by file name.
pub fn sha256sums(files: &[(String, String)]) -> String {
    let mut files = files.to_vec();
    files.sort();
    files.iter().map(|(name, hash)| format!("{hash}  {name}\n")).collect()
}

/// GitHub release notes: the notes plus the checksums.
pub fn github_notes(notes: &str, sums: &str) -> String {
    format!("{}\n\n## SHA-256\n\n```text\n{sums}```\n", notes.trim_end())
}
