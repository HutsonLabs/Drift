//! `Drift::Mac::v1` framing (task **EXT-1**). Pure and strict.
//!
//! Every message is one DVC payload (the DRDYNVC layer reassembles fragments):
//!
//! ```text
//! offset  size  field
//!      0     4  magic "DRFT"
//!      4     1  version (1)
//!      5     1  flags, reserved, must be 0
//!      6     2  message type (u16 LE)
//!      8     4  payload length (u32 LE), at most MAX_PAYLOAD, and exactly what follows
//!     12     n  payload
//! ```
//!
//! Types are grouped by their high byte, one range per task ([`MessageType::area`]). This crate
//! decodes only the negotiation message ([`Hello`]); every other type is kept as
//! [`Message::Unknown`] so a newer peer's messages pass through unharmed until the task that
//! owns them decodes them.

/// The four magic bytes.
pub const MAGIC: [u8; 4] = *b"DRFT";

/// The framing version this crate writes and reads.
pub const VERSION: u8 = 1;

/// Header size in bytes.
pub const HEADER_LEN: usize = 12;

/// The largest payload accepted or written (1 MiB; a mirrored menu bar is the biggest planned
/// message, EXT-3).
pub const MAX_PAYLOAD: usize = 1 << 20;

/// The largest agent string in a [`Hello`].
pub const MAX_AGENT: usize = 256;

/// A message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MessageType(pub u16);

impl MessageType {
    /// Version and capability exchange, both directions.
    pub const HELLO: Self = Self(0x0001);

    /// The task range this type belongs to.
    pub const fn area(self) -> &'static str {
        match self.0 >> 8 {
            0x00 => "negotiation",
            0x01 => "scroll and gestures (EXT-2)",
            0x02 => "menu mirroring (EXT-3)",
            0x03 => "host status and quality (EXT-4)",
            0x04 => "pairing (HOST-8)",
            0x05 => "codec negotiation (EXT-5)",
            _ => "unassigned",
        }
    }
}

/// What a peer can do, as a bit set. Unknown bits are kept (a newer peer's features) and simply
/// never intersect with ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Capabilities(u32);

impl Capabilities {
    /// Nothing beyond the handshake.
    pub const NONE: Self = Self(0);
    /// Scroll with phase and momentum (EXT-2).
    pub const SCROLL: Self = Self(1);
    /// Trackpad gestures (EXT-2).
    pub const GESTURES: Self = Self(1 << 1);
    /// Menu mirroring (EXT-3).
    pub const MENUS: Self = Self(1 << 2);
    /// Host status and quality hints (EXT-4).
    pub const STATUS: Self = Self(1 << 3);
    /// Device pairing (HOST-8).
    pub const PAIRING: Self = Self(1 << 4);
    /// Extension codecs: HEVC and HDR (EXT-5).
    pub const CODEC: Self = Self(1 << 5);
    /// Pairing approved on the Mac instead of a typed code (HOST-15).
    pub const APPROVAL: Self = Self(1 << 6);

