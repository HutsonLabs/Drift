# M0-1 — Workspace, CI gate and bindings conventions

- Status: accepted
- Date: 2026-09-22

## Decisions

1. **Workspace.** Members are listed explicitly in the root `Cargo.toml` (`crates/*`,
   `src-tauri` = package `drift-app`, `tests/e2e` = `drift-e2e`, `xtask`). Edition 2024,
   resolver 3, toolchain 1.99.1 (`rust-toolchain.toml`, with the `aarch64-apple-darwin` target only — macOS 27 is
   Apple silicon only — and `llvm-tools-preview`). `MACOSX_DEPLOYMENT_TARGET=27.0` (raised from 14.0 with the glass redesign) via `.cargo/config.toml [env]`.
   Shared dependency versions live in `[workspace.dependencies]`; crates use `dep.workspace = true`.
2. **Lints** (`[workspace.lints]`, every crate opts in with `[lints] workspace = true`):
   `missing_docs`, `clippy::undocumented_unsafe_blocks`, `clippy::unwrap_used`, `dbg_macro`
   (warn → error under `-D warnings`); `unsafe_op_in_unsafe_fn` and `unused_must_use` deny.
   `clippy.toml` allows unwrap/expect/dbg in tests. A justified `#[allow(clippy::expect_used)]`
   style exception needs a comment.
3. **Profiles.** `[profile.dev.package."*"] opt-level = 3` and
   `[profile.dev.package.drift-codec] opt-level = 3` (plan §0). Release: thin LTO, 1 CGU.
4. **Coverage gate.** `cargo xtask ci` runs nextest under `cargo llvm-cov` and evaluates
   `xtask/src/coverage.rs::TARGETS` (≥ 85 % lines for `drift-core`, `drift-input`,
   `drift-clipboard`, `drift-gfx`, `drift-rdp/src/redirect*` + `rdstls*`). A target with zero
   instrumented lines prints "not yet enforced" and passes; the gate turns on by itself as soon as
   the owning task lands code (stub files containing only `//!` docs have no lines).
   `cargo xtask ci --no-coverage` skips instrumentation locally when iterating.
5. **Bindings.** `drift_app::specta_builder()` is the single list of IPC commands/types.
   `cargo xtask bindings` runs `cargo run -p drift-app --example export_bindings -- ui/src/bindings.ts`;
   `cargo xtask ci` regenerates into `target/` and fails if `ui/src/bindings.ts` differs.
   specta/tauri-specta are pinned to `=2.0.0-rc.25` (pre-release APIs). `u64`/`Duration`
   cannot cross IPC (specta forbids BigInt); durations are serialized as integer milliseconds.
6. **UI.** `ui/` is bun + TypeScript, no framework. `bun run build` (`ui/build.ts`,
   `Bun.build`) writes `ui/dist` (tauri `frontendDist`); `bun test` uses happy-dom via
   `bunfig.toml` preload; `bun run typecheck` runs `tsc --noEmit`. Tauri's
   `beforeDevCommand`/`beforeBuildCommand` call `bun run build`; tauri runs them with `ui/`
   as the working directory, so they must not `cd` into it again. `ui/dist` must exist
   before `drift-app` compiles (`tauri::generate_context!`); `cargo xtask check` builds it when
   missing.
7. **npm ban.** `cargo xtask npm-ban` (also in `ci`) fails on npm/yarn/pnpm lockfiles and on
   npm/npx/yarn/pnpm invocations in scripts, workflows, docs and configs (`third_party/` exempt).
   A line containing `npm-ban: allow` is exempt.
8. **Secret scan.** `cargo xtask secret-scan` (also in `ci`) loads `*.txt` from
   `$DRIFT_SECRETS_DIR` or `~/code/drift-spikes/secrets` and fails if any credential-like value
   appears in a tracked/untracked-unignored file as UTF-8 or UTF-16LE. Reports name only the
   secrets file and line. Skipped (with a message) where the directory does not exist, e.g. CI.
