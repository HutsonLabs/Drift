//! Writes the tauri-specta TypeScript bindings. Used by `cargo xtask bindings`.
//!
//! Usage: `cargo run -p drift-app --example export_bindings -- <output.ts>`

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(out) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: export_bindings <output.ts>");
        return ExitCode::FAILURE;
    };
    match drift_app::export_bindings(&out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("export_bindings: {e}");
            ExitCode::FAILURE
        }
    }
}
