//! Codec negotiation on `Drift::Mac::v1` (task **EXT-5**; ADR `IOS-0` decision 2, ADR
//! `EXT-5-hevc-hdr`).
//!
//! Once both Hellos agreed on [`Capabilities::CODEC`]:
//!
//! ```text
//! client → Offer { hevc profiles it decodes, transfer functions its display shows }
//! host   → Select { the profile it will send (or none), the transfer }
//! ```
//!
//! The host picks with [`select`] from what its encoder has and the picture's size, and sends the
//! frames as `WireToSurface1` with Drift's private codec id; the client decodes whatever HEVC
//! arrives (the bitstream says its profile), so the `Select` is the host's statement of intent.
//! Without the capability, or without an answer, the session stays on AVC444/AVC420.
//!
//! Payloads:
//!
//! ```text
//! Offer:  n:u8 · n × profile:u8 · m:u8 · m × transfer:u8
//! Select: has_hevc:u8 (0|1) · profile:u8 · transfer:u8
//! ```

use std::sync::{Arc, Mutex, PoisonError};

pub use drift_core::video::{HevcProfile, Transfer};

use crate::Negotiated;
use crate::channel::Responder;
use crate::wire::{Capabilities, Message, MessageType, WireError};

/// The most profiles (or transfer functions) one offer may list.
pub const MAX_PROFILES: usize = 8;

/// Message types of the codec range (`0x05xx`).
pub mod types {
    use crate::wire::MessageType;

    /// Client → host: what the client decodes and shows.
    pub const OFFER: MessageType = MessageType(0x0501);
    /// Host → client: what the host will send.
    pub const SELECT: MessageType = MessageType(0x0502);
}

/// What a client decodes and shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodecOffer {
    /// HEVC profiles the client decodes.
    pub hevc: Vec<HevcProfile>,
    /// Transfer functions the client's display shows (`Sdr` always; `Pq`/`Hlg` on HDR screens).
    pub transfers: Vec<Transfer>,
}

/// What the host will send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecSelect {
    /// The HEVC profile, or `None` for H.264 (AVC444/AVC420).
    pub hevc: Option<HevcProfile>,
    /// The transfer function of the pictures.
    pub transfer: Transfer,
}

/// One message of the codec range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecMessage {
    /// [`types::OFFER`].
    Offer(CodecOffer),
    /// [`types::SELECT`].
    Select(CodecSelect),
}

impl CodecMessage {
    /// As a channel message.
    pub fn to_message(&self) -> Message {
        match self {
            Self::Offer(o) => {
                let mut p = Vec::with_capacity(2 + o.hevc.len() + o.transfers.len());
                let hevc: Vec<u8> = o.hevc.iter().take(MAX_PROFILES).map(|h| h.id()).collect();
                let transfers: Vec<u8> = o.transfers.iter().take(MAX_PROFILES).map(|t| t.id()).collect();
                p.push(u8::try_from(hevc.len()).unwrap_or(0));
                p.extend(hevc);
                p.push(u8::try_from(transfers.len()).unwrap_or(0));
                p.extend(transfers);
                Message::Unknown { kind: types::OFFER, payload: p }
            }
            Self::Select(s) => Message::Unknown {
                kind: types::SELECT,
                payload: vec![u8::from(s.hevc.is_some()), s.hevc.map_or(0, HevcProfile::id), s.transfer.id()],
            },
        }
    }

    /// The codec message in `message`: `None` for another type, `Some(Err)` for a malformed one.
    pub fn from_message(message: &Message) -> Option<Result<Self, WireError>> {
        let Message::Unknown { kind, payload } = message else { return None };
        match *kind {
            types::OFFER => Some(decode_offer(payload).map(Self::Offer)),
            types::SELECT => Some(decode_select(payload).map(Self::Select)),
            _ => None,
        }
    }
}

fn list<T>(p: &[u8], at: usize, from: fn(u8) -> Option<T>) -> Result<(Vec<T>, usize), WireError> {
    let n = usize::from(*p.get(at).ok_or(WireError::Malformed("codec list count missing"))?);
    if n > MAX_PROFILES {
        return Err(WireError::Malformed("codec list too long"));
    }
    let ids = p.get(at + 1..at + 1 + n).ok_or(WireError::Malformed("codec list shorter than its count"))?;
    let items = ids
        .iter()
        .map(|&id| from(id).ok_or(WireError::Malformed("unknown codec id")))
        .collect::<Result<_, _>>()?;
    Ok((items, at + 1 + n))
}

fn decode_offer(p: &[u8]) -> Result<CodecOffer, WireError> {
    let (hevc, next) = list(p, 0, HevcProfile::from_id)?;
    let (transfers, end) = list(p, next, Transfer::from_id)?;
    if end != p.len() {
        return Err(WireError::Malformed("bytes after the codec offer"));
    }
    Ok(CodecOffer { hevc, transfers })
}

