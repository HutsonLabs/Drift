//! The `Drift::Mac::v1` DVC processors for both ends (task **EXT-1**).
//!
//! Dynamic channels are opened by the server. A Drift Host opens `Drift::Mac::v1`; GNOME and
//! any other RDP server do not, so a client's [`Link`] stays [`LinkState::Absent`] and the
//! session is plain RDP. When the channel opens, each end sends its [`Hello`] from `start`, and
//! the other end's Hello settles the version and the shared capabilities ([`negotiate`]).
//!
//! A malformed message never becomes a DVC error, because ironrdp ends the whole RDP session on
//! one: the extension switches itself off ([`LinkState::Failed`]) and the session carries on as
//! plain RDP. Messages after the handshake queue in the [`Link`]'s inbox for the task that owns
//! their type (EXT-2 … EXT-5, HOST-8).

use std::sync::{Arc, Mutex, PoisonError};

use ironrdp_core::{EncodeResult, WriteCursor, impl_as_any};
use ironrdp_dvc::{DvcClientProcessor, DvcEncode, DvcMessage, DvcProcessor, DvcServerProcessor};
use ironrdp_pdu::PduResult;

use crate::wire::{Capabilities, Hello, Message, decode, encode};
use crate::{Negotiated, negotiate};

/// The channel name (ADR `IOS-0`, decision 3).
pub const CHANNEL_NAME: &str = "Drift::Mac::v1";

/// Protocol versions this build speaks.
pub const PROTOCOL_VERSIONS: (u16, u16) = (1, 1);

/// Where one end of the channel stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    /// The server never opened the channel: plain RDP.
    Absent,
    /// Open; waiting for the peer's Hello.
    Opened,
    /// Both ends agreed.
    Negotiated(Negotiated),
    /// Switched off after a bad message or a failed negotiation; plain RDP from here.
    Failed(String),
    /// The channel was closed.
    Closed,
}

#[derive(Debug)]
struct Inner {
    state: LinkState,
    inbox: Vec<Message>,
    /// The DVC channel id while open, for an end that sends on its own (EXT-4's host status).
    channel_id: Option<u32>,
}

/// The session-facing side of one end: its state and the messages waiting for their task.
/// Cloning shares it.
#[derive(Debug, Clone)]
pub struct Link(Arc<Mutex<Inner>>);

impl PartialEq for Link {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Link {}

impl Default for Link {
    fn default() -> Self {
        Self::new()
    }
}

impl Link {
    /// A link in [`LinkState::Absent`].
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Inner { state: LinkState::Absent, inbox: Vec::new(), channel_id: None })))
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The current state.
    pub fn state(&self) -> LinkState {
        self.inner().state.clone()
    }

    /// The negotiated version and capabilities, once agreed.
    pub fn negotiated(&self) -> Option<Negotiated> {
        match self.inner().state {
            LinkState::Negotiated(n) => Some(n),
            _ => None,
        }
    }

    /// The DVC channel id while the channel is open.
    pub fn channel_id(&self) -> Option<u32> {
        self.inner().channel_id
    }

    /// Takes every message received since the last call.
    pub fn take_inbox(&self) -> Vec<Message> {
        std::mem::take(&mut self.inner().inbox)
    }

    /// Back to [`LinkState::Absent`] (a new connection leg starts).
    pub fn reset(&self) {
        let mut inner = self.inner();
        inner.state = LinkState::Absent;
        inner.inbox.clear();
        inner.channel_id = None;
    }

    fn set(&self, state: LinkState) {
        self.inner().state = state;
    }

    fn push(&self, message: Message) {
        self.inner().inbox.push(message);
    }
}

/// Answers messages for the task that owns them, inside the channel's own message handling.
///
/// DVC processors can only send in reply to what they receive (and when the channel starts), so
/// an exchange like pairing (HOST-8) is a responder: [`Responder::on_negotiated`] may speak
/// first once both Hellos agreed, and [`Responder::on_message`] either answers a message (it is
/// then not queued in the [`Link`]'s inbox) or leaves it for the inbox.
pub trait Responder: Send + std::fmt::Debug {
    /// Both ends agreed on `negotiated`; messages to send now.
    fn on_negotiated(&mut self, negotiated: &Negotiated) -> Vec<Message> {
        let _ = negotiated;
        Vec::new()
    }

    /// A message after the handshake. `Some(replies)` consumes it; `None` queues it.
    fn on_message(&mut self, message: &Message) -> Option<Vec<Message>> {
        let _ = message;
        None
    }
}

/// An encoded frame as a DVC message.
struct Frame(Vec<u8>);

/// An already encoded frame as a DVC message, for an end that sends outside the processor's own
/// message handling (a client's scrolls and gestures, EXT-2).
pub struct RawFrame(pub Vec<u8>);

impl ironrdp_core::Encode for RawFrame {
    fn encode(&self, dst: &mut WriteCursor<'_>) -> EncodeResult<()> {
        ironrdp_core::ensure_size!(in: dst, size: self.0.len());
        dst.write_slice(&self.0);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "Drift::Mac::v1"
    }

    fn size(&self) -> usize {
        self.0.len()
    }
}

impl DvcEncode for RawFrame {}

impl ironrdp_core::Encode for Frame {
    fn encode(&self, dst: &mut WriteCursor<'_>) -> EncodeResult<()> {
        ironrdp_core::ensure_size!(in: dst, size: self.0.len());
        dst.write_slice(&self.0);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "Drift::Mac::v1"
    }

