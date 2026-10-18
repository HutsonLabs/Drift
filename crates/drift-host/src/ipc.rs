//! Messages between the daemon and its agents (task **HOST-13**). Pure framing.
//!
//! A frame is a big-endian `u32` length and a body: one kind byte and its fields. A connection
//! handed to an agent travels as a [`Message::Connection`] frame with the socket descriptor in
//! the same `sendmsg` (`SCM_RIGHTS`, see [`crate::fdpass`]). Decoding is strict: unknown kinds,
//! bad fields, truncation and trailing bytes are errors, and frames are capped at [`MAX_FRAME`].

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::broker::SessionKind;

/// The largest frame body either side accepts.
pub const MAX_FRAME: usize = 256 * 1024;

/// One message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Agent → daemon, first: which session it serves and for which uid.
    Hello {
        /// Its session.
        kind: SessionKind,
        /// The user it serves (0 for the login window).
        uid: u32,
    },
    /// Daemon → agent: admitted.
    Welcome {
        /// The agent's id.
        agent: u64,
    },
    /// Daemon → agent: a client connection (descriptor attached).
    Connection {
        /// Where it comes from.
        peer: SocketAddr,
    },
    /// Agent → daemon: write this `devices.toml` (the daemon owns the pairing store).
    SaveDevices {
        /// The whole file.
        toml: String,
    },
    /// Daemon → agent: saved.
    Saved,
    /// Either way: a refusal or failure.
    Error {
        /// What went wrong.
        text: String,
    },
}

/// A frame could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IpcError {
    /// Fewer bytes than the message needs.
    #[error("truncated message")]
    Truncated,
    /// Bytes after the message.
    #[error("trailing bytes after the message")]
    TrailingBytes,
    /// An unknown message kind.
    #[error("unknown message kind {0:#04x}")]
    UnknownKind(u8),
    /// A field is out of range.
    #[error("invalid field: {0}")]
    Invalid(&'static str),
    /// Over [`MAX_FRAME`].
    #[error("frame too large")]
    TooLarge,
}

const HELLO: u8 = 1;
const WELCOME: u8 = 2;
const CONNECTION: u8 = 3;
const SAVE_DEVICES: u8 = 4;
const SAVED: u8 = 5;
const ERROR: u8 = 6;

fn put_text(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&bytes[..len as usize]);
}

/// The frame for `message`: length prefix and body.
pub fn encode(message: &Message) -> Vec<u8> {
    let mut body = Vec::new();
    match message {
        Message::Hello { kind, uid } => {
            body.push(HELLO);
            body.push(match kind {
                SessionKind::Aqua => 0,
                SessionKind::LoginWindow => 1,
            });
            body.extend_from_slice(&uid.to_be_bytes());
        }
        Message::Welcome { agent } => {
            body.push(WELCOME);
            body.extend_from_slice(&agent.to_be_bytes());
        }
        Message::Connection { peer } => {
            body.push(CONNECTION);
            match peer.ip() {
                IpAddr::V4(ip) => {
                    body.push(4);
                    body.extend_from_slice(&ip.octets());
                }
                IpAddr::V6(ip) => {
                    body.push(6);
                    body.extend_from_slice(&ip.octets());
                }
            }
            body.extend_from_slice(&peer.port().to_be_bytes());
        }
        Message::SaveDevices { toml } => {
            body.push(SAVE_DEVICES);
            put_text(&mut body, toml);
        }
        Message::Saved => body.push(SAVED),
        Message::Error { text } => {
            body.push(ERROR);
            put_text(&mut body, text);
        }
    }
    let mut frame = u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes().to_vec();
    frame.extend_from_slice(&body);
    frame
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], IpcError> {
        if self.0.len() < n {
            return Err(IpcError::Truncated);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], IpcError> {
        self.take(N)?.try_into().map_err(|_| IpcError::Truncated)
    }

    fn text(&mut self) -> Result<String, IpcError> {
        let len = u32::from_be_bytes(self.array()?) as usize;
        if len > MAX_FRAME {
            return Err(IpcError::TooLarge);
        }
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| IpcError::Invalid("text is not UTF-8"))
    }
}

/// Decodes a frame body (without the length prefix).
pub fn decode(body: &[u8]) -> Result<Message, IpcError> {
    if body.len() > MAX_FRAME {
        return Err(IpcError::TooLarge);
    }
    let mut r = Reader(body);
    let kind = r.take(1)?[0];
    let message = match kind {
        HELLO => {
            let kind = match r.take(1)?[0] {
                0 => SessionKind::Aqua,
                1 => SessionKind::LoginWindow,
                _ => return Err(IpcError::Invalid("session kind")),
            };
            Message::Hello { kind, uid: u32::from_be_bytes(r.array()?) }
        }
        WELCOME => Message::Welcome { agent: u64::from_be_bytes(r.array()?) },
        CONNECTION => {
            let ip = match r.take(1)?[0] {
                4 => IpAddr::V4(Ipv4Addr::from(r.array::<4>()?)),
                6 => IpAddr::V6(Ipv6Addr::from(r.array::<16>()?)),
                _ => return Err(IpcError::Invalid("address family")),
            };
            Message::Connection { peer: SocketAddr::new(ip, u16::from_be_bytes(r.array()?)) }
        }
        SAVE_DEVICES => Message::SaveDevices { toml: r.text()? },
        SAVED => Message::Saved,
        ERROR => Message::Error { text: r.text()? },
        other => return Err(IpcError::UnknownKind(other)),
    };
    if !r.0.is_empty() {
        return Err(IpcError::TrailingBytes);
    }
    Ok(message)
}
