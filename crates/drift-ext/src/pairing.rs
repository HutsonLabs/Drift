//! Pairing and device authentication on `Drift::Mac::v1` (task **HOST-8**; ADR `IOS-0` decision 7,
//! ADRs `IOS-9-pairing` and `HOST-8-pairing`).
//!
//! The exchange, after both Hellos agreed on [`Capabilities::PAIRING`]:
//!
//! ```text
//! host   → Challenge { nonce }                        (fresh 32 random bytes per connection)
//! client → PairRequest { code, name, key, signature }  (first time: the code from the Mac's screen)
//!        | AuthRequest { key, signature }              (afterwards: the device key alone)
//! host   → PairReply { status } | AuthReply { status }
//! ```
//!
//! With [`Capabilities::APPROVAL`] (HOST-15) a device picked from the Bonjour list may send an
//! `ApprovalRequest { name, key, signature }` instead of a code. The host asks the Mac's user and
//! sends its `PairReply` when they decided (Allow, Don't Allow) or nobody did in time.
//!
//! The signature is ECDSA P-256 / SHA-256 over [`signed_bytes`]: a purpose label, the nonce and
//! the SHA-256 fingerprint of the host certificate the client pinned. The client has already
//! checked that certificate against its pin during TLS, so a signature made for one host and one
//! connection is worth nothing anywhere else.
//!
//! This module is pure: encoding and decoding ([`PairingMessage`]) and the client's side
//! ([`ClientPairing`]). The host's side lives in `drift-host`, which owns the code and the list of
//! paired devices.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::Negotiated;
use crate::channel::Responder;
use crate::wire::{Capabilities, Message, MessageType, WireError};

/// Length of the host's challenge.
pub const NONCE_LEN: usize = 32;

/// The pairing protocol version (the same number as the QR payload's, IOS-9).
pub const PAIRING_VERSION: u32 = 1;

/// Longest device name accepted (bytes of UTF-8).
pub const MAX_NAME: usize = 128;

/// Longest public key accepted (an uncompressed P-256 point is 65 bytes).
pub const MAX_KEY: usize = 133;

/// Longest signature accepted (a DER ECDSA P-256 signature is at most 72 bytes).
pub const MAX_SIGNATURE: usize = 80;

/// Message types of the pairing range (`0x04xx`).
pub mod types {
    use crate::wire::MessageType;

    /// Host → client: the nonce to sign.
    pub const CHALLENGE: MessageType = MessageType(0x0401);
    /// Client → host: pair with a code.
    pub const PAIR_REQUEST: MessageType = MessageType(0x0402);
    /// Client → host: prove the device key.
    pub const AUTH_REQUEST: MessageType = MessageType(0x0403);
    /// Host → client: the pairing's result.
    pub const PAIR_REPLY: MessageType = MessageType(0x0404);
    /// Host → client: the authentication's result.
    pub const AUTH_REPLY: MessageType = MessageType(0x0405);
    /// Client → host: pair once the Mac's user approves (HOST-15).
    pub const APPROVAL_REQUEST: MessageType = MessageType(0x0406);
}

/// The host's answer to a pairing request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PairStatus {
    /// The code matched; the device key is stored.
    Accepted,
    /// The code does not match the one on the Mac's screen.
    WrongCode,
    /// The code on the Mac's screen has expired.
    Expired,
    /// Too many wrong codes; pairing is locked for a while.
    TooManyAttempts,
    /// The Mac is not showing a code.
    NotPairing,
    /// The Mac's user chose Don't Allow (HOST-15).
    Declined,
    /// Nobody answered on the Mac in time (HOST-15).
    NoAnswer,
}

/// The host's answer to an authentication request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthStatus {
    /// The device is paired and proved its key.
    Ok,
    /// No paired device has this key.
    UnknownDevice,
    /// The signature does not verify.
    BadSignature,
}

/// What a signature is for (part of what is signed, so one cannot stand in for the other).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// A pairing request.
    Pair,
    /// An authentication request.
    Authenticate,
    /// A request for the Mac's user to approve the device (HOST-15).
    Approve,
}

/// The bytes a device signs: a purpose label, the host's nonce and the pinned certificate's
/// SHA-256 fingerprint.
pub fn signed_bytes(purpose: Purpose, nonce: &[u8; NONCE_LEN], host_fingerprint: &[u8; 32]) -> Vec<u8> {
    let label: &[u8] = match purpose {
        Purpose::Pair => b"drift-pair-v1\0",
        Purpose::Authenticate => b"drift-auth-v1\0",
        Purpose::Approve => b"drift-approve-v1\0",
    };
    [label, nonce.as_slice(), host_fingerprint.as_slice()].concat()
}

