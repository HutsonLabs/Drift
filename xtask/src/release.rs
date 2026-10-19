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
    /// Tag the commit and publish the two DMGs as a release on spaces.hutson.cloud (never
    /// GitHub, RELEASE-spaces).
    pub spaces: bool,
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
    /// Lockstep: every artifact embeds the same version and build number (read back from the
    /// files), so iOS and macOS never ship out of sync.
    SyncCheck,
    /// Upload the `.ipa` and `.pkg` (skipped when App Store Connect already has the build).
    Testflight,
    /// "What to Test" on both TestFlight builds.
    WhatsNew,
    /// Tag, push the tag to `origin`, create the release on spaces.hutson.cloud with both DMGs
    /// and `SHA256SUMS`.
    Spaces,
}

impl StepId {
    /// Whether the step talks to the network and is retried with backoff.
    pub fn retries(self) -> bool {
        matches!(self, Self::BuildNumber | Self::Notarize | Self::Testflight | Self::WhatsNew | Self::Spaces)
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
    if options.dry_run && (options.testflight || options.spaces) {
        bail!("--dry-run cannot be combined with --testflight or --spaces");
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
    steps.push(StepId::SyncCheck);
    if options.testflight {
        steps.extend([StepId::Testflight, StepId::WhatsNew]);
    }
    if options.spaces {
        steps.push(StepId::Spaces);
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
    /// TestFlight upload state per App Store platform (a resume uploads only what is missing).
    #[serde(default)]
    pub uploads: BTreeMap<Platform, UploadState>,
}

impl Manifest {
    /// Records the upload state of `platform`.
    pub fn record_upload(&mut self, platform: Platform, state: UploadState) {
        self.uploads.insert(platform, state);
    }

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
/// Preflight, the gate and the lockstep check always run; the build number and the clean step
/// are kept once done.
pub fn should_skip(step: StepId, manifest: &Manifest, hash_of: impl Fn(&Path) -> Option<String>) -> bool {
    if matches!(step, StepId::Preflight | StepId::Gate | StepId::SyncCheck) {
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

// ---- lockstep: iOS and macOS share one version and one build number ---------------------------

/// An App Store platform Drift ships on. Both always get the same version and build number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Platform {
    /// iPhone and iPad (`IOS` in App Store Connect).
    #[serde(rename = "IOS")]
    Ios,
    /// The Mac App Store (`MAC_OS`).
    #[serde(rename = "MAC_OS")]
    MacOs,
}

impl Platform {
    /// Both platforms, in upload order.
    pub const ALL: [Self; 2] = [Self::Ios, Self::MacOs];

    /// The App Store Connect name.
    pub fn asc_name(self) -> &'static str {
        match self {
            Self::Ios => "IOS",
            Self::MacOs => "MAC_OS",
        }
    }

    fn from_asc(name: &str) -> Option<Self> {
        match name {
            "IOS" => Some(Self::Ios),
            "MAC_OS" => Some(Self::MacOs),
            _ => None,
        }
    }
}

/// Where a platform's TestFlight upload stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UploadState {
    /// Uploaded by this release.
    Uploaded,
    /// App Store Connect already had this build (an earlier attempt), so nothing was sent.
    AlreadyPresent,
    /// The upload failed; `--resume` tries it again with the same build number.
    Failed,
}

/// `(platform, build number)` of every build in a `GET /v1/builds?include=preReleaseVersion`
/// response; builds on other platforms (visionOS) and non-numeric numbers are left out.
pub fn builds_by_platform(json: &str) -> Result<Vec<(Platform, u32)>> {
    let value: serde_json::Value =
        serde_json::from_str(json).context("App Store Connect sent invalid JSON")?;
    let platforms: BTreeMap<&str, Platform> = value["included"]
        .as_array()
        .map(|inc| {
            inc.iter()
                .filter(|i| i["type"] == "preReleaseVersions")
                .filter_map(|i| {
                    Some((i["id"].as_str()?, Platform::from_asc(i["attributes"]["platform"].as_str()?)?))
                })
                .collect()
        })
        .unwrap_or_default();
    let data = value["data"].as_array().context("the builds response has no data")?;
    Ok(data
        .iter()
        .filter_map(|b| {
            let pre = b["relationships"]["preReleaseVersion"]["data"]["id"].as_str()?;
            let number = b["attributes"]["version"].as_str()?.split('.').next()?.parse().ok()?;
            Some((*platforms.get(pre)?, number))
        })
        .collect())
}

/// The next release's build number: one above the highest on **any** platform.
pub fn next_build(builds: &[(Platform, u32)]) -> u32 {
    builds.iter().map(|(_, n)| *n).max().map_or(1, |n| n.saturating_add(1))
}

/// How the two platforms stand in App Store Connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AscSync {
    /// No builds yet.
    Empty,
    /// Both platforms' newest build is this number.
    InSync(u32),
    /// The newest number exists on one platform only: an interrupted release; `--resume` uploads
    /// the missing platform with the same number.
    Partial {
        /// The number.
        build: u32,
        /// The platform without it.
        missing: Platform,
    },
    /// The platforms' newest builds differ (and neither is a partial release of the other's
    /// number); the next release uses the highest + 1 on both, which brings them back in step.
    Diverged {
        /// iOS's newest.
        ios: u32,
        /// macOS's newest.
        macos: u32,
    },
}

/// Classifies App Store Connect's builds.
pub fn asc_sync(builds: &[(Platform, u32)]) -> AscSync {
    let newest = |p: Platform| builds.iter().filter(|(q, _)| *q == p).map(|(_, n)| *n).max();
    let has = |p: Platform, n: u32| builds.iter().any(|(q, m)| *q == p && *m == n);
    match (newest(Platform::Ios), newest(Platform::MacOs)) {
        (None, None) => AscSync::Empty,
        (Some(i), Some(m)) if i == m => AscSync::InSync(i),
        (Some(i), m)
            if m.is_none_or(|m| i > m)
                && !has(Platform::MacOs, i)
                && m.is_none_or(|m| has(Platform::Ios, m)) =>
        {
            AscSync::Partial { build: i, missing: Platform::MacOs }
        }
        (i, Some(m))
            if i.is_none_or(|i| m > i)
                && !has(Platform::Ios, m)
                && i.is_none_or(|i| has(Platform::MacOs, i)) =>
        {
            AscSync::Partial { build: m, missing: Platform::Ios }
        }
        (i, m) => AscSync::Diverged { ios: i.unwrap_or(0), macos: m.unwrap_or(0) },
    }
}

/// Lockstep check: every artifact's embedded `(CFBundleShortVersionString, CFBundleVersion)`
/// equals `(version, build)`, and all four are present: both App Store platforms, and Drift's
/// and Drift Host's DMGs. Drift and Drift Host always ship together, whether or not either
/// changed since the last release (RELEASE-lockstep).
pub fn check_sync(
    found: &[(ArtifactKind, Option<(String, String)>)],
    version: &str,
    build: u32,
) -> Result<()> {
    let want = (version.to_owned(), build.to_string());
    let mut wrong = Vec::new();
    for kind in
        [ArtifactKind::IosIpa, ArtifactKind::MacAppStorePkg, ArtifactKind::MacDmg, ArtifactKind::HostDmg]
    {
        if !found.iter().any(|(k, _)| *k == kind) {
            wrong.push(format!("{kind:?} is missing"));
        }
    }
    for (kind, embedded) in found {
        match embedded {
            Some(v) if *v == want => {}
            Some((v, b)) => wrong.push(format!("{kind:?} embeds {v} ({b}), not {version} ({build})")),
            None => wrong.push(format!("{kind:?}: cannot read its Info.plist")),
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        bail!(
            "the release is out of step (iOS, Mac App Store, Drift and Drift Host ship together): {}",
            wrong.join("; ")
        )
    }
}

/// The platforms still to upload, given whether App Store Connect has the build on iOS and on
/// macOS.
pub fn to_upload(ios_present: bool, macos_present: bool) -> Vec<Platform> {
    Platform::ALL
        .into_iter()
        .filter(|p| match p {
            Platform::Ios => !ios_present,
            Platform::MacOs => !macos_present,
        })
        .collect()
}

// ---- a clean tree after every build -------------------------------------------------------------

/// Tracked paths the Xcode build rewrites (Tauri writes the build number into the iOS
/// `Info.plist`, xcodegen re-quotes `project.pbxproj`); the pipeline restores them after each build
/// step so the tree stays clean and `--resume` can run.
pub const RESTORED_AFTER_BUILD: &[&str] = &["src-tauri/gen/apple"];

/// Paths in `git status --porcelain` output outside [`RESTORED_AFTER_BUILD`]: changes a build
/// step must not make.
pub fn unexpected_changes(porcelain: &str) -> Vec<String> {
    porcelain
        .lines()
        .filter_map(|l| l.get(3..))
        .map(|p| p.trim().trim_matches('"').to_owned())
        .filter(|p| !RESTORED_AFTER_BUILD.iter().any(|r| p == r || p.starts_with(&format!("{r}/"))))
        .collect()
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

/// Release notes: the notes plus the checksums.
pub fn release_notes(notes: &str, sums: &str) -> String {
    format!("{}\n\n## SHA-256\n\n```text\n{sums}```\n", notes.trim_end())
}

// ---- publishing on spaces.hutson.cloud (RELEASE-spaces) -------------------------------------

/// The Forgejo API releases are published through. Never GitHub.
pub const SPACES_API: &str = "https://spaces.hutson.cloud/api/v1";
/// The repository on spaces.hutson.cloud.
pub const SPACES_REPO: &str = "HutsonLabs/drift";
/// The environment variable holding the spaces.hutson.cloud API token.
pub const SPACES_TOKEN_VAR: &str = "FORGEJO_API_KEY";

/// The body of `POST /repos/{SPACES_REPO}/releases` for release `version` (`build`) of commit
/// `commit`.
pub fn spaces_release(version: &str, build: u32, commit: &str, notes: &str, sums: &str) -> serde_json::Value {
    serde_json::json!({
        "tag_name": tag(version, build),
        "target_commitish": commit,
        "name": format!("Drift {version} ({build})"),
        "body": release_notes(notes, sums),
        "draft": false,
        "prerelease": false,
    })
}

/// The files attached to a published release: both DMGs (always together) and the checksums.
pub fn published_files(version: &str, build: u32) -> Vec<String> {
    vec![
        artifact_name(ArtifactKind::MacDmg, version, build),
        artifact_name(ArtifactKind::HostDmg, version, build),
        "SHA256SUMS".to_owned(),
    ]
}

/// Whether a finished run can be published later without rebuilding (`--publish`): not a dry
/// run, and notarization, the checksums and the lockstep check all ended done (their latest
/// records).
pub fn publishable(manifest: &Manifest) -> Result<()> {
    if manifest.dry_run {
        bail!("{} ({}) was a dry run; only a real build can be published", manifest.version, manifest.build);
    }
    for step in [StepId::Notarize, StepId::Checksums, StepId::SyncCheck] {
        let last = manifest.steps.iter().rev().find(|r| r.step == step).map(|r| r.status);
        if !matches!(last, Some(StepStatus::Done | StepStatus::Skipped)) {
            bail!(
                "{step:?} did not finish for {} ({}); resume the release first",
                manifest.version,
                manifest.build
            );
        }
    }
    Ok(())
}
