//! `drift-host`: run Drift Host from a terminal (HOST-2 skeleton, HOST-8 pairing; the menu-bar
//! app is HOST-9).
//!
//! ```text
//! drift-host set-password          # read the account's password from stdin into the Keychain
//! drift-host [--config-dir DIR] [--listen ADDR]          # serve paired devices
//! drift-host pair [--config-dir DIR] [--listen ADDR]     # …and show a pairing code and QR code
//! drift-host devices [--config-dir DIR]                  # list paired devices
//! drift-host forget ID [--config-dir DIR]                # unpair a device
//! ```

use std::io::BufRead as _;
use std::path::PathBuf;
use std::process::ExitCode;

use std::sync::Arc;

use drift_core::{SecretRole, SystemClock};
use drift_host::advert::{HostAnnouncement, local_host_name};
use drift_host::config::{HostConfig, default_dir};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::pairing::{CODE_LIFETIME, DeviceStore, Pairing};
use drift_host::server::{Host, HostOptions};
use uuid::Uuid;

/// Keychain service of the host's own items (not the client's `com.hutsonlabs.drift`).
const KEYCHAIN_SERVICE: &str = "com.hutsonlabs.drift.host";
/// The fixed "profile" id the host's NLA password is filed under.
const ACCOUNT_ID: Uuid = Uuid::from_u128(0x6472_6966_742d_686f_7374_2d6e_6c61_0001);

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve { pair: bool },
    SetPassword,
    Devices,
    Forget(Uuid),
}

struct Args {
    config_dir: Option<PathBuf>,
    listen: Option<std::net::SocketAddr>,
    command: Command,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { config_dir: None, listen: None, command: Command::Serve { pair: false } };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "set-password" => args.command = Command::SetPassword,
            "pair" => args.command = Command::Serve { pair: true },
            "devices" => args.command = Command::Devices,
            "forget" => {
                let id = it.next().ok_or("forget needs a device id (see `drift-host devices`)")?;
                args.command = Command::Forget(id.parse().map_err(|_| format!("{id} is not a device id"))?);
            }
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
    if config.host_id.is_nil() {
        config.host_id = Uuid::new_v4();
        config.save(&dir).map_err(|e| e.to_string())?;
    }
    let store = DeviceStore::load(&dir).map_err(|e| e.to_string())?;
    match args.command {
        Command::Devices => {
            if store.devices().is_empty() {
                println!("No paired devices. Run `drift-host pair` and pair from Drift on the device.");
            }
            for d in store.devices() {
                println!("{}  {}", d.id, d.name);
            }
            return Ok(());
        }
        Command::Forget(id) => {
            let pairing = Pairing::new(store, Arc::new(SystemClock));
            return if pairing.remove_device(id) { Ok(()) } else { Err(format!("no paired device {id}")) };
        }
        Command::SetPassword | Command::Serve { .. } => {}
    }
    let keychain = drift_apple::Keychain::with_service(KEYCHAIN_SERVICE);
    if args.command == Command::SetPassword {
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
    if config.inject_input {
        tracing::warn!("inject_input is set, but this build only logs input (HOST-6 wiring comes later)");
    }
    if !config.pairing && config.listen != config.effective_listen() {
        tracing::warn!(listen = %config.listen, "pairing is off: listening on loopback only");
    }
    let pairing = config.pairing.then(|| Pairing::new(store, Arc::new(SystemClock)));
    let host = Host::start(HostOptions {
        listen: config.effective_listen(),
        identity: identity.clone(),
        username: config.username.clone(),
        password,
        desktop: config.desktop,
        frames: Box::new(|| Box::new(TestPattern::new())),
        input: Box::new(RecordingInput::default()),
        pairing: pairing.clone(),
    })
    .map_err(|e| format!("cannot listen on {}: {e}", config.listen))?;
    println!("Drift Host listening on {}", host.addr());
    println!("Certificate fingerprint (pin this): {}", identity.fingerprint());
    println!("Account: {}", config.username);
    let host_name = local_host_name();
    let name = if config.name.trim().is_empty() {
        host_name.trim_end_matches(".local").to_owned()
    } else {
        config.name.clone()
    };
    let announcement = HostAnnouncement {
        host_id: config.host_id,
        name,
        host: host_name,
        port: host.port(),
        fingerprint: identity.fingerprint(),
    };
    // Discovery only makes sense where devices may connect: with pairing.
    let _advert = match &pairing {
        Some(_) => announcement.advertise(),
        None => None,
    };
    if let (Some(pairing), Command::Serve { pair: true }) = (&pairing, &args.command) {
        let code = pairing.open_window();
        println!();
        println!("Pairing code: {code}  (valid for {} minutes)", CODE_LIFETIME.as_secs() / 60);
        println!("{}", announcement.pairing_qr(&code));
        println!("Or open: {}", announcement.pairing_link(&code));
    } else if let Some(pairing) = &pairing {
        println!("Paired devices: {} (run `drift-host pair` to add one)", pairing.devices().len());
    }
    host.wait();
    Ok(())
}