/// One pairing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingMessage {
    /// [`types::CHALLENGE`].
    Challenge {
        /// The bytes to sign.
        nonce: [u8; NONCE_LEN],
    },
    /// [`types::PAIR_REQUEST`].
    PairRequest {
        /// [`PAIRING_VERSION`].
        version: u32,
        /// The eight digits the user saw on the Mac.
        code: String,
        /// The device's name, for the Mac's list of paired devices.
        device_name: String,
        /// The device's public key (uncompressed P-256 point).
        device_key: Vec<u8>,
        /// The signature of [`signed_bytes`] with [`Purpose::Pair`].
        signature: Vec<u8>,
    },
    /// [`types::AUTH_REQUEST`].
    AuthRequest {
        /// The device's public key.
        device_key: Vec<u8>,
        /// The signature of [`signed_bytes`] with [`Purpose::Authenticate`].
        signature: Vec<u8>,
    },
    /// [`types::APPROVAL_REQUEST`].
    ApprovalRequest {
        /// [`PAIRING_VERSION`].
        version: u32,
        /// The device's name, shown in the Mac's prompt and list of paired devices.
        device_name: String,
        /// The device's public key (uncompressed P-256 point).
        device_key: Vec<u8>,
        /// The signature of [`signed_bytes`] with [`Purpose::Approve`].
        signature: Vec<u8>,
    },
    /// [`types::PAIR_REPLY`].
    PairReply(PairStatus),
    /// [`types::AUTH_REPLY`].
    AuthReply(AuthStatus),
}

impl PairingMessage {
    /// The message as a channel message.
    pub fn to_message(&self) -> Message {
        let (kind, payload) = match self {
            Self::Challenge { nonce } => (types::CHALLENGE, nonce.to_vec()),
            Self::PairRequest { version, code, device_name, device_key, signature } => {
                let mut p = version.to_le_bytes().to_vec();
                put(&mut p, code.as_bytes(), 1);
                put(&mut p, device_name.as_bytes(), 2);
                put(&mut p, device_key, 2);
                put(&mut p, signature, 2);
                (types::PAIR_REQUEST, p)
            }
            Self::AuthRequest { device_key, signature } => {
                let mut p = Vec::new();
                put(&mut p, device_key, 2);
                put(&mut p, signature, 2);
                (types::AUTH_REQUEST, p)
            }
            Self::ApprovalRequest { version, device_name, device_key, signature } => {
                let mut p = version.to_le_bytes().to_vec();
                put(&mut p, device_name.as_bytes(), 2);
                put(&mut p, device_key, 2);
                put(&mut p, signature, 2);
                (types::APPROVAL_REQUEST, p)
            }
            Self::PairReply(status) => (types::PAIR_REPLY, vec![pair_status_byte(*status)]),
            Self::AuthReply(status) => (types::AUTH_REPLY, vec![auth_status_byte(*status)]),
        };
        Message::Unknown { kind, payload }
    }

    /// The pairing message in `message`: `None` for another range, `Some(Err)` for a malformed
    /// payload.
    pub fn from_message(message: &Message) -> Option<Result<Self, WireError>> {
        let Message::Unknown { kind, payload } = message else { return None };
        if kind.0 >> 8 != 0x04 {
            return None;
        }
        Some(decode(*kind, payload))
    }
}

/// Appends `bytes` with a little-endian length prefix of `width` bytes (1 or 2).
fn put(out: &mut Vec<u8>, bytes: &[u8], width: usize) {
    let len = bytes.len();
    if width == 1 {
        out.push(u8::try_from(len).unwrap_or(u8::MAX));
    } else {
        out.extend(u16::try_from(len).unwrap_or(u16::MAX).to_le_bytes());
    }
    out.extend(bytes);
}

/// A cursor over a payload that never panics.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.0.len() < n {
            return Err(WireError::Malformed("pairing message shorter than its fields"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn field(&mut self, width: usize, max: usize) -> Result<&'a [u8], WireError> {
        let len = match width {
            1 => usize::from(self.take(1)?[0]),
            _ => {
                let b = self.take(2)?;
                usize::from(u16::from_le_bytes([b[0], b[1]]))
            }
        };
        if len > max {
            return Err(WireError::Malformed("pairing field too long"));
        }
        self.take(len)
    }

    fn end(&self) -> Result<(), WireError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(WireError::Malformed("trailing bytes in a pairing message"))
        }
    }
}

fn utf8(bytes: &[u8]) -> Result<String, WireError> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| WireError::Malformed("pairing text is not UTF-8"))
}

