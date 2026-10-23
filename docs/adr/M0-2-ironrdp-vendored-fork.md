# M0-2 — IronRDP is vendored in-repo instead of a public git fork

- Status: accepted
- Date: 2026-09-22
- Deviates from: plan §2 decision 1 ("fork `drift/ironrdp`, branch `drift-main`, consumed as a
  cargo git dependency pinned by `rev`") and M0-2 ("open an upstream PR for each change").

## Context

Drift needs IronRDP at rev `b149f500b85124c513646494335fb6cee525d897` plus Drift changes
(RDSTLS request, Server Redirection PDU decode, `ActiveStageOutput::ServerRedirect`, an RDSTLS
client, the `CB_TEMP_DIRECTORY` length fix; plan §1.10). The plan assumed a public fork on
GitHub. Creating a public fork, pushing branches, or opening upstream PRs are actions on the
owner's public GitHub presence and must not be taken without the owner's explicit consent.

## Decision

1. IronRDP is **vendored** at `third_party/ironrdp/`:
   - commit `chore: vendor IronRDP b149f50` is a pristine `git archive b149f500` (LICENSE-MIT and
     LICENSE-APACHE kept, per crate too);
   - commit `chore(M0-2): trim vendored IronRDP …` removes `web-client/`, `ffi/`, `fuzz/`,
     `xtask/`, `benches/`, `.github/`, `.agents/`, `.cargo/` and its `rust-toolchain.toml`
     (Drift's `rust-toolchain.toml` applies). All of `crates/` is kept so path dependencies resolve and
     the fork's own test suite (`ironrdp-testsuite-core`) still runs.
2. The vendored tree is **its own cargo workspace** (`members = ["crates/*"]`). The root
   workspace lists `exclude = ["third_party"]` and consumes crates through
   `[workspace.dependencies] ironrdp-* = { path = "third_party/ironrdp/crates/…" }`.
   Drift crates write `ironrdp-pdu.workspace = true`.
3. Every Drift change to IronRDP is a **separate commit** touching only `third_party/ironrdp/`
   (message prefix `fix(ironrdp):`/`feat(ironrdp):`), and its patch is exported to
   `third_party/ironrdp-patches/NNNN-<slug>.patch` with
   `git format-patch -1 <sha> --relative=third_party/ironrdp -o third_party/ironrdp-patches/`.
   Those files apply with `git am` onto upstream `b149f50`, ready for upstream PRs once the owner
   approves.
4. Running the fork's tests: `cd third_party/ironrdp && cargo test -p <crate>` (it has its own
   `Cargo.lock`; the Drift toolchain file applies from the parent directory).

## Consequences

- "Bump the rev" (plan §2.1) becomes: re-vendor a new upstream rev in a dedicated branch,
  re-apply `third_party/ironrdp-patches/*.patch` with `git am`, run the full e2e suite.
- `cargo clippy`/`nextest`/coverage in `cargo xtask ci` cover Drift workspace members only; the
  vendored crates are built as path dependencies (`opt-level = 3` in dev via
  `[profile.dev.package."*"]`). On top of that, `cargo xtask ci` enters `third_party/ironrdp`
  for `cargo fmt --check` and `cargo test` over the crates Drift patches and depends on plus
  `ironrdp-testsuite-core`, which is what makes M0-2's Done ("the fork's own tests pass")
  enforceable — see `docs/adr/M0-6-ci-gate-coverage.md`. `npm-ban` skips `third_party/`; the
  secret scan does not.
- If the owner later creates the public fork, switching is mechanical: replace the path
  dependencies with `git = …, rev = …` and delete `third_party/ironrdp`.
