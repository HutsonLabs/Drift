//! `drift-host`: run Drift Host from a terminal (HOST-2 skeleton; the menu-bar app is HOST-9).
//!
//! ```text
//! drift-host set-password          # read the account's password from stdin into the Keychain
//! drift-host [--config-dir DIR] [--listen ADDR]
//! ```

use std::io::BufRead as _;
use std::path::PathBuf;
use std::process::ExitCode;

use drift_core::SecretRole;
use drift_host::config::{HostConfig, default_dir};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::server::{Host, HostOptions};
use uuid::Uuid;

/// Keychain service of the host's own items (not the client's `com.hutsonlabs.drift`).
const KEYCHAIN_SERVICE: &str = "com.hutsonlabs.drift.host";
/// The fixed "profile" id the host's NLA password is filed under.
const ACCOUNT_ID: Uuid = Uuid::from_u128(0x6472_6966_742d_686f_7374_2d6e_6c61_0001);

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
    let mut config = HostConfig::load(&dir).map_err(|e| e.to_string())?;
    if let Some(listen) = args.listen {
        config.listen = listen;
    }
    let keychain = drift_apple::Keychain::with_service(KEYCHAIN_SERVICE);
    if args.set_password {
        eprintln!("Password for {} (read from stdin; it is echoed if this is a terminal):", config.username);
        let mut line = zeroize::Zeroizing::new(String::new());
        std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
        let password = line.trim_end_matches(['\r', '\n']);
        if password.is_empty() {
            return Err("empty password".into());
        }
        keychain.set(ACCOUNT_ID, SecretRole::RdpUser, password).map_err(|e| e.to_string())?;
        config.save(&dir).map_err(|e| e.to_string())?;
        eprintln!("Saved in the Keychain ({KEYCHAIN_SERVICE}).");
        return Ok(());
    }
    let password = keychain
        .get(ACCOUNT_ID, SecretRole::RdpUser)
        .map_err(|e| e.to_string())?
        .ok_or("no password stored: run `drift-host set-password` first")?;
    let identity = HostIdentity::load_or_create(&dir).map_err(|e| e.to_string())?;
    let live = drift_host::setup::capture_or_pattern();
    let input = drift_host::setup::input_for(config.inject_input, live.as_ref().map(|l| l.1.clone()));
    if config.listens_beyond_loopback() {
        tracing::warn!(listen = %config.listen, "listening beyond loopback without device pairing (HOST-8)");
    }
    let host = Host::start(HostOptions {
        listen: config.listen,
        identity: identity.clone(),
        username: config.username.clone(),
        password,
        desktop: config.desktop,
        frames: Box::new(|| Box::new(TestPattern::new())),
        live: live.map(|l| l.0),
        input,
    })
    .map_err(|e| format!("cannot listen on {}: {e}", config.listen))?;
    println!("Drift Host listening on {}", host.addr());
    println!("Certificate fingerprint (pin this): {}", identity.fingerprint());
    println!("Account: {}", config.username);
    host.wait();
    Ok(())
}
