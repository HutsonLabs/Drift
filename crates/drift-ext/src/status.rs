//! Host status and quality hints on `Drift::Mac::v1` (task **EXT-4**; ADR
//! `EXT-4-quality-hints`).
//!
//! ```text
//! client → host  QUALITY_PREFERENCE  max: u8 (QualityLevel), away: u8 (0 or 1)
//! host → client  HOST_STATUS         backlog: u8, quality: u8, bitrate_kbps: u32, fps: u8
//! ```
//!
//! The host decides its quality from its own backlog (HOST-7) and never goes above the client's
//! `max`. A client sends its preference as soon as both ends agreed on
//! [`Capabilities::STATUS`]; the host answers it, and sends a new status whenever its quality
//! changes.

use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};

use crate::Negotiated;
use crate::channel::Responder;
use crate::wire::{Capabilities, Message, WireError};

/// Message types of the EXT-4 range (`0x03xx`).
pub mod types {
    use crate::wire::MessageType;

    /// Host → client: backlog and the quality in use.
    pub const HOST_STATUS: MessageType = MessageType(0x0301);
    /// Client → host: the highest quality wanted, and whether the client is away from the host's
    /// local network.
    pub const QUALITY_PREFERENCE: MessageType = MessageType(0x0302);
}

/// A step of the host's quality ladder, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum QualityLevel {
    /// Full bit rate and frame rate.
    #[default]
    Best,
    /// A lower bit rate.
    High,
    /// Lower again, half the frame rate.
    Medium,
    /// The lowest step.
    Low,
}

const LEVELS: [QualityLevel; 4] =
    [QualityLevel::Best, QualityLevel::High, QualityLevel::Medium, QualityLevel::Low];

impl QualityLevel {
    /// One step lower (the lowest stays).
    pub const fn lower(self) -> Self {
        match self {
            Self::Best => Self::High,
            Self::High => Self::Medium,
            Self::Medium | Self::Low => Self::Low,
        }
    }

    /// One step higher (the best stays).
    pub const fn higher(self) -> Self {
        match self {
            Self::Best | Self::High => Self::Best,
            Self::Medium => Self::High,
            Self::Low => Self::Medium,
        }
    }

    /// The label shown in the statistics HUD.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Best => "best",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// How far behind the client the host sees itself (HOST-7's meter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Backlog {
    /// Keeping up.
    #[default]
    Clear,
    /// Falling behind.
    Building,
    /// Well behind.
    Backlogged,
}

const BACKLOGS: [Backlog; 3] = [Backlog::Clear, Backlog::Building, Backlog::Backlogged];

fn index<T: PartialEq>(table: &[T], value: T) -> u8 {
    table.iter().position(|v| *v == value).and_then(|i| u8::try_from(i).ok()).unwrap_or(0)
}

fn lookup<T: Copy>(table: &[T], byte: u8, what: &'static str) -> Result<T, WireError> {
    table.get(usize::from(byte)).copied().ok_or(WireError::Malformed(what))
}

/// What the host reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostStatus {
    /// The backlog level.
    pub backlog: Backlog,
    /// The quality in use.
    pub quality: QualityLevel,
    /// The encoder's average bit rate, kbit/s.
    pub bitrate_kbps: u32,
    /// The frame-rate cap.
    pub fps: u8,
}

impl HostStatus {
    /// As a channel message.
    pub fn to_message(&self) -> Message {
        let mut payload = vec![index(&BACKLOGS, self.backlog), index(&LEVELS, self.quality)];
        payload.extend(self.bitrate_kbps.to_le_bytes());
        payload.push(self.fps);
        Message::Unknown { kind: types::HOST_STATUS, payload }
    }

    /// The status in `message` (`None` for other types).
    pub fn from_message(message: &Message) -> Option<Result<Self, WireError>> {
        let Message::Unknown { kind: types::HOST_STATUS, payload: p } = message else { return None };
        Some(if p.len() == 7 {
            (|| {
                Ok(Self {
                    backlog: lookup(&BACKLOGS, p[0], "unknown backlog level")?,
                    quality: lookup(&LEVELS, p[1], "unknown quality level")?,
                    bitrate_kbps: u32::from_le_bytes([p[2], p[3], p[4], p[5]]),
                    fps: p[6],
                })
            })()
        } else {
            Err(WireError::Malformed("host status is 7 bytes"))
        })
    }
}

/// What the client asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QualityPreference {
    /// The highest quality the host may use.
    pub max: QualityLevel,
    /// The client is away from the host's local network.
    pub away: bool,
}

impl QualityPreference {
    /// The default: the best quality at home, [`QualityLevel::Medium`] at most when away (a
    /// cellular link or a VPN rarely carries the full rate, and a backlog takes seconds to drain).
    pub const fn default_for(away: bool) -> Self {
        Self { max: if away { QualityLevel::Medium } else { QualityLevel::Best }, away }
    }

