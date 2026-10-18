//! `drift-host`: run Drift Host from a terminal (HOST-2 skeleton, HOST-8 pairing; the menu-bar
//! app is HOST-9).
//!
//! ```text
//! drift-host set-password          # read the account's password from stdin into the Keychain
//! drift-host [--config-dir DIR] [--listen ADDR]          # serve paired devices
//! drift-host pair [--config-dir DIR] [--listen ADDR]     # …and show a pairing code and QR code
//! drift-host devices [--config-dir DIR]                  # list paired devices
//! drift-host forget ID [--config-dir DIR]                # unpair a device
//! drift-host daemon [--socket PATH] [--listen ADDR]      # the root broker (launchd, HOST-13)
//! drift-host agent aqua|loginwindow [--socket PATH]      # a session agent (launchd, HOST-13)
//! ```
//!
//! `daemon` and `agent` use the shared store (`/Library/Application Support/...`) unless
//! `--config-dir` says otherwise.

use std::io::BufRead as _;
use std::path::PathBuf;
use std::process::ExitCode;

use std::sync::Arc;

use drift_core::SystemClock;
use drift_host::broker::SessionKind;
use drift_host::config::default_dir;
use drift_host::launch;
use drift_host::pairing::{CODE_LIFETIME, DeviceStore, Pairing};
use uuid::Uuid;

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve { pair: bool },
    SetPassword,
    Devices,
    Forget(Uuid),
    Daemon,
    Agent(SessionKind),
}

struct Args {
    config_dir: Option<PathBuf>,
    socket: Option<PathBuf>,
    listen: Option<std::net::SocketAddr>,
    command: Command,
}

fn parse_args() -> Result<Args, String> {
    let mut args =
        Args { config_dir: None, socket: None, listen: None, command: Command::Serve { pair: false } };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "set-password" => args.command = Command::SetPassword,
            "pair" => args.command = Command::Serve { pair: true },
            "devices" => args.command = Command::Devices,
            "daemon" => args.command = Command::Daemon,
            "agent" => {
                let kind = it.next().ok_or("agent needs a session: aqua or loginwindow")?;
                args.command = Command::Agent(match kind.as_str() {
                    "aqua" => SessionKind::Aqua,
                    "loginwindow" => SessionKind::LoginWindow,
                    other => return Err(format!("unknown session {other}: aqua or loginwindow")),
                });
            }
            "--socket" => args.socket = Some(it.next().ok_or("--socket needs a value")?.into()),
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
    // HOST-13: the daemon and its agents share one store and one socket.
    let socket = args.socket.clone().unwrap_or_else(|| PathBuf::from(drift_host::daemon::DEFAULT_SOCKET));
    match args.command {
        Command::Daemon => {
            let dir = args.config_dir.unwrap_or_else(|| PathBuf::from(drift_host::daemon::DEFAULT_STORE));
            let (daemon, _advert) = launch::start_daemon(&dir, args.listen, &socket)?;
            println!("Drift Host broker listening on {}", daemon.addr());
            daemon.wait();
            return Ok(());
        }
        Command::Agent(kind) => {
            let dir = args.config_dir.unwrap_or_else(|| PathBuf::from(drift_host::daemon::DEFAULT_STORE));
            let (launched, _link) = launch::start_agent(&dir, &socket, kind)?;
            launched.host.wait();
            return Ok(());
        }
        _ => {}
    }
    let dir = args.config_dir.or_else(default_dir).ok_or("no config directory (HOME is not set)")?;
    let config = launch::load_config(&dir, args.listen)?;
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
        Command::SetPassword | Command::Serve { .. } | Command::Daemon | Command::Agent(_) => {}
    }
    if args.command == Command::SetPassword {
        eprintln!("Password for {} (read from stdin; it is echoed if this is a terminal):", config.username);
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
    if let (Some(pairing), Command::Serve { pair: true }) = (&launched.pairing, &args.command) {
        let code = pairing.open_window();
        println!();
        println!("Pairing code: {code}  (valid for {} minutes)", CODE_LIFETIME.as_secs() / 60);
        println!("{}", launched.announcement.pairing_qr(&code));
        println!("Or open: {}", launched.announcement.pairing_link(&code));
    } else if let Some(pairing) = &launched.pairing {
        println!("Paired devices: {} (run `drift-host pair` to add one)", pairing.devices().len());
    }
    launched.host.wait();
    Ok(())
}
