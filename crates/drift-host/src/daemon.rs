//! The broker daemon (task **HOST-13**, ADR `HOST-13-login-window`). Humble object; decisions
//! are in [`crate::broker`].
//!
//! It owns the listening port and the pairing store, and nothing else: it accepts a TCP
//! connection, asks [`crate::broker::route`] which agent serves the console, and hands the socket
//! over (`SCM_RIGHTS`). It never reads a client's bytes, so the code that runs as root is the
//! accept loop, the routing, the agents' IPC frames and the device-list writes.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::os::fd::AsFd as _;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::broker::{AgentId, AgentPolicy, AgentTable, ConsoleState, Route, route};
use crate::fdpass::{peer_uid, recv_message, send_message};
use crate::ipc::Message;
use crate::pairing::{DEVICES_FILE, parse_devices};

/// The default broker socket (the daemon creates it; agents connect).
pub const DEFAULT_SOCKET: &str = "/var/run/com.hutsonlabs.drift.host.sock";

/// The default shared store: identity, `host.toml` and `devices.toml` for every agent.
pub const DEFAULT_STORE: &str = "/Library/Application Support/com.hutsonlabs.drift.host";

/// What the daemon runs with.
pub struct DaemonOptions {
    /// The TCP address devices connect to.
    pub listen: SocketAddr,
    /// The Unix socket agents connect to.
    pub socket: PathBuf,
    /// Where `devices.toml` is written.
    pub store_dir: PathBuf,
    /// Which agents are accepted.
    pub policy: AgentPolicy,
    /// Who is at the console now ([`crate::console::console_state`] in production).
    pub console: Box<dyn Fn() -> ConsoleState + Send + Sync>,
    /// How long a connection waits for its agent to start.
    pub wait_for_agent: Duration,
}

impl std::fmt::Debug for DaemonOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonOptions").field("listen", &self.listen).field("socket", &self.socket).finish()
    }
}

/// Counters for tests and the log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonStats {
    /// Agents connected now.
    pub agents: usize,
    /// Agents turned away.
    pub refused_agents: u64,
    /// Connections handed to an agent.
    pub handed: u64,
    /// Connections closed because no agent was there in time.
    pub dropped_no_agent: u64,
    /// Connections refused because the console could not be read.
    pub dropped_no_console: u64,
    /// Device lists written.
    pub saves: u64,
}

type Writers = Arc<Mutex<HashMap<AgentId, Arc<Mutex<UnixStream>>>>>;

struct Shared {
    table: Mutex<AgentTable>,
    writers: Writers,
    stats: Mutex<DaemonStats>,
    stop: AtomicBool,
    options: DaemonOptions,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running daemon. Stops on drop.
pub struct Daemon {
    addr: SocketAddr,
    shared: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon").field("addr", &self.addr).finish_non_exhaustive()
    }
}

const POLL: Duration = Duration::from_millis(50);

impl Daemon {
    /// Binds the TCP port and the agents' socket and starts brokering.
    pub fn start(options: DaemonOptions) -> io::Result<Self> {
        let tcp = std::net::TcpListener::bind(options.listen)?;
        tcp.set_nonblocking(true)?;
        let addr = tcp.local_addr()?;
        // A stale socket from a previous run would make bind fail.
        let _ = std::fs::remove_file(&options.socket);
        let unix = UnixListener::bind(&options.socket)?;
        unix.set_nonblocking(true)?;
        // Every user's agent connects; who it is comes from getpeereid, not the file mode.
        std::fs::set_permissions(&options.socket, std::fs::Permissions::from_mode(0o666))?;
        let shared = Arc::new(Shared {
            table: Mutex::new(AgentTable::default()),
            writers: Arc::default(),
            stats: Mutex::default(),
            stop: AtomicBool::new(false),
            options,
        });
        let s1 = Arc::clone(&shared);
        let agents = std::thread::Builder::new()
            .name("drift-host-agents".into())
            .spawn(move || accept_agents(&s1, &unix))?;
        let s2 = Arc::clone(&shared);
        let clients = std::thread::Builder::new()
            .name("drift-host-accept".into())
            .spawn(move || accept_clients(&s2, &tcp))?;
        tracing::info!(%addr, socket = %shared.options.socket.display(), "broker listening");
        Ok(Self { addr, shared, threads: vec![agents, clients] })
    }

    /// The TCP address devices connect to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A snapshot of the counters.
    pub fn stats(&self) -> DaemonStats {
        let mut stats = lock(&self.shared.stats).clone();
        stats.agents = lock(&self.shared.table).len();
        stats
    }

    /// Stops both loops and removes the socket.
    pub fn stop(mut self) {
        self.shutdown();
    }

