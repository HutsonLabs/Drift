//! `cargo xtask release` runner (ADR `RELEASE-pipeline`). The decisions live in
//! `xtask::release`; this module runs the steps, keeps the manifest and talks to the tools.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use sha2::{Digest as _, Sha256};
use xtask::release::{self, Artifact, ArtifactKind, Manifest, Options, Platform, StepId};
use xtask::{store, testflight};

use super::{asc_get, asc_key, asc_request, bun, cargo, checked, mas_package_build};

/// The Mac App Store provisioning profile, where STORE-1 keeps it.
const DEFAULT_PROFILE: &str =
    "Library/MobileDevice/Provisioning Profiles/Drift_Mac_App_Store.provisionprofile";
/// Free disk the builds need (bytes).
const MIN_FREE_DISK: u64 = 40 * 1024 * 1024 * 1024;
/// A profile must stay valid at least this long after the release.
const PROFILE_MARGIN: Duration = Duration::from_secs(14 * 24 * 3600);

/// Command-line options of `cargo xtask release`.
#[derive(Debug, Clone, Default)]
pub struct Args {
    /// What to run.
    pub options: Options,
    /// Continue the last run of this version and commit, skipping finished steps.
    pub resume: bool,
    /// Use this build number instead of App Store Connect's highest + 1.
    pub build_number: Option<u32>,
    /// The Mac App Store provisioning profile.
    pub profile: Option<PathBuf>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).current_dir(root).output().context("running git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn git_porcelain(root: &Path) -> Result<String> {
    let out = Command::new("git").args(["status", "--porcelain"]).current_dir(root).output()?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn tree_is_clean(root: &Path) -> Result<bool> {
    Ok(git(root, &["status", "--porcelain"])?.is_empty())
}

/// Writes `target/ci-receipts/<HEAD>` after a passing full gate from a clean tree.
pub fn write_ci_receipt(root: &Path) -> Result<()> {
    if !tree_is_clean(root)? {
        eprintln!("xtask: the tree has changes; no release receipt written");
        return Ok(());
    }
    let sha = git(root, &["rev-parse", "HEAD"])?;
    let path = release::receipt_path(root, &sha);
    std::fs::create_dir_all(path.parent().context("receipt dir")?)?;
    std::fs::write(&path, format!("cargo xtask ci passed at {}\n", now()))?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<(String, u64)> {
    let mut file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let size = std::io::copy(&mut file, &mut hasher)?;
    Ok((format!("{:x}", hasher.finalize()), size))
}

/// Copies `from` into the output directory as the artifact of `kind` and describes it.
fn collect(from: &Path, out: &Path, kind: ArtifactKind, version: &str, build: u32) -> Result<Artifact> {
    let name = release::artifact_name(kind, version, build);
    let to = out.join(&name);
    std::fs::copy(from, &to).with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
    let (sha256, size) = sha256_file(&to)?;
    Ok(Artifact { kind, file: name, sha256, size })
}

/// Writes `manifest.json` atomically (temp file + rename).
fn save(out: &Path, manifest: &Manifest) -> Result<()> {
    let tmp = out.join("manifest.json.tmp");
    std::fs::write(&tmp, manifest.to_json()?)?;
    std::fs::rename(&tmp, out.join("manifest.json"))?;
    Ok(())
}

fn log(out: Option<&Path>, line: &str) {
    println!("{line}");
    if let Some(out) = out {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(out.join("release.log"))
        {
            let _ = writeln!(f, "{} {line}", now());
        }
    }
}

/// Runs `cmd` and kills it after `timeout`.
fn run_timeout(cmd: &mut Command, what: &str, timeout: Duration) -> Result<()> {
    let mut child = cmd.spawn().with_context(|| format!("running {what}"))?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            bail!("{what} failed ({status})");
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{what} timed out after {} s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Runs `f`, retrying network steps with backoff.
fn with_retries<T>(step: StepId, mut f: impl FnMut() -> Result<T>) -> Result<T> {
    let attempts = if step.retries() { release::ATTEMPTS } else { 1 };
    let mut attempt = 0;
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if attempt + 1 < attempts => {
                let wait = release::backoff(attempt);
                eprintln!(
                    "xtask: {step:?} attempt {} failed: {e:#}; retrying in {} s",
                    attempt + 1,
                    wait.as_secs()
                );
                std::thread::sleep(wait);
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

// ---- the deterministic environment -----------------------------------------------------------------

/// Sets the build environment every step inherits: commit time as `SOURCE_DATE_EPOCH`, no
/// incremental compilation, and source paths remapped so the checkout and cargo home do not
/// leak into binaries.
fn deterministic_env(root: &Path, epoch: u64) {
    let cargo_home = std::env::var("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cargo"));
    let rustflags = format!(
        "--remap-path-prefix={}=. --remap-path-prefix={}=/cargo",
        root.display(),
        cargo_home.display()
    );
    // SAFETY: called once at the start of `cargo xtask release`, before any thread is spawned.
    unsafe {
        std::env::set_var("SOURCE_DATE_EPOCH", epoch.to_string());
        std::env::set_var("CARGO_INCREMENTAL", "0");
        std::env::set_var("RUSTFLAGS", rustflags);
    }
}

// ---- preflight -------------------------------------------------------------------------------------

fn profile_path(args: &Args) -> PathBuf {
    args.profile
        .clone()
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(DEFAULT_PROFILE))
}

fn free_disk(path: &Path) -> Result<u64> {
    let out = Command::new("df").arg("-k").arg(path).output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().nth(1).context("df printed nothing")?;
    let kb: u64 = line.split_whitespace().nth(3).context("df column")?.parse()?;
    Ok(kb * 1024)
}

/// Every problem that stops a release, collected rather than stopping at the first.
fn preflight(root: &Path, args: &Args, version: &str) -> Result<()> {
    let opts = args.options;
    let mut problems = Vec::new();
    let mut warn = |strict: bool, msg: String| {
        if strict {
            problems.push(msg);
        } else {
            eprintln!("xtask: warning (dry run): {msg}");
        }
    };
    let strict = !opts.dry_run;
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch != "main" {
        warn(strict, format!("on branch {branch}, not main"));
    }
    let head = git(root, &["rev-parse", "HEAD"])?;
    let pushed = git(root, &["branch", "-r", "--contains", &head]).unwrap_or_default();
    if !pushed.lines().any(|l| l.trim().starts_with("origin/")) {
        warn(strict, "HEAD is not on origin yet".to_owned());
    }
    if !tree_is_clean(root)? {
        problems.push("the working tree has changes".to_owned());
    }
    // Toolchain.
    let wanted = std::fs::read_to_string(root.join("rust-toolchain.toml"))?
        .parse::<toml::Table>()
        .ok()
        .and_then(|t| t["toolchain"]["channel"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let rustc = Command::new("rustc").arg("--version").current_dir(root).output()?;
    if !String::from_utf8_lossy(&rustc.stdout).contains(&wanted) {
        problems.push(format!("rustc is not the pinned {wanted}"));
    }
    if !Command::new(cargo())
        .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()?
        .status
        .success()
    {
        problems.push("Cargo.lock is out of date (cargo metadata --locked)".to_owned());
    }
    // Credentials (names only).
    for name in ["APPLE_API_KEY_PATH", "APPLE_KEY_ID", "APPLE_API_ISSUER"] {
        if std::env::var(name).map_or(true, |v| v.is_empty()) {
            problems.push(format!("{name} is not set (set -a; . ~/code/term.hut/.env; set +a)"));
        }
    }
    if let Ok(key) = std::env::var("APPLE_API_KEY_PATH")
        && !Path::new(&key).is_file()
    {
        problems.push("APPLE_API_KEY_PATH does not name a file".to_owned());
    }
    // Signing identities and the keychain.
    let ids = Command::new("security").args(["find-identity", "-v", "-p", "codesigning"]).output()?;
    let ids = String::from_utf8_lossy(&ids.stdout);
    for (sha, name) in
        [(store::DEVELOPER_ID, "Developer ID Application"), (store::APPLE_DISTRIBUTION, "Apple Distribution")]
    {
        if !ids.contains(sha) {
            problems.push(format!("the {name} identity {sha} is not in the keychain"));
        }
    }
    let all = Command::new("security").args(["find-identity", "-v"]).output()?;
    if !String::from_utf8_lossy(&all.stdout).contains(store::MAC_INSTALLER) {
        problems.push(format!("the Mac Installer Distribution identity {} is missing", store::MAC_INSTALLER));
    }
    let probe_dir = root.join("target/release-out");
    std::fs::create_dir_all(&probe_dir)?;
    let probe = probe_dir.join("keychain-probe");
    std::fs::copy("/usr/bin/true", &probe)?;
    let signed = Command::new("codesign")
        .args(["--force", "--timestamp=none", "--sign", store::DEVELOPER_ID])
        .arg(&probe)
        .output()?;
    let _ = std::fs::remove_file(&probe);
    if !signed.status.success() {
        problems.push(format!("codesign cannot sign with {} (is its keychain locked?)", store::DEVELOPER_ID));
    }
    // The Mac App Store profile.
    let profile = profile_path(args);
    match Command::new("security").args(["cms", "-D", "-i"]).arg(&profile).output() {
        Ok(out) if out.status.success() => {
            let plist = String::from_utf8_lossy(&out.stdout);
            if !plist.contains(&format!("{}.com.hutsonlabs.drift", store::TEAM_ID)) {
                problems.push(format!("{} is not the Drift profile", profile.display()));
            }
            match profile_expiry(&out.stdout) {
                Some(expiry) if expiry > now() + PROFILE_MARGIN.as_secs() => {}
                Some(_) => problems.push(format!("{} expires within 14 days", profile.display())),
                None => problems.push(format!("cannot read the expiry of {}", profile.display())),
            }
        }
        _ => problems.push(format!("the Mac App Store profile {} is missing", profile.display())),
    }
    // Disk, tools, notes.
    match free_disk(root) {
        Ok(free) if free >= MIN_FREE_DISK => {}
        Ok(free) => problems.push(format!("only {} GB free; the builds need 40 GB", free >> 30)),
        Err(e) => problems.push(format!("cannot read free disk: {e}")),
    }
    let mut tools = vec!["xcrun", "hdiutil", "productbuild", "plutil", "ditto"];
    if opts.spaces {
        tools.push("curl");
        if let Err(e) = spaces_token() {
            problems.push(format!("{e:#}"));
        }
    }
    for tool in tools {
        if Command::new("which").arg(tool).output().map_or(true, |o| !o.status.success()) {
            problems.push(format!("{tool} is not installed"));
        }
    }
    if !Command::new(cargo()).args(["tauri", "--version"]).output().is_ok_and(|o| o.status.success()) {
        problems.push("cargo-tauri is not installed".to_owned());
    }
    match std::fs::read_to_string(release::notes_path(root, version)) {
        Ok(notes) => {
            if let Err(e) = release::validate_notes(&notes) {
                problems.push(format!("release/notes/{version}.md: {e}"));
            }
        }
        Err(_) => {
            problems.push(format!("release/notes/{version}.md is missing (copy release/notes/TEMPLATE.md)"))
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    bail!("preflight failed:\n  - {}", problems.join("\n  - "))
}

/// The profile's `ExpirationDate` as Unix seconds.
fn profile_expiry(decoded: &[u8]) -> Option<u64> {
    let out = Command::new("plutil")
        .args(["-extract", "ExpirationDate", "raw", "-o", "-", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()
        .and_then(|mut child| {
            use std::io::Write as _;
            child.stdin.take()?.write_all(decoded).ok()?;
            child.wait_with_output().ok()
        })?;
    let iso = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let date =
        Command::new("date").args(["-j", "-u", "-f", "%Y-%m-%dT%H:%M:%SZ", &iso, "+%s"]).output().ok()?;
    String::from_utf8_lossy(&date.stdout).trim().parse().ok()
}

// ---- the run -----------------------------------------------------------------------------------------

/// `cargo xtask release --set-version yy.mm`: rewrites every version file and the lockfile.
pub fn set_version(root: &Path, version: &str) -> Result<()> {
    let files = release::VERSION_FILES
        .iter()
        .map(|f| Ok((*f, std::fs::read_to_string(root.join(f.path))?)))
        .collect::<Result<Vec<_>>>()?;
    for (path, text) in release::set_version(&files, version)? {
        std::fs::write(root.join(path), text)?;
    }
    checked(
        Command::new(cargo()).args(["metadata", "--format-version", "1"]).current_dir(root),
        "cargo metadata",
    )?;
    let notes = release::notes_path(root, version);
    if !notes.exists() {
        std::fs::write(&notes, release::NOTES_TEMPLATE.replace("yy.mm", version))?;
    }
    println!(
        "version set to {version} in {} files and Cargo.lock; edit {} and commit:\n  git commit -am \"chore(release): {version}\"",
        release::VERSION_FILES.len(),
        notes.display()
    );
    Ok(())
}

/// The release pipeline.
pub fn run(root: &Path, args: &Args) -> Result<()> {
    let opts = args.options;
    let steps = release::steps(opts)?;
    let version = release::check_versions(&release::read_versions(root)?)?;
    let sha = git(root, &["rev-parse", "HEAD"])?;
    let epoch: u64 = git(root, &["log", "-1", "--format=%ct"])?.parse()?;
    deterministic_env(root, epoch);
    let latest = root.join("target/release-out/LATEST");

    // An earlier run of the same release, if resuming.
    let mut resumed: Option<(PathBuf, Manifest)> = None;
    if args.resume {
        let dir = std::fs::read_to_string(&latest).context("--resume: no earlier release run")?;
        let out = root.join("target/release-out").join(dir.trim());
        let manifest = Manifest::from_json(&std::fs::read_to_string(out.join("manifest.json"))?)?;
        release::resumable(&manifest, &version, &sha)?;
        resumed = Some((out, manifest));
    }

    let started = now();
    log(None, &format!("==> release Drift {version} at {} ({:?})", &sha[..12], steps));
    let preflight_at = now();
    preflight(root, args, &version)?;
    let gate_at = now();
    if release::receipt_path(root, &sha).exists() {
        log(None, "==> gate: cargo xtask ci already passed on this commit");
    } else {
        log(None, "==> gate: running cargo xtask ci");
        super::ci(root, true, true)?;
    }
    let gate_done = now();

    // The build number, once per release.
    let (out, mut manifest) = match resumed {
        Some((out, mut m)) => {
            m.dry_run = opts.dry_run;
            log(Some(&out), &format!("==> resuming {} ({})", out.display(), m.build));
            (out, m)
        }
        None => {
            let build = match args.build_number {
                Some(n) => n,
                None => {
                    let builds = with_retries(StepId::BuildNumber, || {
                        release::builds_by_platform(&asc_get(&format!(
                            "/v1/builds?filter%5Bapp%5D={}&fields%5Bbuilds%5D=version,preReleaseVersion\
                             &include=preReleaseVersion&fields%5BpreReleaseVersions%5D=platform&limit=200",
                            testflight::APP_ID
                        ))?)
                    })?;
                    // Lockstep: one number for iOS and macOS, above everything on either.
                    match release::asc_sync(&builds) {
                        release::AscSync::Partial { build, missing } => eprintln!(
                            "xtask: warning: build {build} exists without {missing:?}: an interrupted release. \
                             Resume it with --resume to upload the missing platform with {build}; this run \
                             starts {} on both.",
                            release::next_build(&builds)
                        ),
                        release::AscSync::Diverged { ios, macos } => eprintln!(
                            "xtask: App Store Connect is out of step (iOS {ios}, macOS {macos}); this release \
                             uses {} on both.",
                            release::next_build(&builds)
                        ),
                        _ => {}
                    }
                    release::next_build(&builds)
                }
            };
            let out = release::out_dir(root, &version, build);
            if out.exists() {
                bail!("{} already exists; use --resume or remove it", out.display());
            }
            std::fs::create_dir_all(&out)?;
            let toolchain = Command::new("rustc").arg("--version").current_dir(root).output()?;
            let mut lockfiles = std::collections::BTreeMap::new();
            for lock in ["Cargo.lock", "ui/bun.lock", "third_party/ironrdp/Cargo.lock"] {
                if let Ok((hash, _)) = sha256_file(&root.join(lock)) {
                    lockfiles.insert(lock.to_owned(), hash);
                }
            }
            let m = Manifest {
                version: version.clone(),
                build,
                git_sha: sha.clone(),
                source_date_epoch: epoch,
                toolchain: String::from_utf8_lossy(&toolchain.stdout).trim().to_owned(),
                lockfiles,
                dry_run: opts.dry_run,
                steps: Vec::new(),
                artifacts: Vec::new(),
                uploads: std::collections::BTreeMap::new(),
            };
            (out, m)
        }
    };
    std::fs::write(&latest, out.file_name().context("out dir")?.to_string_lossy().as_bytes())?;
    let build = manifest.build;
    manifest.done(StepId::Preflight, preflight_at, gate_at, Vec::new());
    manifest.done(StepId::Gate, gate_at, gate_done, Vec::new());
    if !manifest.steps.iter().any(|r| r.step == StepId::BuildNumber) {
        manifest.done(StepId::BuildNumber, gate_done, now(), Vec::new());
    }
    save(&out, &manifest)?;
    log(Some(&out), &format!("==> Drift {version} ({build}) into {}", out.display()));

    let ctx =
        Ctx { root, out: &out, version: &version, build, profile: profile_path(args), dry_run: opts.dry_run };
    for step in
        steps.iter().copied().filter(|s| !matches!(s, StepId::Preflight | StepId::Gate | StepId::BuildNumber))
    {
        let hash_of = |file: &Path| sha256_file(&out.join(file)).ok().map(|(h, _)| h);
        if args.resume && release::should_skip(step, &manifest, hash_of) {
            log(Some(&out), &format!("==> {step:?}: done earlier, outputs intact; skipped"));
            manifest.skipped(step, now());
            save(&out, &manifest)?;
            continue;
        }
        let at = now();
        log(Some(&out), &format!("==> {step:?}"));
        let result = with_retries(step, || run_step(step, &ctx, &mut manifest)).and_then(|outputs| {
            // Builds must leave the tree as preflight found it: restore what Xcode rewrites,
            // refuse anything else.
            for path in release::RESTORED_AFTER_BUILD {
                git(root, &["checkout", "--", path])?;
            }
            let changed = release::unexpected_changes(&git_porcelain(root)?);
            if changed.is_empty() {
                Ok(outputs)
            } else {
                bail!("{step:?} changed tracked or new files: {}", changed.join(", "))
            }
        });
        match result {
            Ok(outputs) => {
                manifest.done(step, at, now(), outputs);
                save(&out, &manifest)?;
                log(Some(&out), &format!("    {step:?} done in {} s", now() - at));
            }
            Err(e) => {
                manifest.fail(step, now(), &format!("{e:#}"));
                save(&out, &manifest)?;
                log(Some(&out), &format!("    {step:?} FAILED: {e:#}"));
                bail!("release stopped at {step:?}; fix it and rerun with --resume: {e:#}");
            }
        }
    }
    log(Some(&out), &format!("==> release Drift {version} ({build}) finished in {} s", now() - started));
    for a in &manifest.artifacts {
        println!("    {:>12}  {}", a.size, a.file);
    }
    Ok(())
}

struct Ctx<'a> {
    root: &'a Path,
    out: &'a Path,
    version: &'a str,
    build: u32,
    profile: PathBuf,
    dry_run: bool,
}

fn altool(args: &[String], what: &str, timeout: Duration) -> Result<()> {
    let (key, key_id, issuer) = asc_key()?;
    let keys_dir = Path::new(&key).parent().context("the API key has no directory")?.to_owned();
    run_timeout(
        Command::new("xcrun")
            .args(args)
            .args(["--apiKey", &key_id, "--apiIssuer", &issuer])
            .env("API_PRIVATE_KEYS_DIR", &keys_dir),
        what,
        timeout,
    )
}

fn run_step(step: StepId, ctx: &Ctx<'_>, manifest: &mut Manifest) -> Result<Vec<Artifact>> {
    let (root, out, version, build) = (ctx.root, ctx.out, ctx.version, ctx.build);
    match step {
        StepId::Preflight | StepId::Gate | StepId::BuildNumber => Ok(Vec::new()),
        StepId::Clean => {
            for dir in [
                "target/release/bundle",
                "target/aarch64-apple-darwin/release/bundle",
                "target/universal-apple-darwin/release/bundle",
                "src-tauri/gen/apple/build",
            ] {
                let path = root.join(dir);
                if path.exists() {
                    std::fs::remove_dir_all(&path).with_context(|| format!("removing {dir}"))?;
                }
            }
            let ui = root.join("ui");
            checked(
                Command::new(bun()).args(["install", "--frozen-lockfile"]).current_dir(&ui),
                "bun install",
            )?;
            checked(Command::new(bun()).args(["run", "build"]).current_dir(&ui), "bun run build")?;
            Ok(Vec::new())
        }
        StepId::Ios => {
            let plan = testflight::plan(root, build);
            std::fs::create_dir_all(root.join("src-tauri/gen/apple/assets"))?;
            run_timeout(
                Command::new(cargo()).args(&plan.build_args).current_dir(root.join("src-tauri")),
                "cargo tauri ios build",
                step.timeout(),
            )?;
            with_retries(StepId::Testflight, || {
                altool(&plan.validate_args, "altool --validate-app (iOS)", step.timeout())
            })?;
            Ok(vec![collect(&plan.ipa, out, ArtifactKind::IosIpa, version, build)?])
        }
        StepId::MacAppStore => {
            let pkg = mas_package_build(root, Some(&ctx.profile), Some(build))?;
            let (validate, _) = testflight::mac_altool_args(&pkg);
            with_retries(StepId::Testflight, || {
                altool(&validate, "altool --validate-app (macOS)", step.timeout())
            })?;
            Ok(vec![collect(&pkg, out, ArtifactKind::MacAppStorePkg, version, build)?])
        }
        StepId::MacDmg => Ok(vec![mac_dmg(ctx, step)?]),
        StepId::Notarize => {
            let (key, key_id, issuer) = asc_key()?;
            let mut outputs = Vec::new();
            for kind in [ArtifactKind::MacDmg] {
                let name = release::artifact_name(kind, version, build);
                let dmg = out.join(&name);
                if Command::new("xcrun").args(["stapler", "validate"]).arg(&dmg).output()?.status.success() {
                    log(Some(out), &format!("    {name}: already notarized and stapled"));
                } else {
                    with_retries(StepId::Notarize, || {
                        run_timeout(
                            Command::new("xcrun")
                                .args(["notarytool", "submit"])
                                .arg(&dmg)
                                .args(["--wait", "--key", &key, "--key-id", &key_id, "--issuer", &issuer]),
                            "notarytool submit",
                            step.timeout(),
                        )
                    })?;
                    checked(Command::new("xcrun").args(["stapler", "staple"]).arg(&dmg), "stapler staple")?;
                }
                checked(
                    Command::new("spctl")
                        .args(["-a", "-t", "open", "--context", "context:primary-signature", "-vv"])
                        .arg(&dmg),
                    "spctl",
                )?;
                let (sha256, size) = sha256_file(&dmg)?;
                // Stapling changed the DMG: the build step's record follows it, so a resume keeps it.
                for record in &mut manifest.steps {
                    for a in &mut record.outputs {
                        if a.file == name {
                            a.sha256.clone_from(&sha256);
                            a.size = size;
                        }
                    }
                }
                outputs.push(Artifact { kind, file: name, sha256, size });
            }
            Ok(outputs)
        }
        StepId::Checksums => {
            let mut files = Vec::new();
            let mut artifacts = Vec::new();
            for kind in [ArtifactKind::IosIpa, ArtifactKind::MacAppStorePkg, ArtifactKind::MacDmg] {
                let name = release::artifact_name(kind, version, build);
                let (sha256, size) = sha256_file(&out.join(&name))?;
                files.push((name.clone(), sha256.clone()));
                artifacts.push(Artifact { kind, file: name, sha256, size });
            }
            let sums = out.join("SHA256SUMS");
            std::fs::write(&sums, release::sha256sums(&files))?;
            let (sha256, size) = sha256_file(&sums)?;
            let checksums =
                Artifact { kind: ArtifactKind::Checksums, file: "SHA256SUMS".into(), sha256, size };
            artifacts.push(checksums.clone());
            manifest.artifacts = artifacts;
            Ok(vec![checksums])
        }
        StepId::SyncCheck => {
            let mut found = Vec::new();
            for kind in [ArtifactKind::IosIpa, ArtifactKind::MacAppStorePkg, ArtifactKind::MacDmg] {
                let file = out.join(release::artifact_name(kind, version, build));
                if file.exists() {
                    found.push((kind, embedded_version(kind, &file, out)));
                }
            }
            release::check_sync(&found, version, build)?;
            log(Some(out), &format!("    every artifact embeds {version} ({build})"));
            Ok(Vec::new())
        }
        StepId::Testflight => {
            // Lockstep: only after both platforms were built, validated and checked (SyncCheck
            // runs right before this step on every run, including a resume).
            let mut failed = Vec::new();
            let present = |p: Platform| asc_build_id(p.asc_name(), build).map(|id| id.is_some());
            let (ios, macos) = (present(Platform::Ios)?, present(Platform::MacOs)?);
            for p in Platform::ALL {
                if (p == Platform::Ios && ios) || (p == Platform::MacOs && macos) {
                    if manifest.uploads.get(&p) != Some(&release::UploadState::Uploaded) {
                        manifest.record_upload(p, release::UploadState::AlreadyPresent);
                    }
                    log(Some(out), &format!("    TestFlight already has {p:?} build {build}; not uploading"));
                }
            }
            for p in release::to_upload(ios, macos) {
                let args = match p {
                    Platform::Ios => {
                        let ipa = out.join(release::artifact_name(ArtifactKind::IosIpa, version, build));
                        testflight::plan(root, build)
                            .upload_args
                            .iter()
                            .map(|a| if a.ends_with(".ipa") { ipa.display().to_string() } else { a.clone() })
                            .collect::<Vec<_>>()
                    }
                    Platform::MacOs => {
                        testflight::mac_altool_args(&out.join(release::artifact_name(
                            ArtifactKind::MacAppStorePkg,
                            version,
                            build,
                        )))
                        .1
                    }
                };
                let result = with_retries(StepId::Testflight, || {
                    altool(&args, &format!("altool --upload-app ({p:?})"), step.timeout())
                });
                match result {
                    Ok(()) => manifest.record_upload(p, release::UploadState::Uploaded),
                    Err(e) => {
                        manifest.record_upload(p, release::UploadState::Failed);
                        failed.push(format!("{p:?}: {e:#}"));
                    }
                }
                save(out, manifest)?;
            }
            if !failed.is_empty() {
                bail!(
                    "upload failed ({}); --resume uploads the missing platform with build {build}",
                    failed.join("; ")
                );
            }
            Ok(Vec::new())
        }
        StepId::WhatsNew => {
            let notes = std::fs::read_to_string(release::notes_path(root, version))?;
            let text = release::whats_new(&notes);
            for platform in Platform::ALL.map(Platform::asc_name) {
                let id = wait_for_build(platform, build, step.timeout())?;
                set_whats_new(&id, &text, out)?;
                log(Some(out), &format!("    {platform} build {build}: What to Test set"));
            }
            Ok(Vec::new())
        }
        StepId::Spaces => {
            if ctx.dry_run {
                bail!("a dry run never publishes");
            }
            let commit = git(ctx.root, &["rev-parse", "HEAD"])?;
            spaces(ctx.root, ctx.out, ctx.version, ctx.build, &commit)?;
            Ok(Vec::new())
        }
    }
}

/// `(CFBundleShortVersionString, CFBundleVersion)` of the app inside an artifact.
fn embedded_version(kind: ArtifactKind, file: &Path, out: &Path) -> Option<(String, String)> {
    let read = |plist: &Path| -> Option<(String, String)> {
        let get = |key: &str| {
            Command::new("plutil")
                .args(["-extract", key, "raw", "-o", "-"])
                .arg(plist)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        };
        Some((get("CFBundleShortVersionString")?, get("CFBundleVersion")?))
    };
    let scratch = out.join("sync-check");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).ok()?;
    let result = match kind {
        ArtifactKind::IosIpa => {
            let ok = Command::new("unzip")
                .args(["-q", "-o"])
                .arg(file)
                .args(["Payload/Drift.app/Info.plist", "-d"])
                .arg(&scratch)
                .status()
                .is_ok_and(|s| s.success());
            if ok { read(&scratch.join("Payload/Drift.app/Info.plist")) } else { None }
        }
        ArtifactKind::MacAppStorePkg => {
            let expanded = scratch.join("pkg");
            let ok = Command::new("pkgutil")
                .arg("--expand-full")
                .arg(file)
                .arg(&expanded)
                .status()
                .is_ok_and(|s| s.success());
            if ok { find_plist(&expanded, "Drift.app").and_then(|p| read(&p)) } else { None }
        }
        ArtifactKind::MacDmg => {
            let mount = scratch.join("mnt");
            std::fs::create_dir_all(&mount).ok()?;
            let ok = Command::new("hdiutil")
                .args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"])
                .arg(&mount)
                .arg(file)
                .stdout(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            let found = if ok { read(&mount.join("Drift.app").join("Contents/Info.plist")) } else { None };
            if ok {
                let _ = Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mount).status();
            }
            found
        }
        ArtifactKind::Checksums => None,
    };
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// `<…>/<app>/Contents/Info.plist` somewhere under `dir`.
fn find_plist(dir: &Path, app: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == app) {
            let plist = path.join("Contents/Info.plist");
            if plist.is_file() {
                return Some(plist);
            }
        }
        if path.is_dir()
            && !path.is_symlink()
            && let Some(found) = find_plist(&path, app)
        {
            return Some(found);
        }
    }
    None
}

/// Drift for Mac with Developer ID in a signed DMG (not yet notarized).
fn mac_dmg(ctx: &Ctx<'_>, step: StepId) -> Result<Artifact> {
    let (root, build) = (ctx.root, ctx.build);
    let target = "aarch64-apple-darwin";
    let config = format!(r#"{{"bundle":{{"macOS":{{"bundleVersion":"{build}"}}}}}}"#);
    // Tauri would notarize the bare app when it sees App Store Connect credentials; the pipeline
    // notarizes the DMG instead (one ticket, stapled to what users download).
    run_timeout(
        Command::new(cargo())
            .args(["tauri", "build", "--target", target, "--bundles", "app", "--config", &config])
            .current_dir(root.join("src-tauri"))
            .env_remove("APPLE_API_KEY")
            .env_remove("APPLE_API_ISSUER")
            .env_remove("APPLE_API_KEY_PATH")
            .env_remove("APPLE_ID")
            .env_remove("APPLE_PASSWORD"),
        "cargo tauri build (Developer ID)",
        step.timeout(),
    )?;
    let bundle = root.join("target").join(target).join("release/bundle");
    let app = bundle.join("macos/Drift.app");
    checked(
        Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(&app),
        "codesign --verify",
    )?;
    let staging = bundle.join("dmg-staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    checked(Command::new("ditto").arg(&app).arg(staging.join("Drift.app")), "ditto")?;
    std::os::unix::fs::symlink("/Applications", staging.join("Applications"))?;
    let dmg = bundle.join("dmg").join("Drift.dmg");
    std::fs::create_dir_all(dmg.parent().context("dmg dir")?)?;
    checked(
        Command::new("hdiutil")
            .args(["create", "-volname", "Drift", "-srcfolder"])
            .arg(&staging)
            .args(["-ov", "-format", "UDZO"])
            .arg(&dmg),
        "hdiutil",
    )?;
    checked(
        Command::new("codesign").args(["--force", "--timestamp", "--sign", store::DEVELOPER_ID]).arg(&dmg),
        "codesign (dmg)",
    )?;
    collect(&dmg, ctx.out, ArtifactKind::MacDmg, ctx.version, build)
}

/// The App Store Connect id of build `build` on `platform` (`IOS`, `MAC_OS`), if it exists.
fn asc_build_id(platform: &str, build: u32) -> Result<Option<String>> {
    let json = asc_get(&format!(
        "/v1/builds?filter%5Bapp%5D={}&filter%5Bversion%5D={build}&filter%5BpreReleaseVersion.platform%5D={platform}&fields%5Bbuilds%5D=version,processingState",
        testflight::APP_ID
    ))?;
    let value: serde_json::Value = serde_json::from_str(&json)?;
    Ok(value["data"].as_array().and_then(|d| d.first()).and_then(|b| b["id"].as_str()).map(str::to_owned))
}

/// Waits until App Store Connect has processed build `build` on `platform`; returns its id.
fn wait_for_build(platform: &str, build: u32, timeout: Duration) -> Result<String> {
    let start = Instant::now();
    loop {
        let json = asc_get(&format!(
            "/v1/builds?filter%5Bapp%5D={}&filter%5Bversion%5D={build}&filter%5BpreReleaseVersion.platform%5D={platform}&fields%5Bbuilds%5D=processingState",
            testflight::APP_ID
        ))?;
        let value: serde_json::Value = serde_json::from_str(&json)?;
        if let Some(b) = value["data"].as_array().and_then(|d| d.first()) {
            match b["attributes"]["processingState"].as_str() {
                Some("VALID") => return Ok(b["id"].as_str().unwrap_or_default().to_owned()),
                Some(state @ ("FAILED" | "INVALID")) => bail!("{platform} build {build} is {state}"),
                _ => {}
            }
        }
        if start.elapsed() > timeout {
            bail!("{platform} build {build} was not processed within {} s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_secs(30));
    }
}

fn set_whats_new(build_id: &str, text: &str, out: &Path) -> Result<()> {
    let json = asc_get(&format!("/v1/builds/{build_id}/betaBuildLocalizations"))?;
    let value: serde_json::Value = serde_json::from_str(&json)?;
    let existing = value["data"]
        .as_array()
        .and_then(|d| d.iter().find(|l| l["attributes"]["locale"] == "en-US"))
        .and_then(|l| l["id"].as_str().map(str::to_owned));
    let _ = out;
    match existing {
        Some(id) => {
            let body = serde_json::json!({"data": {"type": "betaBuildLocalizations", "id": id, "attributes": {"whatsNew": text}}});
            asc_request("PATCH", &format!("/v1/betaBuildLocalizations/{id}"), Some(&body.to_string()))?;
        }
        None => {
            let body = serde_json::json!({"data": {
                "type": "betaBuildLocalizations",
                "attributes": {"locale": "en-US", "whatsNew": text},
                "relationships": {"build": {"data": {"type": "builds", "id": build_id}}}
            }});
            asc_request("POST", "/v1/betaBuildLocalizations", Some(&body.to_string()))?;
        }
    }
    Ok(())
}

/// The spaces.hutson.cloud API token, checked against the server.
fn spaces_token() -> Result<String> {
    let token =
        std::env::var(release::SPACES_TOKEN_VAR).ok().filter(|t| !t.is_empty()).with_context(|| {
            format!("{} is not set (spaces.hutson.cloud API token)", release::SPACES_TOKEN_VAR)
        })?;
    let (status, _) = spaces_api(&token, "GET", "/user", None, None)?;
    if status != 200 {
        bail!(
            "{} is not accepted by spaces.hutson.cloud (HTTP {status}); make a token with repository write access",
            release::SPACES_TOKEN_VAR
        );
    }
    Ok(token)
}

/// One call to the spaces.hutson.cloud API through `curl`: a JSON body or a file upload. The
/// token goes in on stdin, never on the command line. Returns the HTTP status and the JSON
/// answer (`null` when there is none).
fn spaces_api(
    token: &str,
    method: &str,
    path: &str,
    json: Option<&serde_json::Value>,
    file: Option<&Path>,
) -> Result<(u16, serde_json::Value)> {
    use std::io::Write as _;
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--max-time", "1800", "-X", method, "-H", "@-", "-H", "Accept: application/json"])
        .args(["-w", "\n%{http_code}"]);
    if let Some(json) = json {
        cmd.args(["-H", "Content-Type: application/json", "--data-binary", &json.to_string()]);
    }
    if let Some(file) = file {
        cmd.arg("-F").arg(format!("attachment=@{}", file.display()));
    }
    cmd.arg(format!("{}{path}", release::SPACES_API));
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("running curl")?;
    child
        .stdin
        .take()
        .context("curl stdin")?
        .write_all(format!("Authorization: token {token}\n").as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!("curl {method} {path} failed ({})", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let (body, status) = text.rsplit_once('\n').context("curl printed no status")?;
    let status: u16 = status.trim().parse().context("curl printed no status")?;
    Ok((status, serde_json::from_str(body).unwrap_or(serde_json::Value::Null)))
}

/// Tags `commit` `v<version>.<build>`, pushes the tag to `origin` (spaces.hutson.cloud), and
/// publishes the release there with both DMGs and `SHA256SUMS`. Idempotent: an existing release
/// is reused and only missing files are attached. Never GitHub (RELEASE-spaces).
fn spaces(root: &Path, out: &Path, version: &str, build: u32, commit: &str) -> Result<()> {
    let token = spaces_token()?;
    let tag = release::tag(version, build);
    match git(root, &["rev-list", "-n", "1", &tag]) {
        Ok(at) if at == commit => {}
        Ok(at) => bail!("tag {tag} exists at {at}, not {commit}"),
        Err(_) => {
            git(root, &["tag", "-a", &tag, commit, "-m", &format!("Drift {version} ({build})")])?;
        }
    }
    with_retries(StepId::Spaces, || git(root, &["push", "origin", &tag]).map(|_| ()))?;
    let repo = format!("/repos/{}", release::SPACES_REPO);
    let (status, existing) = spaces_api(&token, "GET", &format!("{repo}/releases/tags/{tag}"), None, None)?;
    let release = if status == 200 {
        log(Some(out), &format!("    release {tag} already exists on spaces.hutson.cloud"));
        existing
    } else {
        let notes = std::fs::read_to_string(release::notes_path(root, version))?;
        let sums = std::fs::read_to_string(out.join("SHA256SUMS"))?;
        let body = release::spaces_release(version, build, commit, &notes, &sums);
        let (status, created) = with_retries(StepId::Spaces, || {
            spaces_api(&token, "POST", &format!("{repo}/releases"), Some(&body), None)
        })?;
        if status != 201 {
            bail!("creating release {tag} failed (HTTP {status}): {created}");
        }
        created
    };
    let id = release["id"].as_u64().context("the release has no id")?;
    let attached: Vec<&str> = release["assets"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect())
        .unwrap_or_default();
    for name in release::published_files(version, build) {
        if attached.contains(&name.as_str()) {
            continue;
        }
        let path = out.join(&name);
        let (status, answer) = with_retries(StepId::Spaces, || {
            spaces_api(&token, "POST", &format!("{repo}/releases/{id}/assets?name={name}"), None, Some(&path))
        })?;
        if status != 201 {
            bail!("attaching {name} failed (HTTP {status}): {answer}");
        }
        log(Some(out), &format!("    attached {name}"));
    }
    log(Some(out), &format!("    https://spaces.hutson.cloud/{}/releases/tag/{tag}", release::SPACES_REPO));
    Ok(())
}

/// Publishes an earlier, finished build (`target/release-out/<dir>`) on spaces.hutson.cloud
/// without rebuilding it: the run must be publishable, every recorded artifact must still have
/// its checksum, and the tag goes on the commit it was built from (which must be on
/// `origin/main`).
pub fn publish(root: &Path, dir: &str) -> Result<()> {
    let out = root.join("target/release-out").join(dir);
    let mut manifest = Manifest::from_json(&std::fs::read_to_string(out.join("manifest.json"))?)?;
    release::publishable(&manifest)?;
    for a in &manifest.artifacts {
        let (hash, _) = sha256_file(&out.join(&a.file))?;
        if hash != a.sha256 {
            bail!("{} changed since it was built", a.file);
        }
    }
    let (version, build, commit) = (manifest.version.clone(), manifest.build, manifest.git_sha.clone());
    git(root, &["fetch", "-q", "origin"])?;
    let on_main = Command::new("git")
        .args(["merge-base", "--is-ancestor", &commit, "origin/main"])
        .current_dir(root)
        .status()?
        .success();
    if !on_main {
        bail!("{commit} (the build's commit) is not on origin/main");
    }
    log(Some(&out), &format!("==> publishing Drift {version} ({build}) from {}", &commit[..12]));
    let at = now();
    match spaces(root, &out, &version, build, &commit) {
        Ok(()) => {
            manifest.done(StepId::Spaces, at, now(), Vec::new());
            save(&out, &manifest)
        }
        Err(e) => {
            manifest.fail(StepId::Spaces, now(), &format!("{e:#}"));
            save(&out, &manifest)?;
            Err(e)
        }
    }
}
