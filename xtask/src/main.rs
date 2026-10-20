//! `cargo xtask` — Drift's build automation entry point. See `docs/team-conventions.md`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use xtask::{
    bench, ci_plan, coverage, e2e_env, fixtures, fuzz_audit, host_check, npm_ban, repo, sanitize,
    secret_scan, workflows,
};

#[derive(Parser)]
#[command(name = "cargo xtask", about = "Drift build automation")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Full CI gate: npm-ban, secret-scan, workflow audit, bun test/build, bindings freshness,
    /// fmt, clippy -D warnings over all features and targets, the vendored IronRDP fork's fmt
    /// and tests, nextest under llvm-cov + coverage gate, cargo deny. The step list lives in
    /// `xtask::ci_plan` and is asserted by `xtask/tests/ci_plan.rs`.
    Ci {
        /// Run nextest without llvm-cov instrumentation and skip the coverage gate.
        #[arg(long)]
        no_coverage: bool,
        /// Skip `cargo deny` (e.g. offline).
        #[arg(long)]
        no_deny: bool,
    },
    /// Fast local subset: fmt check, clippy -D warnings, nextest.
    Check,
    /// Fail if npm/yarn/pnpm lockfiles or invocations exist (plan §0).
    NpmBan,
    /// Fail if `.github/workflows/` is missing a run the plan requires (M0-6, §5.3, M9-2).
    Workflows,
    /// Fail if a parser plan M9-2 wants fuzzed has no `cargo fuzz` target in the nightly run.
    FuzzTargets,
    /// Fail if any dev-machine secret appears in a repository file.
    SecretScan {
        /// Secrets directory (default `$DRIFT_SECRETS_DIR` or `~/code/drift-spikes/secrets`).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Evaluate the coverage gate against an llvm-cov JSON summary.
    CoverageGate {
        /// Path to the `cargo llvm-cov --json --summary-only` output.
        json: PathBuf,
    },
    /// Regenerate `ui/src/bindings.ts` from the tauri-specta builder.
    Bindings {
        /// Only check that the committed bindings are up to date.
        #[arg(long)]
        check: bool,
    },
    /// Run the real-host e2e suite through SSH local forwards (plan §5.3).
    E2e {
        /// Measure the M9-1 performance budgets and compare them with the stored baseline
        /// (`tests/e2e/bench-baseline.json`); a regression of more than 10 % fails the run.
        #[arg(long)]
        bench: bool,
        /// Replace the stored baseline with this run's numbers (implies `--bench`).
        #[arg(long, requires = "bench")]
        update_baseline: bool,
        /// Extra arguments passed to `cargo nextest run`.
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Build the Apple silicon .app/.dmg (task M9-5).
    Bundle,
    /// List private-API strings in a built app's executables that App Review may flag (STORE-2).
    ReviewRisk {
        /// The `.app` bundle (every file in `Contents/MacOS` is scanned) or one executable.
        path: PathBuf,
    },
    /// Release pipeline (ADR `RELEASE-pipeline`): preflight, gate, one build number for iOS and
    /// macOS, every artifact into `target/release-out/<yy.mm>-<build>/` with `manifest.json` and
    /// `SHA256SUMS`, the lockstep check, notarization; publishing only with `--testflight` /
    /// `--spaces` (spaces.hutson.cloud, never GitHub). Needs the App Store Connect key in the environment
    /// (`set -a; . ~/code/term.hut/.env; set +a`).
    Release {
        /// Build and validate everything; never notarize, upload or publish.
        #[arg(long)]
        dry_run: bool,
        /// Upload the iOS and the Mac App Store builds (always both) to TestFlight and set
        /// "What to Test".
        #[arg(long)]
        testflight: bool,
        /// Tag `v<yy.mm>.<build>`, push the tag to `origin` and publish the DMG as a release on
        /// spaces.hutson.cloud (token in `FORGEJO_API_KEY`).
        #[arg(long)]
        spaces: bool,
        /// Publish an earlier finished build on spaces.hutson.cloud without rebuilding it
        /// (`yy.mm-build`, a directory in `target/release-out`).
        #[arg(long, value_name = "YY.MM-BUILD")]
        publish: Option<String>,
        /// Continue the last run of this version and commit; finished steps with intact
        /// outputs are skipped and the build number is reused.
        #[arg(long)]
        resume: bool,
        /// Use this build number instead of App Store Connect's highest + 1 (both platforms).
        #[arg(long)]
        build_number: Option<u32>,
        /// The Mac App Store provisioning profile (default: STORE-1's copy in
        /// `~/Library/MobileDevice/Provisioning Profiles`).
        #[arg(long)]
        profile: Option<PathBuf>,
        /// Only rewrite every version file to `yy.mm` (and `Cargo.lock`), then stop.
        #[arg(long, value_name = "YY.MM")]
        set_version: Option<String>,
    },
    /// Build the Mac App Store package: universal, sandboxed, Apple Distribution (STORE-1).
    MasPackage {
        /// The Mac App Store provisioning profile for `com.hutsonlabs.drift` to embed.
        #[arg(long)]
        profile: Option<PathBuf>,
    },
    /// Verify the GNOME host configuration over SSH, read-only (task M0-4).
    HostSetupCheck {
        /// SSH destination (default `$DRIFT_E2E_SSH` or `homelab@10.1.2.40`).
        #[arg(long)]
        host: Option<String>,
        /// Headless daemons as USER:PORT[:persistent] (default: the homelab test users).
        #[arg(long = "headless")]
        headless: Vec<String>,
        /// Evaluate a saved report instead of connecting.
        #[arg(long)]
        report: Option<PathBuf>,
        /// Also write the raw report here.
        #[arg(long)]
        save_report: Option<PathBuf>,
    },
    /// Import sanitized §1.7 fixtures from a staging directory into fixtures/ (task M0-3).
    ImportFixtures {
        /// Staging directory with provenance.toml (default `$DRIFT_FIXTURES_STAGING` or
        /// `~/code/drift-spikes/fixtures-staging`).
        #[arg(long)]
        staging: Option<PathBuf>,
        /// Secrets directory (default `$DRIFT_SECRETS_DIR` or `~/code/drift-spikes/secrets`).
        #[arg(long)]
        secrets: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.cmd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cmd: Cmd) -> Result<()> {
    let root = repo::root();
    match cmd {
        Cmd::Ci { no_coverage, no_deny } => ci(&root, !no_coverage, !no_deny),
        Cmd::Check => check(&root),
        Cmd::NpmBan => npm_ban_check(&root),
        Cmd::Workflows => workflows_check(&root),
        Cmd::FuzzTargets => fuzz_targets_check(&root),
        Cmd::SecretScan { dir } => secret_scan_check(&root, dir),
        Cmd::CoverageGate { json } => coverage_gate(&root, &json),
        Cmd::Bindings { check } => bindings(&root, check),
        Cmd::E2e { bench, update_baseline, args } => {
            e2e(&root, &args, bench.then_some(BenchOptions { update_baseline }))
        }
        Cmd::Bundle => not_yet("bundle", "M9-5"),
        Cmd::ReviewRisk { path } => review_risk(&path),
        Cmd::MasPackage { profile } => mas_package(&root, profile.as_deref()).map(|_| ()),
        Cmd::Release { dry_run, testflight, spaces, publish, resume, build_number, profile, set_version } => {
            if let Some(dir) = publish {
                release_cmd::publish(&root, &dir)
            } else if let Some(version) = set_version {
                release_cmd::set_version(&root, &version)
            } else {
                release_cmd::run(
                    &root,
                    &release_cmd::Args {
                        options: xtask::release::Options { dry_run, testflight, spaces },
                        resume,
                        build_number,
                        profile,
                    },
                )
            }
        }
        Cmd::HostSetupCheck { host, headless, report, save_report } => {
            host_setup_check(host, &headless, report.as_deref(), save_report.as_deref())
        }
        Cmd::ImportFixtures { staging, secrets } => import_fixtures(&root, staging, secrets),
    }
}

fn review_risk(path: &Path) -> Result<()> {
    let macos = path.join("Contents/MacOS");
    // A Mac app keeps its executables in Contents/MacOS; an iOS app is flat, so every Mach-O
    // file at its top level is scanned.
    let dir = if macos.is_dir() { Some(macos) } else { path.is_dir().then(|| path.to_owned()) };
    let files: Vec<PathBuf> = match dir {
        Some(dir) => std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && is_mach_o(p))
            .collect(),
        None => vec![path.to_owned()],
    };
    let mut found = 0;
    for file in files {
        let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        for risk in xtask::store::review_risks(&bytes) {
            found += 1;
            println!("{}: `{}` — {}", file.display(), risk.needle, risk.why);
        }
    }
    println!("{found} review risk(s)");
    Ok(())
}

/// Whether `path` starts with a Mach-O or universal ("fat") magic number.
fn is_mach_o(path: &Path) -> bool {
    use std::io::Read as _;
    let mut magic = [0_u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok()
        && matches!(u32::from_be_bytes(magic), 0xFEED_FACF | 0xCFFA_EDFE | 0xCAFE_BABE | 0xBEBA_FECA)
}

mod release_cmd;

fn checked(cmd: &mut Command, what: &str) -> Result<()> {
    let status = cmd.status().with_context(|| format!("running {what}"))?;
    if !status.success() {
        bail!("{what} failed ({status})");
    }
    Ok(())
}

/// The App Store Connect key from the environment: (`.p8` path, key id, issuer id).
fn asc_key() -> Result<(String, String, String)> {
    let var = |name: &str| std::env::var(name).with_context(|| format!("{name} is not set"));
    Ok((var("APPLE_API_KEY_PATH")?, var("APPLE_KEY_ID")?, var("APPLE_API_ISSUER")?))
}

/// `GET path` on the App Store Connect API.
fn asc_get(path: &str) -> Result<String> {
    asc_request("GET", path, None)
}

/// `method path` on the App Store Connect API with an optional JSON body. The token goes to curl
/// on stdin, never on a command line, and is never printed.
fn asc_request(method: &str, path: &str, json: Option<&str>) -> Result<String> {
    use std::io::Write as _;
    use std::process::Stdio;
    use xtask::testflight;
    let (key, key_id, issuer) = asc_key()?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let body = testflight::token_body(&key_id, &issuer, now);
    let mut openssl = Command::new("openssl")
        .args(["dgst", "-sha256", "-sign", &key])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    openssl.stdin.take().context("openssl stdin")?.write_all(body.as_bytes())?;
    let der = openssl.wait_with_output()?.stdout;
    let raw = testflight::der_signature_to_raw(&der).context("openssl did not produce an ES256 signature")?;
    let token = format!("{body}.{}", testflight::base64url(&raw));
    let mut curl = Command::new("curl");
    curl.args(["-sS", "--fail-with-body", "--max-time", "120", "-X", method, "-K", "-"]);
    if let Some(json) = json {
        curl.args(["-H", "Content-Type: application/json", "--data-binary", json]);
    }
    let mut curl = curl
        .arg(format!("https://api.appstoreconnect.apple.com{path}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    curl.stdin
        .take()
        .context("curl stdin")?
        .write_all(format!("header = \"Authorization: Bearer {token}\"\n").as_bytes())?;
    let out = curl.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "App Store Connect {method} {path} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stdout).chars().take(400).collect::<String>()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn mas_package(root: &Path, profile: Option<&Path>) -> Result<PathBuf> {
    mas_package_build(root, profile, None)
}

fn mas_package_build(root: &Path, profile: Option<&Path>, build: Option<u32>) -> Result<PathBuf> {
    let plan = xtask::store::mas_plan(root, profile, build);
    // Tauri notarizes whenever App Store Connect credentials are in the environment; a Mac App
    // Store build is never notarized (App Review checks it), so it must not see them.
    let status = Command::new(cargo())
        .args(&plan.tauri_args)
        .current_dir(root.join("src-tauri"))
        .env_remove("APPLE_API_KEY")
        .env_remove("APPLE_API_ISSUER")
        .env_remove("APPLE_API_KEY_PATH")
        .env_remove("APPLE_ID")
        .env_remove("APPLE_PASSWORD")
        .status()?;
    if !status.success() {
        bail!("cargo {} failed", plan.tauri_args.join(" "));
    }
    match &plan.embed_profile {
        Some((from, to)) => {
            std::fs::copy(from, to).with_context(|| format!("copying {}", from.display()))?;
            // The profile sits inside the sealed bundle: sign again with the same identity
            // and entitlements so the signature covers it.
            let status = Command::new("codesign")
                .args(["--force", "--options", "runtime", "--sign", xtask::store::APPLE_DISTRIBUTION])
                .arg("--entitlements")
                .arg(root.join("src-tauri/entitlements.mas.plist"))
                .arg(&plan.app)
                .status()?;
            if !status.success() {
                bail!("codesign failed");
            }
        }
        None => eprintln!(
            "xtask: no --profile given; the package has no embedded provisioning profile and App Store \
             validation will reject it (STORE-1)"
        ),
    }
    if let Some(dir) = plan.pkg.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let status = Command::new("productbuild").args(&plan.productbuild_args).status()?;
    if !status.success() {
        bail!("productbuild failed");
    }
    println!("{}", plan.pkg.display());
    Ok(plan.pkg)
}

fn not_yet(name: &str, task: &str) -> Result<()> {
    bail!("`cargo xtask {name}` is not implemented yet (owned by task {task})")
}

fn import_fixtures(root: &Path, staging: Option<PathBuf>, secrets: Option<PathBuf>) -> Result<()> {
    let staging = staging
        .or_else(|| std::env::var_os("DRIFT_FIXTURES_STAGING").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("code/drift-spikes/fixtures-staging"))
        })
        .context("no staging directory; pass --staging")?;
    let secrets_dir =
        secrets.or_else(secret_scan::default_secrets_dir).context("no secrets directory; pass --secrets")?;
    let known = sanitize::load_known_secrets(&secrets_dir)?;
    if known.is_empty() {
        bail!("no secrets loaded from {}; refusing to import unsanitized fixtures", secrets_dir.display());
    }
    let dest = root.join("fixtures");
    step(&format!("import-fixtures {} -> {}", staging.display(), dest.display()));
    let report = fixtures::import(&staging, &dest, &known)?;
    let problems = fixtures::verify_manifest(&dest)?;
    if !problems.is_empty() {
        bail!("manifest verification failed:\n{}", problems.join("\n"));
    }
    eprintln!(
        "import-fixtures: {} files, {} known + {} one-time secrets, {} replacements, {} stale removed",
        report.files,
        known.len(),
        report.one_time_secrets,
        report.replacements,
        report.removed.len()
    );
    for r in &report.removed {
        eprintln!("import-fixtures: removed stale {r}");
    }
    Ok(())
}

fn host_setup_check(
    host: Option<String>,
    headless: &[String],
    report: Option<&Path>,
    save: Option<&Path>,
) -> Result<()> {
    let mut exp = host_check::Expectations::default();
    if !headless.is_empty() {
        exp.headless = headless.iter().map(|s| s.parse()).collect::<Result<_>>()?;
    }
    let host = host.or_else(|| std::env::var("DRIFT_E2E_SSH").ok()).unwrap_or_else(|| E2E_HOST.into());
    // The system daemon briefly stops listening while it hands a connection over, so a
    // failing live check is retried a few times before it is reported.
    let attempts = if report.is_some() { 1 } else { 3 };
    let mut last = Vec::new();
    for attempt in 1..=attempts {
        let text = match report {
            Some(p) => std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?,
            None => fetch_host_report(&host, &exp)?,
        };
        if let Some(p) = save {
            std::fs::write(p, &text)?;
        }
        let parsed = host_check::parse_report(&text)?;
        last = host_check::evaluate(&parsed, &exp);
        if last.iter().all(|c| c.ok) || attempt == attempts {
            break;
        }
        eprintln!("host-setup-check: attempt {attempt} had failures; retrying");
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    for c in &last {
        eprintln!("host-setup-check: {} {} ({})", if c.ok { "ok  " } else { "FAIL" }, c.name, c.detail);
    }
    let failed = last.iter().filter(|c| !c.ok).count();
    if failed > 0 {
        bail!(
            "{failed} host check(s) failed; see docs/gnome-host-setup.md (host/drift-host-setup.sh --check)"
        );
    }
    eprintln!("host-setup-check: {} checks passed on {host}", last.len());
    Ok(())
}

fn fetch_host_report(host: &str, exp: &host_check::Expectations) -> Result<String> {
    use std::io::Write as _;
    let specs: Vec<String> = exp.headless.iter().map(ToString::to_string).collect();
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", host, &format!("bash -s -- {}", specs.join(" "))])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("spawning ssh")?;
    child.stdin.take().context("ssh stdin")?.write_all(host_check::REPORT_SCRIPT.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("ssh {host} failed ({})", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn step(name: &str) {
    eprintln!("\n==> xtask: {name}");
}

/// Runs a command, failing with its description if it exits non-zero.
fn sh(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .current_dir(root)
        .args(args)
        .status()
        .with_context(|| format!("spawning {program}"))?;
    if !status.success() {
        bail!("`{program} {}` failed ({status})", args.join(" "));
    }
    Ok(())
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn bun() -> String {
    std::env::var("BUN").unwrap_or_else(|_| "bun".into())
}

/// Runs a list of [`ci_plan`] steps in order, announcing each one.
///
/// The gate is described by [`ci_plan::ci`], so `xtask/tests/ci_plan.rs` can assert what it
/// covers (the feature-gated targets of M6-2/M8-3, and the vendored fork's tests for M0-2).
fn run_steps(root: &Path, steps: &[ci_plan::Step]) -> Result<()> {
    for s in steps {
        step(&s.name);
        match &s.action {
            ci_plan::Action::InProcess(c) => run_in_process(root, *c)?,
            ci_plan::Action::Run { program, args, dir } => {
                let program = match program {
                    ci_plan::Program::Cargo => cargo(),
                    ci_plan::Program::Bun => bun(),
                };
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                sh(&root.join(dir), &program, &args)?;
            }
        }
    }
    Ok(())
}

fn run_in_process(root: &Path, check: ci_plan::Check) -> Result<()> {
    match check {
        ci_plan::Check::NpmBan => npm_ban_check(root),
        ci_plan::Check::SecretScan => secret_scan_check(root, None),
        ci_plan::Check::Workflows => workflows_check(root),
        ci_plan::Check::FuzzTargets => fuzz_targets_check(root),
        ci_plan::Check::BindingsFresh => bindings(root, true),
        ci_plan::Check::CoverageGate => coverage_gate(root, &root.join(ci_plan::COVERAGE_JSON)),
    }
}

fn ci(root: &Path, with_coverage: bool, with_deny: bool) -> Result<()> {
    run_steps(root, &ci_plan::ci(ci_plan::Options { coverage: with_coverage, deny: with_deny }))?;
    eprintln!("\nxtask: ci passed");
    // A receipt for `cargo xtask release` (RELEASE): only the full gate, from a clean tree.
    if with_coverage && with_deny {
        release_cmd::write_ci_receipt(root)?;
    }
    Ok(())
}

fn check(root: &Path) -> Result<()> {
    ensure_ui_dist(root)?;
    run_steps(root, &ci_plan::check_only())?;
    eprintln!("\nxtask: check passed");
    Ok(())
}

/// Audits `.github/workflows/` against the runs the plan requires (M0-6, §5.3, M9-2).
fn workflows_check(root: &Path) -> Result<()> {
    let dir = root.join(".github/workflows");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let text =
                std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            files.push((name, text));
        }
    }
    let problems = workflows::audit(&files);
    if problems.is_empty() {
        eprintln!("workflows: {} file(s) satisfy the plan", files.len());
        return Ok(());
    }
    for p in &problems {
        eprintln!("workflows: {p}");
    }
    bail!("{} workflow gap(s); see plan M0-6 and §5.3", problems.len())
}

/// Audits the `cargo fuzz` targets plan M9-2 requires (existence + nightly run).
fn fuzz_targets_check(root: &Path) -> Result<()> {
    let problems = fuzz_audit::audit_repo(root)?;
    if problems.is_empty() {
        eprintln!("fuzz targets: {} parser(s) covered nightly", fuzz_audit::REQUIRED.len());
        return Ok(());
    }
    for p in &problems {
        eprintln!("fuzz targets: {p}");
    }
    bail!("{} fuzz gap(s); see plan M9-2", problems.len())
}

/// `tauri::generate_context!` needs `ui/dist`; build it if missing.
fn ensure_ui_dist(root: &Path) -> Result<()> {
    if root.join("ui/dist/index.html").is_file() {
        return Ok(());
    }
    let ui = root.join("ui");
    let b = bun();
    step("ui/dist missing: bun install + bun run build");
    sh(&ui, &b, &["install", "--frozen-lockfile"])?;
    sh(&ui, &b, &["run", "build"])
}

fn npm_ban_check(root: &Path) -> Result<()> {
    let files = repo::list_files(root)?;
    let v = npm_ban::scan_files(root, &files);
    if v.is_empty() {
        eprintln!("npm-ban: {} files clean", files.len());
        return Ok(());
    }
    for x in &v {
        eprintln!("npm-ban: {}:{}: {}", x.path.display(), x.line, x.message);
    }
    bail!("{} npm-ban violation(s); use bun (plan §0)", v.len())
}

fn secret_scan_check(root: &Path, dir: Option<PathBuf>) -> Result<()> {
    let Some(dir) = dir.or_else(secret_scan::default_secrets_dir) else {
        eprintln!("secret-scan: no secrets directory configured; skipped");
        return Ok(());
    };
    let secrets = secret_scan::load_secrets(&dir)?;
    if secrets.is_empty() {
        eprintln!("secret-scan: no secrets found in {}; skipped", dir.display());
        return Ok(());
    }
    let files = repo::list_files(root)?;
    let hits = secret_scan::scan_files(root, &files, &secrets)?;
    if hits.is_empty() {
        eprintln!("secret-scan: {} secrets x {} files clean", secrets.len(), files.len());
        return Ok(());
    }
    for h in &hits {
        eprintln!("secret-scan: {} contains secret {} ({:?})", h.path.display(), h.secret, h.encoding);
    }
    bail!("{} secret(s) found in repository files", hits.len())
}

fn coverage_gate(root: &Path, json: &Path) -> Result<()> {
    let text = std::fs::read_to_string(json).with_context(|| format!("reading {}", json.display()))?;
    let results = coverage::evaluate(&text, root, coverage::TARGETS)?;
    let mut failed = false;
    for r in &results {
        let status = if !r.enforced() {
            "not yet enforced (no code)"
        } else if r.passed() {
            "ok"
        } else {
            failed = true;
            "FAIL"
        };
        eprintln!(
            "coverage: {:<28} {:>6.2}% ({}/{} lines, min {:.0}%) {status}",
            r.name,
            r.percent(),
            r.covered,
            r.lines,
            r.min_percent
        );
    }
    if failed {
        bail!("coverage gate failed");
    }
    Ok(())
}

fn bindings(root: &Path, check_only: bool) -> Result<()> {
    ensure_ui_dist(root)?;
    let committed = root.join("ui/src/bindings.ts");
    let out = if check_only { root.join("target/bindings.check.ts") } else { committed.clone() };
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    sh(
        root,
        &cargo(),
        &[
            "run",
            "--quiet",
            "--locked",
            "-p",
            "drift-app",
            "--example",
            "export_bindings",
            "--",
            out.to_str().context("non-UTF-8 path")?,
        ],
    )?;
    if check_only {
        let fresh = std::fs::read(&out)?;
        let current = std::fs::read(&committed).unwrap_or_default();
        if fresh != current {
            bail!("ui/src/bindings.ts is stale; run `cargo xtask bindings` and commit the result");
        }
        eprintln!("bindings: ui/src/bindings.ts is fresh");
    } else {
        eprintln!("bindings: wrote {}", out.display());
    }
    Ok(())
}

/// Daemon ports on the GNOME host and their local forward ports (plan §5.3).
const E2E_PORTS: &[(u16, u16)] = &[(13389, 3389), (13390, 3390), (13391, 3391), (13392, 3392)];
const E2E_HOST: &str = "homelab@10.1.2.40";

/// What `cargo xtask e2e --bench` does after the suite (task M9-1).
#[derive(Debug, Clone, Copy)]
struct BenchOptions {
    /// Overwrite the stored baseline with this run's numbers.
    update_baseline: bool,
}

/// Where the e2e bench writes its measurements.
const BENCH_REPORT: &str = "target/e2e/bench.json";

fn e2e(root: &Path, extra: &[String], bench_opts: Option<BenchOptions>) -> Result<()> {
    let host = std::env::var("DRIFT_E2E_SSH").unwrap_or_else(|_| E2E_HOST.into());
    let locals: Vec<u16> = E2E_PORTS.iter().map(|(local, _)| *local).collect();
    let busy = e2e_env::ports_in_use(&locals);
    if !busy.is_empty() {
        let list = busy.iter().map(u16::to_string).collect::<Vec<_>>().join(", ");
        bail!(
            "local forward port(s) {list} are already in use, so this run's SSH forwards cannot \
             be trusted; close the leftover forwards first (`pkill -f 'ssh -N .*:localhost:339'`)"
        );
    }
    let mut ssh = Command::new("ssh");
    ssh.args([
        "-N",
        "-o",
        "BatchMode=yes",
        "-o",
        "ExitOnForwardFailure=yes",
        // The suite runs for minutes and the tests open their own SSH connections; keep this
        // one alive and independent of any multiplexing master.
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ControlPath=none",
    ]);
    for (local, remote) in E2E_PORTS {
        ssh.arg("-L").arg(format!("{local}:localhost:{remote}"));
    }
    ssh.arg(&host).stdin(Stdio::null());
    step(&format!("opening SSH forwards to {host}"));
    let mut child = ForwardGuard(ssh.spawn().context("spawning ssh")?);
    wait_for_port(E2E_PORTS[0].0, &mut child.0)?;

    let mut cmd = Command::new(cargo());
    // The tests share one GNOME host (and one daemon per mode), so they never run in parallel.
    cmd.current_dir(root).args([
        "nextest",
        "run",
        "-p",
        "drift-e2e",
        "--run-ignored",
        "only",
        "--test-threads",
        "1",
    ]);
    let report_path = root.join(BENCH_REPORT);
    if bench_opts.is_some() {
        // A stale report would be merged into this run's numbers.
        let _ = std::fs::remove_file(&report_path);
        if let Some(parent) = report_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        cmd.env("DRIFT_E2E_BENCH", "1");
        cmd.env("DRIFT_E2E_BENCH_REPORT", &report_path);
        if extra.is_empty() {
            cmd.args(["-E", "binary(bench)"]);
        }
    }
    cmd.args(extra);
    cmd.env("DRIFT_E2E_HOST", "127.0.0.1").env("DRIFT_E2E_TLS_NAME", "10.1.2.40");
    for (local, remote) in E2E_PORTS {
        cmd.env(format!("DRIFT_E2E_PORT_{remote}"), local.to_string());
    }
    let mut vars: Vec<(String, String)> =
        std::env::vars().filter(|(k, _)| k.starts_with("DRIFT_E2E_")).collect();
    if let Some(dir) = secret_scan::default_secrets_dir() {
        for (k, v) in e2e_env::vars_from_dir(&dir) {
            if std::env::var_os(&k).is_none() {
                cmd.env(&k, &v);
                vars.push((k, v));
            }
        }
    }
    // Harness-side redact layer: every line of the run's output goes through the redactor.
    let redactor = e2e_env::Redactor::from_vars(&vars);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    step("cargo nextest run -p drift-e2e --run-ignored only");
    let mut child = cmd.spawn().context("running nextest")?;
    let pumps = [
        child.stdout.take().map(|s| pump_redacted(s, std::io::stdout(), redactor.clone())),
        child.stderr.take().map(|s| pump_redacted(s, std::io::stderr(), redactor)),
    ];
    let status = child.wait().context("waiting for nextest")?;
    for pump in pumps.into_iter().flatten() {
        let _ = pump.join();
    }
    if !status.success() {
        bail!("e2e failed ({status})");
    }
    match bench_opts {
        Some(opts) => bench_gate(root, &report_path, opts),
        None => Ok(()),
    }
}

/// Prints the M9-1 table and fails on a budget miss or a regression of more than 10 %.
fn bench_gate(root: &Path, report_path: &Path, opts: BenchOptions) -> Result<()> {
    step("bench: comparing with the stored baseline");
    let text = std::fs::read_to_string(report_path)
        .with_context(|| format!("reading {} (did the bench test run?)", report_path.display()))?;
    let report: bench::Report =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", report_path.display()))?;
    let baseline_path = root.join(bench::BASELINE_PATH);
    let baseline: Option<bench::Report> =
        std::fs::read_to_string(&baseline_path).ok().and_then(|t| serde_json::from_str(&t).ok());
    if baseline.is_none() {
        eprintln!("bench: no baseline at {}; only the budgets are enforced", baseline_path.display());
    }
    let rows = bench::compare(&report, baseline.as_ref());
    eprint!("\n{}", bench::table(&rows));
    for (key, value) in &report.notes {
        eprintln!("bench: {key}: {value}");
    }
    for row in &rows {
        if row.verdict == bench::Verdict::BudgetWaived
            && let Some(reason) = baseline.as_ref().and_then(|b| b.exceptions.get(row.metric.key))
        {
            eprintln!("bench: {} is over budget by agreement: {reason}", row.metric.key);
        }
    }
    if opts.update_baseline {
        let mut fresh = bench::baseline_from(&report);
        // A measurement never carries waivers; they are a decision and live on in the file.
        if let Some(old) = &baseline {
            fresh.exceptions = old.exceptions.clone();
        }
        std::fs::write(&baseline_path, format!("{}\n", serde_json::to_string_pretty(&fresh)?))?;
        eprintln!("bench: wrote {}", baseline_path.display());
    }
    let problems = bench::problems(&rows);
    for p in &problems {
        eprintln!("bench: {p}");
    }
    if !problems.is_empty() {
        bail!("{} performance problem(s) (plan M9-1)", problems.len());
    }
    eprintln!("bench: every budget met, no regression over 10 %");
    Ok(())
}

fn pump_redacted(
    src: impl std::io::Read + Send + 'static,
    mut dst: impl std::io::Write + Send + 'static,
    redactor: e2e_env::Redactor,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        use std::io::BufRead as _;
        let mut reader = std::io::BufReader::new(src);
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
            let _ = dst.write_all(redactor.redact(&String::from_utf8_lossy(&line)).as_bytes());
            let _ = dst.flush();
            line.clear();
        }
    })
}

struct ForwardGuard(Child);

impl Drop for ForwardGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for_port(port: u16, child: &mut Child) -> Result<()> {
    for _ in 0..100 {
        if let Some(status) = child.try_wait()? {
            bail!("ssh exited early ({status})");
        }
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    bail!("SSH forward on 127.0.0.1:{port} did not come up")
}
