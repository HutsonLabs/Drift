//! Library half of `cargo xtask`: pure, unit-tested checks used by the CLI in `main.rs`.
//!
//! - [`bench`]: the performance gate of `cargo xtask e2e --bench` (M9-1).
//! - [`icons`]: the macOS-only app icon set (M9-4).
//! - [`ci_plan`]: the `cargo xtask ci` gate as data, so tests can assert what it covers.
//! - [`workflows`]: `.github/workflows/` matches the runs the plan requires (M0-6, §5.3).
//! - [`fuzz_audit`]: the `cargo fuzz` targets plan M9-2 requires exist and run nightly.
//! - [`npm_ban`]: enforces "Bun, never npm" (plan §0, M0-1 Red test).
//! - [`secret_scan`]: fails if any value from `~/code/drift-spikes/secrets/*.txt` appears
//!   in tracked files as bytes/UTF-8 or UTF-16LE.
//! - [`coverage`]: the ≥ 85 % line-coverage gate for the crates listed in plan §0.
//! - [`e2e_env`]: maps the dev-machine secrets directory onto `DRIFT_E2E_*` variables.
//! - [`repo`]: repository discovery and file listing helpers.
//! - [`sanitize`] and [`fixtures`]: `cargo xtask import-fixtures` (M0-3).
//! - [`host_check`]: `cargo xtask host-setup-check` (M0-4).
//! - [`host_bundle`]: `cargo xtask host-bundle`, the Drift Host app bundle (HOST-9).

pub mod bench;
pub mod ci_plan;
pub mod coverage;
pub mod e2e_env;
pub mod fixtures;
pub mod fuzz_audit;
pub mod host_bundle;
pub mod host_check;
pub mod icons;
pub mod npm_ban;
pub mod repo;
pub mod sanitize;
pub mod secret_scan;
pub mod store;
pub mod testflight;
pub mod workflows;