fn decode_select(p: &[u8]) -> Result<CodecSelect, WireError> {
    let [has, profile, transfer] = p else { return Err(WireError::Malformed("codec select is three bytes")) };
    let hevc = match has {
        0 => None,
        1 => Some(HevcProfile::from_id(*profile).ok_or(WireError::Malformed("unknown HEVC profile"))?),
        _ => return Err(WireError::Malformed("codec select flag is 0 or 1")),
    };
    let transfer = Transfer::from_id(*transfer).ok_or(WireError::Malformed("unknown transfer"))?;
    Ok(CodecSelect { hevc, transfer })
}

/// What the host's encoder and screen can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCodecs {
    /// HEVC profiles the encoder has (probed at launch).
    pub hevc: Vec<HevcProfile>,
    /// Transfer functions the captured screen has (`Sdr`; `Pq` on an HDR display).
    pub transfers: Vec<Transfer>,
    /// The largest picture (pixels) sent as HEVC 4:2:0 at full rate.
    pub max_pixels: u64,
    /// The largest picture (pixels) sent as HEVC 4:4:4 at full rate.
    pub max_444_pixels: u64,
}

/// The host's choice for a `(width, height)` picture: full chroma when both ends have it and it
/// fits, HDR (PQ before HLG) when both ends show it and a 10-bit profile is shared, else SDR.
pub fn select(offer: &CodecOffer, host: &HostCodecs, size: (u32, u32)) -> CodecSelect {
    let pixels = u64::from(size.0) * u64::from(size.1);
    let both = |p: HevcProfile| offer.hevc.contains(&p) && host.hevc.contains(&p);
    let hdr = [Transfer::Pq, Transfer::Hlg]
        .into_iter()
        .find(|t| offer.transfers.contains(t) && host.transfers.contains(t));
    let fits = |p: HevcProfile| pixels <= if p.full_chroma() { host.max_444_pixels } else { host.max_pixels };
    let pick = |ten_bit: bool| {
        let order: [HevcProfile; 2] = if ten_bit {
            [HevcProfile::Main44410, HevcProfile::Main10]
        } else {
            [HevcProfile::Main444, HevcProfile::Main]
        };
        order.into_iter().find(|&p| both(p) && fits(p))
    };
    if let Some(transfer) = hdr
        && let Some(profile) = pick(true)
    {
        return CodecSelect { hevc: Some(profile), transfer };
    }
    CodecSelect { hevc: pick(false), transfer: Transfer::Sdr }
}

#[derive(Debug)]
struct ClientInner {
    offer: CodecOffer,
    selection: Mutex<Option<CodecSelect>>,
}

/// The client's side: offers once the codec capability is agreed and keeps the host's answer.
/// Cloning shares it.
#[derive(Debug, Clone)]
pub struct ClientCodec(Arc<ClientInner>);

impl ClientCodec {
    /// A client that offers `offer`.
    pub fn new(offer: CodecOffer) -> Self {
        Self(Arc::new(ClientInner { offer, selection: Mutex::new(None) }))
    }

    /// The capabilities a client with codec negotiation offers.
    pub const fn capabilities() -> Capabilities {
        Capabilities::CODEC
    }

    /// What the host said it will send (`None` before it answered).
    pub fn selection(&self) -> Option<CodecSelect> {
        *self.0.selection.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The channel responder for one connection leg.
    pub fn responder(&self) -> Box<dyn Responder> {
        Box::new(ClientResponder(self.clone()))
    }
}

#[derive(Debug)]
struct ClientResponder(ClientCodec);

impl Responder for ClientResponder {
    fn on_negotiated(&mut self, negotiated: &Negotiated) -> Vec<Message> {
        *self.0.0.selection.lock().unwrap_or_else(PoisonError::into_inner) = None;
        if negotiated.capabilities.contains(Capabilities::CODEC) {
            vec![CodecMessage::Offer(self.0.0.offer.clone()).to_message()]
        } else {
            Vec::new()
        }
    }

    fn on_message(&mut self, message: &Message) -> Option<Vec<Message>> {
        match CodecMessage::from_message(message)? {
            Ok(CodecMessage::Select(s)) => {
                tracing::info!(hevc = ?s.hevc, transfer = ?s.transfer, "the host picked its codec");
                *self.0.0.selection.lock().unwrap_or_else(PoisonError::into_inner) = Some(s);
            }
            Ok(CodecMessage::Offer(_)) => tracing::warn!("ignoring a codec offer from the host"),
            Err(e) => tracing::warn!(error = %e, "malformed codec message"),
        }
        Some(Vec::new())
    }
}

/// Whether `kind` is in the codec range.
pub const fn is_codec(kind: MessageType) -> bool {
    kind.0 >> 8 == 0x05
}
