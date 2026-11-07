# M0-6 — What `cargo xtask ci` covers, and how that is kept honest

- Status: accepted
- Date: 2026-09-22
- Code: `xtask/src/ci_plan.rs`, `xtask/src/workflows.rs`, `xtask/tests/ci_plan.rs`,
  `xtask/tests/workflows.rs`, `.github/workflows/e2e-nightly.yml`

## Context

Three plan "Done" criteria were not enforced by the merge gate, and one of them had already
rotted undetected:

- **M6-2** ("a window-group integration test … run under `cargo test -p drift-app --features
  macos-ui-tests`"): `src-tauri/tests/tabs_ui.rs` stopped compiling when `RunOptions` gained a
  `secrets` field in 051f21e. The gate only built `--features drift-video/recording`, so
  nothing noticed.
- **M8-3** (the `recording` feature and its "Debug ▸ Record Session" menu hook in
  `src-tauri/src/recording.rs`): same blind spot — `drift-video/recording` was built,
  `drift-app/recording` was not.
- **M0-2** ("the fork's own tests pass"): the vendored IronRDP workspace is `exclude`d from the
  root workspace, and neither `cargo xtask ci` nor `.github/workflows/ci.yml` ever entered it.
  Its 1717 tests — including the redirection, RDSTLS and `CB_TEMP_DIRECTORY` decoders Drift
  depends on — were green only by luck.
- **§5.3** ("Nightly e2e … on a self-hosted macOS runner with SSH access to the host"): no such
  workflow file existed.

## Decisions

### 1. The gate is data, not a script

`xtask::ci_plan::ci(Options)` returns the ordered list of steps and `main.rs` executes exactly
that list. Tests can therefore assert what the merge gate covers instead of re-reading a
hand-written sequence of `sh(...)` calls. `cargo xtask check` is the same mechanism with a
shorter list.

### 2. Lints use `--all-features`, tests do not

The lint pass is `cargo clippy --workspace --all-targets --all-features --locked -- -D
warnings`. A hand-maintained `--features` list is what went stale, so the gate no longer keeps
one: every feature a crate declares is linted and compiled the moment it is declared, and
`xtask/tests/ci_plan.rs::the_gate_builds_every_declared_feature` reads the workspace manifests
and fails if any `pkg/feature` is left out.

Tests keep the narrow list (`drift-video/recording`). `--all-features` would enable
`drift-app/macos-ui-tests`, whose test binary opens real NSWindows and needs a logged-in window
server; it stays a manual/acceptance run (`docs/acceptance.md`). `--all-targets` still
*compiles* it on every run, which is all that was missing.

### 3. The vendored fork's tests run in CI

Two steps run inside `third_party/ironrdp`: `cargo fmt --all -- --check` and `cargo test` over
`VENDORED_TEST_PACKAGES`. A rev bump or a re-applied patch queue can no longer silently break
the decoders.

The package list is explicit rather than `--workspace`, because `cargo test --workspace` in that
tree fails on a macOS host: `ironrdp-rdpsnd-native` (pulled in by the `ironrdp` facade crate)
builds `libopus_sys`, which needs a libopus that is not part of the toolchain. The list is the
crates Drift both patches and depends on — `ironrdp-cliprdr`, `ironrdp-connector`,
`ironrdp-pdu`, `ironrdp-session` — plus `ironrdp-testsuite-core`, where M0-2's four Red tests
live. `xtask/tests/ci_plan.rs::the_vendored_test_step_covers_every_patched_crate_drift_depends_on`
derives that set from `third_party/ironrdp-patches/*.patch` and the root manifest's path
dependencies, so a future patch touching a fifth consumed crate fails the gate until the list
grows.

`ironrdp-client` and `ironrdp-web` are patched for upstream compatibility only (they call the
APIs the patches change). Drift depends on neither, and neither builds on a macOS host
(libopus, wasm-bindgen), so they are compiled by nobody and tested by nobody. That is a
deliberate, documented exclusion.

Cost on this machine: about 50 s cold, 6 s warm. `.github/workflows/ci.yml` caches
`third_party/ironrdp -> third_party/ironrdp/target` alongside the root target directory.

### 4. Workflow files are audited in-process

`xtask::workflows::audit` checks `.github/workflows/` against `REQUIRED`: `ci.yml` (M0-6),
`e2e-nightly.yml` (§5.3) and `fuzz-nightly.yml` (M9-2), each with the substrings that make it
the thing the plan asked for (`cargo xtask ci`, `macos-15`; `cargo xtask e2e`, `schedule:`,
`self-hosted`, `macOS`, `DRIFT_E2E_SYS_PASS`, `cargo xtask host-setup-check`; `cargo +nightly
fuzz run`). The check is textual on purpose: the workflows are short and hand-written, and a
YAML dependency would buy little. It runs as a step of `cargo xtask ci` and standalone as
`cargo xtask workflows`.

### 5. `e2e-nightly.yml` is a deliverable, not a running job

The GitHub remote is public and nothing is pushed (see `docs/team-conventions.md`), so the file
is never executed by GitHub today. It is still written as the real thing: a `self-hosted,
macOS, ARM64, drift-lan` runner (the suite needs SSH access to `homelab@10.0.2.40`; a hosted
runner cannot reach it, and macOS Local Network Privacy forces the SSH-forward transport of
plan §1.9), nightly cron at 04:17 UTC after the fuzz run, `concurrency` of one because the
tests share a single GNOME host, `cargo xtask host-setup-check` before the suite, credentials
from repository secrets mapped onto `DRIFT_E2E_*`, and `DRIFT_E2E_BENCH` only on a manual
`workflow_dispatch` with `bench: true` (plan M9-1's budgets need an idle host).

## Consequences

- Adding a cargo feature adds lint coverage automatically; removing the `--all-features` flag
  fails a test that names every uncovered feature.
- Bumping the vendored IronRDP rev now costs a fork test run in every CI invocation.
- `cargo xtask ci` grew two subprocess steps and one in-process check.