    fn size(&self) -> usize {
        self.0.len()
    }
}

impl DvcEncode for Frame {}

/// What both ends do.
#[derive(Debug)]
struct Endpoint {
    link: Link,
    ours: Hello,
    /// Asked in order; the first that consumes a message answers it (EXT-2, HOST-8).
    responders: Vec<Box<dyn Responder>>,
}

impl Endpoint {
    fn new(link: Link, capabilities: Capabilities, role: &str) -> Self {
        let agent = format!("Drift {} {} {role}", env!("CARGO_PKG_VERSION"), std::env::consts::OS);
        let ours =
            Hello { min_version: PROTOCOL_VERSIONS.0, max_version: PROTOCOL_VERSIONS.1, capabilities, agent };
        Self { link, ours, responders: Vec::new() }
    }

    fn frames(&mut self, messages: Vec<Message>) -> Vec<DvcMessage> {
        let mut out: Vec<DvcMessage> = Vec::with_capacity(messages.len());
        for m in messages {
            match encode(&m) {
                Ok(bytes) => out.push(Box::new(Frame(bytes))),
                Err(e) => {
                    self.fail(format!("could not encode {:?}: {e}", m.kind()));
                    return Vec::new();
                }
            }
        }
        out
    }

    fn start(&mut self) -> Vec<DvcMessage> {
        self.link.reset();
        self.link.set(LinkState::Opened);
        match encode(&Message::Hello(self.ours.clone())) {
            Ok(bytes) => vec![Box::new(Frame(bytes))],
            Err(e) => {
                self.fail(format!("could not encode our hello: {e}"));
                Vec::new()
            }
        }
    }

    fn process(&mut self, payload: &[u8]) -> Vec<DvcMessage> {
        if matches!(self.link.state(), LinkState::Failed(_) | LinkState::Closed) {
            return Vec::new();
        }
        let message = match decode(payload) {
            Ok(m) => m,
            Err(e) => {
                self.fail(format!("bad message: {e}"));
                return Vec::new();
            }
        };
        match message {
            Message::Hello(theirs) => match negotiate(&self.ours, &theirs) {
                Ok(n) => {
                    tracing::info!(version = n.version, capabilities = n.capabilities.bits(), agent = %theirs.agent, "Drift::Mac::v1 negotiated");
                    self.link.set(LinkState::Negotiated(n));
                    let replies: Vec<Message> =
                        self.responders.iter_mut().flat_map(|r| r.on_negotiated(&n)).collect();
                    return self.frames(replies);
                }
                Err(e) => self.fail(e.to_string()),
            },
            other if self.link.negotiated().is_some() => {
                match self.responders.iter_mut().find_map(|r| r.on_message(&other)) {
                    Some(replies) => return self.frames(replies),
                    None => self.link.push(other),
                }
            }
            other => self.fail(format!("{:?} before the hello", other.kind())),
        }
        Vec::new()
    }

    fn fail(&self, why: String) {
        tracing::warn!(%why, "Drift::Mac::v1 switched off; continuing as plain RDP");
        self.link.set(LinkState::Failed(why));
    }
}

/// The client end (Drift for Mac, iPhone and iPad).
#[derive(Debug)]
pub struct DriftMacClient(Endpoint);

impl DriftMacClient {
    /// A client end reporting to `link` and offering `capabilities`.
    pub fn new(link: Link, capabilities: Capabilities) -> Self {
        Self(Endpoint::new(link, capabilities, "client"))
    }

    /// Lets `responder` answer messages after the handshake (pairing, HOST-8; gestures, EXT-2).
    /// Responders are asked in the order they were added.
    #[must_use]
    pub fn with_responder(mut self, responder: Box<dyn Responder>) -> Self {
        self.0.responders.push(responder);
        self
    }
}

impl_as_any!(DriftMacClient);

impl DvcProcessor for DriftMacClient {
    fn channel_name(&self) -> &str {
        CHANNEL_NAME
    }

    fn start(&mut self, channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        let messages = self.0.start();
        self.0.link.inner().channel_id = Some(channel_id);
        Ok(messages)
    }

    fn process(&mut self, _channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        Ok(self.0.process(payload))
    }

    fn close(&mut self, _channel_id: u32) {
        self.0.link.set(LinkState::Closed);
    }
}

impl DvcClientProcessor for DriftMacClient {}

/// The server end (Drift Host).
#[derive(Debug)]
pub struct DriftMacServer(Endpoint);

impl DriftMacServer {
    /// A server end reporting to `link` and offering `capabilities`.
    pub fn new(link: Link, capabilities: Capabilities) -> Self {
        Self(Endpoint::new(link, capabilities, "host"))
    }

    /// Lets `responder` answer messages after the handshake (pairing, HOST-8; gestures, EXT-2).
    /// Responders are asked in the order they were added.
    #[must_use]
    pub fn with_responder(mut self, responder: Box<dyn Responder>) -> Self {
        self.0.responders.push(responder);
        self
    }
}

impl_as_any!(DriftMacServer);

impl DvcProcessor for DriftMacServer {
    fn channel_name(&self) -> &str {
        CHANNEL_NAME
    }

    fn start(&mut self, channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        let messages = self.0.start();
        self.0.link.inner().channel_id = Some(channel_id);
        Ok(messages)
    }

    fn process(&mut self, _channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        Ok(self.0.process(payload))
    }

    fn close(&mut self, _channel_id: u32) {
        self.0.link.set(LinkState::Closed);
    }
}

impl DvcServerProcessor for DriftMacServer {}