fn decode(kind: MessageType, payload: &[u8]) -> Result<PairingMessage, WireError> {
    let mut r = Reader(payload);
    let message = match kind {
        types::CHALLENGE => {
            let mut nonce = [0_u8; NONCE_LEN];
            nonce.copy_from_slice(r.take(NONCE_LEN)?);
            PairingMessage::Challenge { nonce }
        }
        types::PAIR_REQUEST => {
            let v = r.take(4)?;
            let version = u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
            let code = utf8(r.field(1, 16)?)?;
            let device_name = utf8(r.field(2, MAX_NAME)?)?;
            let device_key = r.field(2, MAX_KEY)?.to_vec();
            let signature = r.field(2, MAX_SIGNATURE)?.to_vec();
            PairingMessage::PairRequest { version, code, device_name, device_key, signature }
        }
        types::AUTH_REQUEST => {
            let device_key = r.field(2, MAX_KEY)?.to_vec();
            let signature = r.field(2, MAX_SIGNATURE)?.to_vec();
            PairingMessage::AuthRequest { device_key, signature }
        }
        types::APPROVAL_REQUEST => {
            let v = r.take(4)?;
            let version = u32::from_le_bytes([v[0], v[1], v[2], v[3]]);
            let device_name = utf8(r.field(2, MAX_NAME)?)?;
            let device_key = r.field(2, MAX_KEY)?.to_vec();
            let signature = r.field(2, MAX_SIGNATURE)?.to_vec();
            PairingMessage::ApprovalRequest { version, device_name, device_key, signature }
        }
        types::PAIR_REPLY => PairingMessage::PairReply(match r.take(1)?[0] {
            0 => PairStatus::Accepted,
            1 => PairStatus::WrongCode,
            2 => PairStatus::Expired,
            3 => PairStatus::TooManyAttempts,
            4 => PairStatus::NotPairing,
            5 => PairStatus::Declined,
            6 => PairStatus::NoAnswer,
            _ => return Err(WireError::Malformed("unknown pairing status")),
        }),
        types::AUTH_REPLY => PairingMessage::AuthReply(match r.take(1)?[0] {
            0 => AuthStatus::Ok,
            1 => AuthStatus::UnknownDevice,
            2 => AuthStatus::BadSignature,
            _ => return Err(WireError::Malformed("unknown authentication status")),
        }),
        _ => return Err(WireError::Malformed("unknown pairing message type")),
    };
    r.end()?;
    Ok(message)
}

const fn pair_status_byte(status: PairStatus) -> u8 {
    match status {
        PairStatus::Accepted => 0,
        PairStatus::WrongCode => 1,
        PairStatus::Expired => 2,
        PairStatus::TooManyAttempts => 3,
        PairStatus::NotPairing => 4,
        PairStatus::Declined => 5,
        PairStatus::NoAnswer => 6,
    }
}

const fn auth_status_byte(status: AuthStatus) -> u8 {
    match status {
        AuthStatus::Ok => 0,
        AuthStatus::UnknownDevice => 1,
        AuthStatus::BadSignature => 2,
    }
}

// ---- the client's side --------------------------------------------------------------------------

/// The device's key pair, as far as pairing needs it.
pub trait DeviceSigner: Send + Sync {
    /// The public key (uncompressed P-256 point).
    fn public_key(&self) -> Vec<u8>;
    /// An ECDSA P-256 SHA-256 signature (ASN.1 DER) of `message`, or `None` if signing failed.
    fn sign(&self, message: &[u8]) -> Option<Vec<u8>>;
}

/// How the latest exchange ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingOutcome {
    /// The Mac accepted the code and stored the device key.
    Paired,
    /// The Mac recognised the device key.
    Authenticated,
    /// The Mac refused the pairing.
    PairRefused(PairStatus),
    /// The Mac refused the device.
    AuthRefused(AuthStatus),
    /// The device asked for approval, but the Mac's Drift Host cannot approve (HOST-15).
    ApprovalUnsupported,
}

