//! `drift-host`: run Drift Host from a terminal (HOST-2 skeleton; the menu-bar app is HOST-9).
//!
//! ```text
//! drift-host set-password          # read the account's password from stdin into the Keychain
//! drift-host [--config-dir DIR] [--listen ADDR]
//! ```

use std::io::BufRead as _;
use std::path::PathBuf;
use std::process::ExitCode;

use drift_host::config::default_dir;
use drift_host::launch;

struct Args {
    config_dir: Option<PathBuf>,
    listen: Option<std::net::SocketAddr>,
    set_password: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { config_dir: None, listen: None, set_password: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "set-password" => args.set_password = true,
            "--config-dir" => args.config_dir = Some(it.next().ok_or("--config-dir needs a value")?.into()),
            "--listen" => {
                let value = it.next().ok_or("--listen needs a value")?;
                args.listen = Some(value.parse().map_err(|_| format!("{value} is not an address:port"))?);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("drift-host: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let dir = args.config_dir.or_else(default_dir).ok_or("no config directory (HOME is not set)")?;
    if args.set_password {
        eprintln!("Password for the host account (read from stdin; it is echoed if this is a terminal):");
        let mut line = zeroize::Zeroizing::new(String::new());
        std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
        launch::set_password(&dir, line.trim_end_matches(['\r', '\n']))?;
        eprintln!("Saved in the Keychain ({}).", launch::KEYCHAIN_SERVICE);
        return Ok(());
    }
    let launched = launch::start(&dir, args.listen)?;
    println!("Drift Host listening on {}", launched.host.addr());
    println!("Certificate fingerprint (pin this): {}", launched.fingerprint);
    println!("Account: {}", launched.config.username);
    launched.host.wait();
    Ok(())
}