    /// Runs until the process is killed (launchd's daemon).
    pub fn wait(mut self) {
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }

    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.shared.options.socket);
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn accept_agents(shared: &Arc<Shared>, unix: &UnixListener) {
    while !shared.stop.load(Ordering::SeqCst) {
        match unix.accept() {
            Ok((stream, _)) => {
                let shared = Arc::clone(shared);
                let _ = std::thread::Builder::new()
                    .name("drift-host-agent".into())
                    .spawn(move || serve_agent(&shared, stream));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(POLL),
            Err(e) => {
                tracing::warn!(error = %e, "agent accept failed");
                std::thread::sleep(POLL);
            }
        }
    }
}

fn serve_agent(shared: &Shared, stream: UnixStream) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let refuse = |stream: &UnixStream, text: String| {
        tracing::warn!(%text, "agent refused");
        lock(&shared.stats).refused_agents += 1;
        let _ = send_message(stream, &Message::Error { text }, None);
    };
    let (kind, uid) = match recv_message(&stream) {
        Ok((Message::Hello { kind, uid }, _)) => (kind, uid),
        Ok((other, _)) => return refuse(&stream, format!("expected Hello, got {other:?}")),
        Err(e) => return refuse(&stream, format!("no Hello: {e}")),
    };
    let peer = match peer_uid(&stream) {
        Ok(peer) => peer,
        Err(e) => return refuse(&stream, format!("no peer credentials: {e}")),
    };
    if let Err(e) = shared.options.policy.admit(kind, peer, uid) {
        return refuse(&stream, e.to_string());
    }
    let Ok(writer) = stream.try_clone() else { return };
    let writer = Arc::new(Mutex::new(writer));
    let id = lock(&shared.table).register(kind, uid);
    lock(&shared.writers).insert(id, Arc::clone(&writer));
    tracing::info!(agent = id, ?kind, uid, "agent connected");
    let _ = send_message(&lock(&writer), &Message::Welcome { agent: id }, None);
    let _ = stream.set_read_timeout(None);
    loop {
        match recv_message(&stream) {
            Ok((Message::SaveDevices { toml }, _)) => {
                let reply = match save_devices(shared, &toml) {
                    Ok(()) => Message::Saved,
                    Err(text) => Message::Error { text },
                };
                let _ = send_message(&lock(&writer), &reply, None);
            }
            Ok((other, _)) => tracing::warn!(agent = id, ?other, "unexpected message from an agent"),
            Err(_) => break,
        }
    }
    lock(&shared.table).remove(id);
    lock(&shared.writers).remove(&id);
    tracing::info!(agent = id, "agent gone");
}

fn save_devices(shared: &Shared, text: &str) -> Result<(), String> {
    parse_devices(text).map_err(|e| format!("not a device list: {e}"))?;
    let dir = &shared.options.store_dir;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(DEVICES_FILE);
    let tmp = dir.join(format!("{DEVICES_FILE}.tmp"));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    // Readable by the agents (public keys only), written only here.
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    lock(&shared.stats).saves += 1;
    Ok(())
}

fn accept_clients(shared: &Arc<Shared>, tcp: &std::net::TcpListener) {
    while !shared.stop.load(Ordering::SeqCst) {
        match tcp.accept() {
            Ok((stream, peer)) => {
                let shared = Arc::clone(shared);
                let _ = std::thread::Builder::new()
                    .name("drift-host-route".into())
                    .spawn(move || hand_over(&shared, stream, peer));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(POLL),
            Err(e) => {
                tracing::warn!(error = %e, "accept failed");
                std::thread::sleep(POLL);
            }
        }
    }
}

/// Routes one connection, waiting a while for a starting agent, then closes it.
fn hand_over(shared: &Shared, stream: std::net::TcpStream, peer: SocketAddr) {
    let deadline = Instant::now() + shared.options.wait_for_agent;
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let console = (shared.options.console)();
        match route(&console, &lock(&shared.table)) {
            Route::Agent(id) => {
                let writer = lock(&shared.writers).get(&id).cloned();
                if let Some(writer) = writer
                    && send_message(&lock(&writer), &Message::Connection { peer }, Some(stream.as_fd()))
                        .is_ok()
                {
                    lock(&shared.stats).handed += 1;
                    tracing::info!(%peer, agent = id, "connection handed over");
                    return;
                }
                // The agent died between routing and sending: forget it and route again.
                lock(&shared.table).remove(id);
                lock(&shared.writers).remove(&id);
            }
            Route::Refuse(reason) => {
                tracing::warn!(%peer, ?reason, "connection refused");
                lock(&shared.stats).dropped_no_console += 1;
                return;
            }
            Route::Wait if Instant::now() < deadline => std::thread::sleep(POLL),
            Route::Wait => {
                tracing::info!(%peer, ?console, "no agent for the console in time; closing");
                lock(&shared.stats).dropped_no_agent += 1;
                return;
            }
        }
    }
}