impl PairingOutcome {
    /// What to tell the user when the Mac refused, `None` otherwise.
    pub fn refusal(self) -> Option<&'static str> {
        Some(match self {
            Self::Paired | Self::Authenticated => return None,
            Self::PairRefused(PairStatus::WrongCode) => "The Mac did not accept the code.",
            Self::PairRefused(PairStatus::Expired) => {
                "The code on the Mac has expired. Start pairing again on the Mac."
            }
            Self::PairRefused(PairStatus::TooManyAttempts) => {
                "Too many wrong codes. Wait a minute, then start again on the Mac."
            }
            Self::PairRefused(PairStatus::NotPairing) => {
                "The Mac is not showing a pairing code. Start pairing in Drift Host first."
            }
            Self::PairRefused(PairStatus::Declined) => "The Mac declined this device.",
            Self::PairRefused(PairStatus::NoAnswer) => {
                "Nobody clicked Allow on the Mac in time. Connect again and click Allow in Drift Host."
            }
            Self::ApprovalUnsupported => {
                "This Mac's Drift Host is older and cannot approve devices. Pair with the code it shows."
            }
            Self::PairRefused(PairStatus::Accepted) | Self::AuthRefused(AuthStatus::Ok) => return None,
            Self::AuthRefused(AuthStatus::UnknownDevice) => {
                "This device is not paired with this Mac. Pair it again from Drift Host."
            }
            Self::AuthRefused(AuthStatus::BadSignature) => {
                "The Mac could not verify this device's key. Pair it again from Drift Host."
            }
        })
    }
}

struct ClientInner {
    signer: Arc<dyn DeviceSigner>,
    host_fingerprint: [u8; 32],
    device_name: String,
    code: Option<String>,
    /// Asks the Mac's user instead of sending a code (HOST-15).
    approval: bool,
    /// An approval request was sent and the Mac has not answered yet.
    awaiting: AtomicBool,
    outcome: Mutex<Option<PairingOutcome>>,
}

/// The client's pairing state for one connection profile, shared by every connection leg of its
/// session (clones share it). It pairs with the code once; after the Mac accepted, reconnects
/// authenticate with the device key alone.
#[derive(Clone)]
pub struct ClientPairing(Arc<ClientInner>);

impl fmt::Debug for ClientPairing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientPairing")
            .field("device_name", &self.0.device_name)
            .field("has_code", &self.0.code.is_some())
            .field("approval", &self.0.approval)
            .field("outcome", &self.outcome())
            .finish_non_exhaustive()
    }
}

impl PartialEq for ClientPairing {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ClientPairing {}

impl ClientPairing {
    /// Authenticates as `signer`'s key with the host whose pinned certificate has
    /// `host_fingerprint`, introducing the device as `device_name` when pairing.
    pub fn new(
        signer: Arc<dyn DeviceSigner>,
        host_fingerprint: [u8; 32],
        device_name: impl Into<String>,
    ) -> Self {
        Self(Arc::new(ClientInner {
            signer,
            host_fingerprint,
            device_name: device_name.into(),
            code: None,
            approval: false,
            awaiting: AtomicBool::new(false),
            outcome: Mutex::new(None),
        }))
    }

    fn rebuilt(&self, code: Option<String>, approval: bool) -> Self {
        let inner = &self.0;
        Self(Arc::new(ClientInner {
            signer: Arc::clone(&inner.signer),
            host_fingerprint: inner.host_fingerprint,
            device_name: inner.device_name.clone(),
            code,
            approval,
            awaiting: AtomicBool::new(false),
            outcome: Mutex::new(*inner.outcome.lock().unwrap_or_else(PoisonError::into_inner)),
        }))
    }

    /// Pairs with `code` (the digits on the Mac's screen) instead of authenticating. A code
    /// replaces asking for approval.
    #[must_use]
    pub fn with_code(self, code: impl Into<String>) -> Self {
        self.rebuilt(Some(code.into()), false)
    }

    /// Pairs by asking the Mac's user to approve this device (HOST-15) instead of
    /// authenticating.
    #[must_use]
    pub fn asking_approval(self) -> Self {
        self.rebuilt(None, true)
    }

    /// The code this pairing sends, if it still pairs.
    pub fn code(&self) -> Option<&str> {
        self.0.code.as_deref()
    }

    /// Whether this pairing asks the Mac's user (until the Mac accepted once).
    pub fn asks_approval(&self) -> bool {
        self.0.approval
            && !matches!(self.outcome(), Some(PairingOutcome::Paired | PairingOutcome::Authenticated))
    }

    /// Whether the Mac's user has been asked and has not answered yet: the device should tell
    /// its user to click Allow on the Mac.
    pub fn awaiting_approval(&self) -> bool {
        self.0.awaiting.load(Ordering::Relaxed)
    }

    /// The pinned host certificate's fingerprint the signatures name.
    pub fn host_fingerprint(&self) -> &[u8; 32] {
        &self.0.host_fingerprint
    }

