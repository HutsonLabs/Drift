//! An agent's link to the broker daemon (task **HOST-13**). Humble object.
//!
//! The agent says which session it serves, then receives client connections as descriptors and
//! feeds them to its [`Host`](crate::server::Host) ([`Incoming::Handed`]). Pairing writes go back
//! to the daemon ([`AgentLink::saver`]), which owns `devices.toml`.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use crate::broker::SessionKind;
use crate::fdpass::{recv_message, send_message};
use crate::ipc::Message;
use crate::pairing::DeviceSaver;
use crate::server::{Handed, Incoming};

/// A registered agent.
pub struct AgentLink {
    writer: Arc<Mutex<UnixStream>>,
    incoming: Mutex<Option<Handed>>,
    replies: Mutex<mpsc::Receiver<Message>>,
    id: u64,
}

impl std::fmt::Debug for AgentLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentLink").field("id", &self.id).finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AgentLink {
    /// Connects to the daemon at `socket` as an agent of `kind` for this process's user.
    pub fn connect(socket: &Path, kind: SessionKind) -> io::Result<Self> {
        let stream = UnixStream::connect(socket)?;
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        send_message(&stream, &Message::Hello { kind, uid }, None)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let id = match recv_message(&stream)? {
            (Message::Welcome { agent }, _) => agent,
            (Message::Error { text }, _) => {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, text));
            }
            (other, _) => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unexpected {other:?}")));
            }
        };
        stream.set_read_timeout(None)?;
        let writer = Arc::new(Mutex::new(stream.try_clone()?));
        let (sender, handed) = Handed::channel(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)));
        let (reply_tx, replies) = mpsc::channel();
        std::thread::Builder::new().name("drift-host-agent-link".into()).spawn(move || {
            loop {
                match recv_message(&stream) {
                    Ok((Message::Connection { peer }, Some(fd))) => {
                        let stream = std::net::TcpStream::from(fd);
                        if !sender.hand(stream, peer) {
                            return;
                        }
                    }
                    Ok((Message::Connection { .. }, None)) => tracing::warn!("a connection without a socket"),
                    Ok((reply, _)) => {
                        let _ = reply_tx.send(reply);
                    }
                    Err(e) => {
                        tracing::info!(error = %e, "the daemon closed the link");
                        return;
                    }
                }
            }
        })?;
        tracing::info!(agent = id, ?kind, "registered with the daemon");
        Ok(Self { writer, incoming: Mutex::new(Some(handed)), replies: Mutex::new(replies), id })
    }

    /// Where this agent's host gets its connections (once; later calls get [`Incoming::Bind`]).
    pub fn incoming(&self) -> Incoming {
        lock(&self.incoming).take().map_or(Incoming::Bind, Incoming::Handed)
    }

    /// Asks the daemon to write `devices.toml`; waits for its answer.
    pub fn save_devices(&self, toml: &str) -> Result<(), String> {
        save(&self.writer, &self.replies, toml)
    }

    /// A [`DeviceSaver`] for this agent's pairing store.
    pub fn saver(self: &Arc<Self>) -> DeviceSaver {
        let link = Arc::clone(self);
        Arc::new(move |toml: &str| link.save_devices(toml))
    }
}

fn save(
    writer: &Mutex<UnixStream>,
    replies: &Mutex<mpsc::Receiver<Message>>,
    toml: &str,
) -> Result<(), String> {
    let replies = lock(replies);
    send_message(&lock(writer), &Message::SaveDevices { toml: toml.to_owned() }, None)
        .map_err(|e| e.to_string())?;
    match replies.recv_timeout(Duration::from_secs(5)) {
        Ok(Message::Saved) => Ok(()),
        Ok(Message::Error { text }) => Err(text),
        Ok(other) => Err(format!("unexpected {other:?}")),
        Err(_) => Err("the daemon did not answer".into()),
    }
}