    /// As a channel message.
    pub fn to_message(&self) -> Message {
        Message::Unknown {
            kind: types::QUALITY_PREFERENCE,
            payload: vec![index(&LEVELS, self.max), u8::from(self.away)],
        }
    }

    /// The preference in `message` (`None` for other types).
    pub fn from_message(message: &Message) -> Option<Result<Self, WireError>> {
        let Message::Unknown { kind: types::QUALITY_PREFERENCE, payload: p } = message else { return None };
        Some(match p.as_slice() {
            [max, away @ (0 | 1)] => {
                lookup(&LEVELS, *max, "unknown quality level").map(|max| Self { max, away: *away == 1 })
            }
            [_, _] => Err(WireError::Malformed("away is 0 or 1")),
            _ => Err(WireError::Malformed("quality preference is 2 bytes")),
        })
    }
}

/// Whether a host at `ip` is away from the client's local network: anything but loopback,
/// private (RFC 1918, IPv6 unique local), and link-local addresses. The shared address space
/// `100.64.0.0/10` counts as away: it is where VPNs such as Tailscale put their peers.
pub fn away_from(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !(v4.is_loopback() || v4.is_private() || v4.is_link_local()),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return away_from(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            let unique_local = first & 0xfe00 == 0xfc00;
            let link_local = first & 0xffc0 == 0xfe80;
            !(v6.is_loopback() || unique_local || link_local)
        }
    }
}

/// Whether a profile's `host` (an address or a name) is away from the local network: an IP
/// address by [`away_from`]; a single-label name or a `.local` (Bonjour) name is local; any other
/// name is taken as away, since a name in a public domain usually leads off the network.
pub fn away_from_host(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']').trim_end_matches('.');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return away_from(ip);
    }
    let lower = host.to_ascii_lowercase();
    !(lower.ends_with(".local")
        || !lower.contains('.')
        || lower.ends_with(".lan")
        || lower.ends_with(".home.arpa"))
}

/// The client's side: sends its preference once the host agreed on [`Capabilities::STATUS`] and
/// keeps the host's latest status. Cloning shares it.
#[derive(Debug, Clone)]
pub struct StatusClient {
    preference: QualityPreference,
    status: Arc<Mutex<Option<HostStatus>>>,
}

impl StatusClient {
    /// A client asking for `preference`.
    pub fn new(preference: QualityPreference) -> Self {
        Self { preference, status: Arc::new(Mutex::new(None)) }
    }

    /// What this client offers.
    pub const fn capabilities() -> Capabilities {
        Capabilities::STATUS
    }

    /// The host's latest status, if it sent one.
    pub fn host_status(&self) -> Option<HostStatus> {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The channel responder for one connection leg.
    pub fn responder(&self) -> Box<dyn Responder> {
        Box::new(StatusResponder(self.clone()))
    }
}

#[derive(Debug)]
struct StatusResponder(StatusClient);

impl Responder for StatusResponder {
    fn on_negotiated(&mut self, negotiated: &Negotiated) -> Vec<Message> {
        if negotiated.capabilities.contains(Capabilities::STATUS) {
            vec![self.0.preference.to_message()]
        } else {
            Vec::new()
        }
    }

    fn on_message(&mut self, message: &Message) -> Option<Vec<Message>> {
        match HostStatus::from_message(message)? {
            Ok(status) => *self.0.status.lock().unwrap_or_else(PoisonError::into_inner) = Some(status),
            Err(e) => tracing::warn!(error = %e, "dropped a malformed host status"),
        }
        Some(Vec::new())
    }
}

/// The host's side: hands each client preference to `on_preference`, which returns the status
/// to answer with.
pub struct StatusHost {
    on_preference: Box<dyn FnMut(QualityPreference) -> Option<HostStatus> + Send>,
}

impl std::fmt::Debug for StatusHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatusHost").finish_non_exhaustive()
    }
}

impl StatusHost {
    /// A host responder calling `on_preference` for every preference received.
    pub fn new(on_preference: impl FnMut(QualityPreference) -> Option<HostStatus> + Send + 'static) -> Self {
        Self { on_preference: Box::new(on_preference) }
    }
}

impl Responder for StatusHost {
    fn on_message(&mut self, message: &Message) -> Option<Vec<Message>> {
        match QualityPreference::from_message(message)? {
            Ok(p) => Some((self.on_preference)(p).map(|s| vec![s.to_message()]).unwrap_or_default()),
            Err(e) => {
                tracing::warn!(error = %e, "dropped a malformed quality preference");
                Some(Vec::new())
            }
        }
    }
}
