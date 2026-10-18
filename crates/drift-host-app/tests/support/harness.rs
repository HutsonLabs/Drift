//! A tiny libtest-compatible harness that runs every test on the **process main thread**
//! (AppKit views and windows are main-thread-only; libtest and nextest run tests on worker
//! threads). Supports what `cargo test` and `cargo nextest` use: `--list [--format terse]
//! [--ignored]`, `--exact <name>`, a substring filter, and ignores other flags.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;

/// One test: name and body.
pub type Test = (&'static str, fn());

/// Runs `tests` according to the command line and returns the process exit code.
pub fn run(tests: &[Test]) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let list = args.iter().any(|a| a == "--list");
    let ignored_only = args.iter().any(|a| a == "--ignored");
    let exact = args.iter().any(|a| a == "--exact");
    let mut skip_next = false;
    let mut filters = Vec::new();
    for a in &args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if matches!(a.as_str(), "--format" | "--test-threads" | "--color" | "--logfile" | "--skip" | "-Z") {
            skip_next = true;
            continue;
        }
        if !a.starts_with('-') {
            filters.push(a.clone());
        }
    }
    let selected: Vec<&Test> = tests
        .iter()
        .filter(|(name, _)| {
            filters.is_empty()
                || filters.iter().any(|f| if exact { name == f } else { name.contains(f.as_str()) })
        })
        .collect();

    if list {
        if !ignored_only {
            for (name, _) in &selected {
                println!("{name}: test");
            }
        }
        return ExitCode::SUCCESS;
    }
    if ignored_only {
        println!("\ntest result: ok. 0 passed; 0 failed; 0 ignored");
        return ExitCode::SUCCESS;
    }

    let mut failed = Vec::new();
    println!("\nrunning {} tests", selected.len());
    for (name, body) in &selected {
        match catch_unwind(AssertUnwindSafe(body)) {
            Ok(()) => println!("test {name} ... ok"),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed.push(*name);
            }
        }
    }
    let passed = selected.len() - failed.len();
    if failed.is_empty() {
        println!("\ntest result: ok. {passed} passed; 0 failed; 0 ignored");
        ExitCode::SUCCESS
    } else {
        println!("\nfailures: {failed:?}");
        println!("\ntest result: FAILED. {passed} passed; {} failed; 0 ignored", failed.len());
        ExitCode::FAILURE
    }
}
