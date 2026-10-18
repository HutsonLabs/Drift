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
        Self(Arc::new(Mutex::new(Inner { state: LinkState::Absent, inbox: Vec::new() })))
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

    /// Takes every message received since the last call.
    pub fn take_inbox(&self) -> Vec<Message> {
        std::mem::take(&mut self.inner().inbox)
    }

    /// Back to [`LinkState::Absent`] (a new connection leg starts).
    pub fn reset(&self) {
        let mut inner = self.inner();
        inner.state = LinkState::Absent;
        inner.inbox.clear();
    }

    fn set(&self, state: LinkState) {
        self.inner().state = state;
    }

    fn push(&self, message: Message) {
        self.inner().inbox.push(message);
    }
}

/// An encoded frame as a DVC message.
struct Frame(Vec<u8>);

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
}

impl Endpoint {
    fn new(link: Link, capabilities: Capabilities, role: &str) -> Self {
        let agent = format!("Drift {} {} {role}", env!("CARGO_PKG_VERSION"), std::env::consts::OS);
        let ours =
            Hello { min_version: PROTOCOL_VERSIONS.0, max_version: PROTOCOL_VERSIONS.1, capabilities, agent };
        Self { link, ours }
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
                }
                Err(e) => self.fail(e.to_string()),
            },
            other if self.link.negotiated().is_some() => self.link.push(other),
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
}

impl_as_any!(DriftMacClient);

impl DvcProcessor for DriftMacClient {
    fn channel_name(&self) -> &str {
        CHANNEL_NAME
    }

    fn start(&mut self, _channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        Ok(self.0.start())
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
}

impl_as_any!(DriftMacServer);

impl DvcProcessor for DriftMacServer {
    fn channel_name(&self) -> &str {
        CHANNEL_NAME
    }

    fn start(&mut self, _channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        Ok(self.0.start())
    }

    fn process(&mut self, _channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        Ok(self.0.process(payload))
    }

    fn close(&mut self, _channel_id: u32) {
        self.0.link.set(LinkState::Closed);
    }
}

impl DvcServerProcessor for DriftMacServer {}