    /// From raw bits (unknown bits kept).
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The raw bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether every capability of `other` is present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Capabilities {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for Capabilities {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

/// The negotiation message: the versions a peer speaks and what it can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// Oldest protocol version spoken.
    pub min_version: u16,
    /// Newest protocol version spoken.
    pub max_version: u16,
    /// Features offered.
    pub capabilities: Capabilities,
    /// Free text for logs, e.g. `Drift 0.2.0 ios` (at most [`MAX_AGENT`] bytes of UTF-8).
    pub agent: String,
}

/// One decoded message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// [`MessageType::HELLO`].
    Hello(Hello),
    /// Any other type, payload untouched.
    Unknown {
        /// Its type.
        kind: MessageType,
        /// Its payload.
        payload: Vec<u8>,
    },
}

impl Message {
    /// The message's type.
    pub const fn kind(&self) -> MessageType {
        match self {
            Self::Hello(_) => MessageType::HELLO,
            Self::Unknown { kind, .. } => *kind,
        }
    }
}

/// Why a frame was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    /// Fewer bytes than a header.
    #[error("truncated frame: {got} of at least {needed} bytes")]
    Truncated {
        /// Bytes needed.
        needed: usize,
        /// Bytes present.
        got: usize,
    },
    /// Not `DRFT`.
    #[error("bad magic")]
    BadMagic,
    /// A framing version this build does not read.
    #[error("unsupported framing version {0}")]
    UnsupportedVersion(u8),
    /// Reserved flag bits set.
    #[error("reserved flags {0:#04x} set")]
    ReservedFlags(u8),
    /// The declared length is not what follows the header.
    #[error("declared payload of {declared} bytes, {actual} present")]
    LengthMismatch {
        /// Declared length.
        declared: usize,
        /// Bytes after the header.
        actual: usize,
    },
    /// Over [`MAX_PAYLOAD`] (or an agent over [`MAX_AGENT`]).
    #[error("message of {0} bytes is too large")]
    TooLarge(usize),
    /// A payload that does not parse.
    #[error("malformed payload: {0}")]
    Malformed(&'static str),
}

/// Encodes one message (header + payload).
pub fn encode(message: &Message) -> Result<Vec<u8>, WireError> {
    let payload = match message {
        Message::Hello(h) => {
            let agent = h.agent.as_bytes();
            if agent.len() > MAX_AGENT {
                return Err(WireError::TooLarge(agent.len()));
            }
            let mut p = Vec::with_capacity(10 + agent.len());
            p.extend(h.min_version.to_le_bytes());
            p.extend(h.max_version.to_le_bytes());
            p.extend(h.capabilities.bits().to_le_bytes());
            p.extend(u16::try_from(agent.len()).map_err(|_| WireError::TooLarge(agent.len()))?.to_le_bytes());
            p.extend(agent);
            p
        }
        Message::Unknown { payload, .. } => payload.clone(),
    };
    if payload.len() > MAX_PAYLOAD {
        return Err(WireError::TooLarge(payload.len()));
    }
    let len = u32::try_from(payload.len()).map_err(|_| WireError::TooLarge(payload.len()))?;
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend(MAGIC);
    out.extend([VERSION, 0]);
    out.extend(message.kind().0.to_le_bytes());
    out.extend(len.to_le_bytes());
    out.extend(payload);
    Ok(out)
}

/// Decodes exactly one message from `frame`.
pub fn decode(frame: &[u8]) -> Result<Message, WireError> {
    let header: &[u8; HEADER_LEN] = frame
        .get(..HEADER_LEN)
        .and_then(|h| h.try_into().ok())
        .ok_or(WireError::Truncated { needed: HEADER_LEN, got: frame.len() })?;
    if header[..4] != MAGIC {
        return Err(WireError::BadMagic);
    }
    if header[4] != VERSION {
        return Err(WireError::UnsupportedVersion(header[4]));
    }
    if header[5] != 0 {
        return Err(WireError::ReservedFlags(header[5]));
    }
    let kind = MessageType(u16::from_le_bytes([header[6], header[7]]));
    let declared = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let declared = usize::try_from(declared).map_err(|_| WireError::TooLarge(usize::MAX))?;
    if declared > MAX_PAYLOAD {
        return Err(WireError::TooLarge(declared));
    }
    let payload = &frame[HEADER_LEN..];
    if payload.len() != declared {
        return Err(WireError::LengthMismatch { declared, actual: payload.len() });
    }
    match kind {
        MessageType::HELLO => decode_hello(payload).map(Message::Hello),
        _ => Ok(Message::Unknown { kind, payload: payload.to_vec() }),
    }
}

fn decode_hello(p: &[u8]) -> Result<Hello, WireError> {
    let fixed = p.get(..10).ok_or(WireError::Malformed("hello shorter than its fixed part"))?;
    let min_version = u16::from_le_bytes([fixed[0], fixed[1]]);
    let max_version = u16::from_le_bytes([fixed[2], fixed[3]]);
    let capabilities = Capabilities::from_bits(u32::from_le_bytes([fixed[4], fixed[5], fixed[6], fixed[7]]));
    let agent_len = usize::from(u16::from_le_bytes([fixed[8], fixed[9]]));
    if agent_len > MAX_AGENT {
        return Err(WireError::Malformed("agent longer than allowed"));
    }
    let rest = &p[10..];
    if rest.len() != agent_len {
        return Err(WireError::Malformed("agent length does not match the payload"));
    }
    let agent = std::str::from_utf8(rest).map_err(|_| WireError::Malformed("agent is not UTF-8"))?.to_owned();
    Ok(Hello { min_version, max_version, capabilities, agent })
}