    /// How the latest exchange ended (`None` before the Mac answered).
    pub fn outcome(&self) -> Option<PairingOutcome> {
        *self.0.outcome.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What to tell the user if the Mac refused.
    pub fn refusal(&self) -> Option<String> {
        self.outcome().and_then(PairingOutcome::refusal).map(str::to_owned)
    }

    /// The capabilities a client with this pairing offers.
    pub const fn capabilities() -> Capabilities {
        Capabilities::from_bits(Capabilities::PAIRING.bits() | Capabilities::APPROVAL.bits())
    }

    /// The channel responder for one connection leg. A refusal from an earlier leg is forgotten,
    /// so it never explains a later failure; a successful pairing is kept (reconnects
    /// authenticate).
    pub fn responder(&self) -> Box<dyn Responder> {
        let mut outcome = self.0.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        if outcome.is_some_and(|o| o.refusal().is_some()) {
            *outcome = None;
        }
        drop(outcome);
        self.0.awaiting.store(false, Ordering::Relaxed);
        Box::new(ClientResponder { pairing: self.clone(), host_approves: false })
    }

    fn set_outcome(&self, outcome: PairingOutcome) {
        self.0.awaiting.store(false, Ordering::Relaxed);
        *self.0.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
    }

    /// Pairs only until the Mac accepted once.
    fn pairs(&self) -> bool {
        self.0.code.is_some()
            && !matches!(self.outcome(), Some(PairingOutcome::Paired | PairingOutcome::Authenticated))
    }

    fn answer(&self, nonce: &[u8; NONCE_LEN]) -> Option<PairingMessage> {
        let inner = &self.0;
        let device_key = inner.signer.public_key();
        if self.asks_approval() {
            let signature =
                inner.signer.sign(&signed_bytes(Purpose::Approve, nonce, &inner.host_fingerprint))?;
            inner.awaiting.store(true, Ordering::Relaxed);
            Some(PairingMessage::ApprovalRequest {
                version: PAIRING_VERSION,
                device_name: inner.device_name.chars().take(64).collect(),
                device_key,
                signature,
            })
        } else if self.pairs() {
            let signature =
                inner.signer.sign(&signed_bytes(Purpose::Pair, nonce, &inner.host_fingerprint))?;
            Some(PairingMessage::PairRequest {
                version: PAIRING_VERSION,
                code: inner.code.clone().unwrap_or_default(),
                device_name: inner.device_name.chars().take(64).collect(),
                device_key,
                signature,
            })
        } else {
            let signature =
                inner.signer.sign(&signed_bytes(Purpose::Authenticate, nonce, &inner.host_fingerprint))?;
            Some(PairingMessage::AuthRequest { device_key, signature })
        }
    }
}

#[derive(Debug)]
struct ClientResponder {
    pairing: ClientPairing,
    /// The host agreed on [`Capabilities::APPROVAL`].
    host_approves: bool,
}

impl Responder for ClientResponder {
    fn on_negotiated(&mut self, negotiated: &Negotiated) -> Vec<Message> {
        self.host_approves = negotiated.capabilities.contains(Capabilities::APPROVAL);
        // The host speaks first: its challenge carries the nonce to sign.
        Vec::new()
    }

    fn on_message(&mut self, message: &Message) -> Option<Vec<Message>> {
        let parsed = PairingMessage::from_message(message)?;
        let Ok(parsed) = parsed else {
            tracing::warn!("malformed pairing message from the host");
            return Some(Vec::new());
        };
        match parsed {
            PairingMessage::Challenge { .. } if self.pairing.asks_approval() && !self.host_approves => {
                // An older host would take the request for nothing; say why instead.
                self.pairing.set_outcome(PairingOutcome::ApprovalUnsupported);
                Some(Vec::new())
            }
            PairingMessage::Challenge { nonce } => match self.pairing.answer(&nonce) {
                Some(answer) => Some(vec![answer.to_message()]),
                None => {
                    tracing::warn!("the device key could not sign the host's challenge");
                    Some(Vec::new())
                }
            },
            PairingMessage::PairReply(status) => {
                self.pairing.set_outcome(match status {
                    PairStatus::Accepted => PairingOutcome::Paired,
                    other => PairingOutcome::PairRefused(other),
                });
                Some(Vec::new())
            }
            PairingMessage::AuthReply(status) => {
                self.pairing.set_outcome(match status {
                    AuthStatus::Ok => PairingOutcome::Authenticated,
                    other => PairingOutcome::AuthRefused(other),
                });
                Some(Vec::new())
            }
            // Requests are the host's to answer.
            PairingMessage::PairRequest { .. }
            | PairingMessage::AuthRequest { .. }
            | PairingMessage::ApprovalRequest { .. } => Some(Vec::new()),
        }
    }
}
