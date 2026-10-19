# RELEASE — one deterministic, resumable release pipeline

- Status: accepted
- Date: 2026-10-18
- Replaces the `cargo xtask bundle` stub (M9-5) and ties together STORE-1 to STORE-4

## Context

Releases took several hand-run commands (a TestFlight upload per platform, `host-release`, a
Drift DMG by hand), each picking its own build number and output folder; iOS and macOS ended
up on different builds (26.10 (2) and 26.10 (3)). A failure halfway
meant starting again, and nothing recorded what a release was built from.

## Decisions

**Lockstep.** iOS and macOS are always released together and never out of version or build
sync. One release is one `yy.mm` version and one build number N, used by the iOS `.ipa`,
the Mac App Store `.pkg` and both DMGs. N is App Store Connect's highest build on **any**
platform + 1, chosen once. There is no single-platform publish path (the old
`cargo xtask testflight` is gone). The `sync-check` step reads `CFBundleShortVersionString`
and `CFBundleVersion` back out of every artifact and refuses to go on unless all equal
(version, N); it runs on every run, including a resume, right before any upload. Uploads
start only after both App Store artifacts are built, validated and checked; each platform's
upload state is in the manifest; if one upload fails, `--resume` uploads the missing one
with the same N (App Store Connect is asked first, so nothing is sent twice) and never picks
a new N. When App Store Connect is already out of step (today iOS 2, macOS 3), or a number
exists on one platform only, a new release still uses the highest + 1 on both, which puts
them back in step; the build-number step says so.

1. **One command**, `cargo xtask release`, with named steps in a fixed order: preflight, gate,
   build number, clean, iOS, Mac App Store, Drift DMG, Drift Host DMG, notarize, checksums,
   sync check, then the publish steps TestFlight (both platforms), What to Test, GitHub. The
   order and the rules are data in
   `xtask::release` and tested in `xtask/tests/release.rs`; `xtask/src/release_cmd.rs` runs them.
2. **Publishing is opt-in.** `--testflight` and `--github` add the publish steps; without them
   nothing leaves the Mac except the read-only App Store Connect query, `altool` validation
   and notarization. `--dry-run` also skips notarization and cannot be combined with a
   publish flag. The `release` skill (`.claude/skills/release/SKILL.md`) requires the user's
   explicit go-ahead for each publish flag and for version bumps.
3. **Preflight collects every problem** before anything is built: branch `main` and pushed
   (warnings in a dry run), clean tree, pinned toolchain, `Cargo.lock` current, App Store
   Connect key names, the Developer ID, Apple Distribution and Mac Installer identities, an
   unlocked keychain (a throw-away `codesign`), the Mac App Store profile present for
   `com.hutsonlabs.drift` and valid for 14 more days, 40 GB free, the tools, and filled-in
   release notes.
4. **Versions.** `yy.mm` marketing version, `yy.mm.0` where three parts are required; the seven
   files are listed in `release::VERSION_FILES` and must agree. `--set-version yy.mm` rewrites
   them all, refreshes `Cargo.lock` and starts `release/notes/<yy.mm>.md` from the template.
   One build number per release (App Store Connect's highest + 1) goes into every artifact:
   the iOS and Mac App Store bundles, Drift's DMG app and Drift Host's `Info.plist`.
5. **Gate receipt.** `cargo xtask ci` (full gate, clean tree) writes
   `target/ci-receipts/<HEAD>`; the release reuses it or runs the gate itself.
6. **Artifacts** go to `target/release-out/<yy.mm>-<build>/`: `Drift_<v>.<b>_iOS.ipa`,
   `Drift_<v>.<b>_MacAppStore.pkg`, `Drift_<v>.<b>_aarch64.dmg`,
   `Drift-Host_<v>.<b>_aarch64.dmg`, `SHA256SUMS`, `manifest.json` (commit, version, build,
   `SOURCE_DATE_EPOCH`, toolchain, lockfile hashes, each step with times, outputs and errors,
   final artifacts with SHA-256 and size; written atomically) and `release.log`. Drift's
   Developer ID DMG is built like Drift Host's: `tauri build --bundles app`, then `ditto`,
   `hdiutil` and `codesign`, never Tauri's DMG script (its Finder step fails with a locked
   screen). Tauri is kept away from the App Store Connect credentials for the Mac builds, so it
   does not notarize the bare app; the pipeline notarizes and staples the DMGs.
7. **Resume.** `--resume` continues the last run (`target/release-out/LATEST`) of the same
   version and commit. A finished step is skipped only if every output it recorded still has
   the recorded SHA-256; preflight and the gate always run; stapling updates the DMG records.
   A failed step is recorded in the manifest with its error before the run stops.
8. **Reliability.** Network steps (build number, notarization, uploads, What to Test, GitHub)
   retry three times with 10/30/90 s backoff; every long step has a timeout. Uploads check App
   Store Connect for the build first, notarization checks `stapler validate` first, and the
   GitHub step reuses an existing tag at HEAD and an existing release, so reruns do not
   duplicate anything.
9. **Tags** are `v<yy.mm>.<build>` (three parts, like the earlier `v0.2.0`), annotated, pushed
   to `origin` and `github`; the GitHub release carries both DMGs and `SHA256SUMS`, with the
   notes file plus the checksums as its notes.
10. **Determinism.** Every build step inherits `SOURCE_DATE_EPOCH` (HEAD's commit time),
    `CARGO_INCREMENTAL=0` and `RUSTFLAGS` remapping the checkout to `.` and cargo home to
    `/cargo`; builds use `--locked` and `bun install --frozen-lockfile`; bundle outputs are
    removed before building. `cargo xtask release --verify-reproducible` builds `drift-host`
    twice from scratch and compares the binaries.

## What is not byte-reproducible

Code signatures carry a secure timestamp and a CMS blob, notarization tickets are issued per
submission, `hdiutil` images embed creation times, and Xcode archives and `.ipa` files carry
build metadata (and `altool`/App Store processing re-signs). So signed artifacts differ
between runs even from the same commit; the manifest's lockfile hashes, commit and toolchain
identify what they were built from, and the unsigned binaries are what the reproducibility
check compares.
