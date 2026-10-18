//! Pairing with a Drift Host on a Mac, client half (task **IOS-9**; ADR `IOS-0` decision 7,
//! ADR `IOS-9-pairing`).
//!
//! A Mac running Drift Host advertises itself with Bonjour ([`SERVICE_TYPE`], a [`HostAdvert`]
//! in the TXT record) and shows a [`ShortCode`] and a QR code ([`PairingPayload`]). The device
//! either scans the QR code, which names the host and its certificate, or picks the host from
//! the Bonjour list and types the code. Either way it ends up with a [`PairingTarget`]: the host,
//! the certificate fingerprint to pin and the code to prove the user saw the Mac's screen.
//!
//! [`PairingFlow`] is the client's state machine for the handshake over the `Drift::Mac::v1`
//! channel (EXT-1, HOST-8): it checks the TLS certificate against the pin **before** the code is
//! sent, then waits for the host's answer. The channel itself is not built yet.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::profile::{CertFingerprint, ConnectMode, ConnectionProfile};

/// The Bonjour service type of Drift Host.
///
/// Its own type rather than `_rdp._tcp`: the TXT record carries Drift fields (host id,
/// certificate fingerprint), and GNOME hosts do not advertise RDP over Bonjour anyway.
pub const SERVICE_TYPE: &str = "_drift-host._tcp";

/// The pairing protocol version this client speaks (QR payload, TXT record, messages).
pub const PROTOCOL_VERSION: u32 = 1;

/// The RDP port when an advert does not name one.
pub const DEFAULT_PORT: u16 = 3389;

/// The URI scheme of the QR payload.
const SCHEME: &str = "drift-pair:";

/// Why pairing input was refused or a pairing failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairingError {
    /// The typed code is not eight digits.
    #[error("A pairing code has eight digits.")]
    MalformedCode,
    /// The code's check digit does not match: a typo.
    #[error("That code has a typo. Check the digits shown on the Mac.")]
    WrongCheckDigit,
    /// The QR payload or TXT record is not well formed.
    #[error("This is not a Drift pairing code ({0}).")]
    Malformed(String),
    /// A field is missing from the QR payload or TXT record.
    #[error("This pairing code is incomplete (no {0}).")]
    MissingField(&'static str),
    /// The host speaks a newer pairing protocol.
    #[error("This Mac runs a newer Drift Host (pairing version {0}). Update Drift.")]
    UnsupportedVersion(u32),
    /// The host presented a different certificate from the one being paired.
    #[error("The Mac's certificate does not match the pairing code. Nothing was sent.")]
    FingerprintMismatch,
    /// A message arrived in the wrong state.
    #[error("The Mac answered out of order.")]
    UnexpectedMessage,
    /// The host refused.
    #[error("{0}")]
    Rejected(RejectReason),
}

/// Why the host refused a pairing request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RejectReason {
    /// The code does not match the one on the Mac's screen.
    WrongCode,
    /// The code on the Mac's screen has expired.
    Expired,
    /// Too many wrong codes; the Mac stopped pairing for a while.
    TooManyAttempts,
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WrongCode => "The Mac did not accept the code.",
            Self::Expired => "The code on the Mac has expired. Start pairing again on the Mac.",
            Self::TooManyAttempts => "Too many wrong codes. Wait a minute, then start again on the Mac.",
        })
    }
}

// ---- the typed code ---------------------------------------------------------------------------

/// Damm's quasigroup of order 10 (totally anti-symmetric, zero diagonal).
const DAMM: [[u8; 10]; 10] = [
    [0, 3, 1, 7, 5, 9, 8, 6, 4, 2],
    [7, 0, 9, 2, 1, 5, 4, 8, 6, 3],
    [4, 2, 0, 6, 8, 7, 1, 3, 5, 9],
    [1, 7, 5, 0, 9, 8, 3, 4, 2, 6],
    [6, 1, 2, 3, 0, 4, 5, 9, 7, 8],
    [3, 6, 7, 4, 2, 0, 9, 5, 8, 1],
    [5, 8, 6, 9, 7, 2, 0, 1, 3, 4],
    [8, 9, 4, 5, 3, 6, 2, 0, 1, 7],
    [9, 4, 3, 8, 6, 1, 7, 2, 0, 5],
    [2, 5, 8, 1, 4, 3, 6, 7, 9, 0],
];

/// Damm check digit of a string of ASCII digits (`None` if it has anything else). It catches
/// every single-digit error and every swap of adjacent digits.
pub fn damm_check_digit(digits: &str) -> Option<u8> {
    let mut interim = 0_u8;
    for c in digits.bytes() {
        if !c.is_ascii_digit() {
            return None;
        }
        interim = DAMM[usize::from(interim)][usize::from(c - b'0')];
    }
    Some(interim)
}

/// The code the Mac shows: seven random digits and a Damm check digit, shown as `1234 5678`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ShortCode(String);

impl ShortCode {
    /// The code for a seven-digit body (`None` above 9 999 999).
    pub fn from_body(body: u32) -> Option<Self> {
        if body >= 10_000_000 {
            return None;
        }
        let mut digits = format!("{body:07}");
        let check = damm_check_digit(&digits)?;
        digits.push(char::from(b'0' + check));
        Some(Self(digits))
    }

    /// Parses a typed code; spaces and dashes are ignored.
    pub fn parse(text: &str) -> Result<Self, PairingError> {
        let digits: String = text.chars().filter(|c| !matches!(c, ' ' | '-' | '\u{a0}')).collect();
        if digits.len() != 8 || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return Err(PairingError::MalformedCode);
        }
        // A valid Damm number checks to 0 including its check digit.
        if damm_check_digit(&digits) != Some(0) {
            return Err(PairingError::WrongCheckDigit);
        }
        Ok(Self(digits))
    }

    /// The eight digits, without separators.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ShortCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (a, b) = self.0.split_at(4);
        write!(f, "{a} {b}")
    }
}

impl fmt::Debug for ShortCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ShortCode({self})")
    }
}

// ---- what is being paired --------------------------------------------------------------------

/// A host to pair with, the certificate to pin and the code the user saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingTarget {
    /// The Drift Host's stable id.
    pub host_id: Uuid,
    /// The Mac's name (the new connection's name).
    pub name: String,
    /// Host name or address to connect to.
    pub host: String,
    /// RDP port.
    pub port: u16,
    /// SHA-256 of the host's TLS certificate, pinned from the start.
    pub fingerprint: CertFingerprint,
    /// The code shown on the Mac.
    pub code: ShortCode,
}

impl PairingTarget {
    /// The connection a pairing creates: pinned to the host's certificate (never
    /// trust-on-first-use) and marked as paired with `host_id`. Headless mode: the Mac takes the
    /// account's credentials directly, with no greeter. The account name is filled in by the
    /// caller.
    pub fn profile(&self) -> ConnectionProfile {
        let mut p = ConnectionProfile::new(self.name.clone(), self.host.clone(), ConnectMode::Headless);
        p.port = self.port;
        p.cert_pin = Some(self.fingerprint);
        p.paired_host = Some(self.host_id);
        p
    }
}

// ---- the QR payload ---------------------------------------------------------------------------

/// What the Mac's QR code holds:
/// `drift-pair:1?id=<uuid>&name=<pct>&host=<pct>&port=<n>&fp=<64 hex>&code=<8 digits>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingPayload(PairingTarget);

impl From<PairingTarget> for PairingPayload {
    fn from(target: PairingTarget) -> Self {
        Self(target)
    }
}

impl PairingPayload {
    /// The target the payload names.
    pub fn target(&self) -> PairingTarget {
        self.0.clone()
    }

    /// The payload as the QR code's text.
    pub fn to_uri(&self) -> String {
        let t = &self.0;
        format!(
            "{SCHEME}{PROTOCOL_VERSION}?id={}&name={}&host={}&port={}&fp={}&code={}",
            t.host_id,
            percent_encode(&t.name),
            percent_encode(&t.host),
            t.port,
            hex(t.fingerprint.as_bytes()),
            t.code.as_str(),
        )
    }

    /// Parses QR text strictly: right scheme and version, every field present once and valid.
    /// Unknown fields are ignored, so a later host can add some.
    pub fn parse(text: &str) -> Result<Self, PairingError> {
        let rest = text.trim().strip_prefix(SCHEME).ok_or_else(|| malformed("not a drift-pair URI"))?;
        let (version, query) = rest.split_once('?').ok_or_else(|| malformed("no fields"))?;
        let version: u32 = version.parse().map_err(|_| malformed("bad version"))?;
        if version == 0 {
            return Err(malformed("bad version"));
        }
        if version > PROTOCOL_VERSION {
            return Err(PairingError::UnsupportedVersion(version));
        }
        let mut fields = HashMap::new();
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').ok_or_else(|| malformed("field without a value"))?;
            let value = percent_decode(value)?;
            if fields.insert(key, value).is_some() {
                return Err(malformed("repeated field"));
            }
        }
        let field = |name: &'static str| fields.get(name).ok_or(PairingError::MissingField(name));
        let name = field("name")?.clone();
        let host = field("host")?.clone();
        if name.is_empty() || host.is_empty() {
            return Err(malformed("empty name or host"));
        }
        Ok(Self(PairingTarget {
            host_id: field("id")?.parse().map_err(|_| malformed("bad host id"))?,
            name,
            host,
            port: parse_port(field("port")?)?,
            fingerprint: parse_hex_fingerprint(field("fp")?)?,
            code: ShortCode::parse(field("code")?)?,
        }))
    }
}

// ---- Bonjour ----------------------------------------------------------------------------------

/// A Drift Host found with Bonjour, from its TXT record
/// (`v`, `id`, `host`, `fp`, optional `name` and `port`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct HostAdvert {
    /// The Bonjour service instance name.
    pub service_name: String,
    /// The Drift Host's stable id.
    pub host_id: Uuid,
    /// The Mac's name (the service name when the record has none).
    pub name: String,
    /// Host name to connect to (the Mac's `.local` name).
    pub host: String,
    /// RDP port.
    pub port: u16,
    /// SHA-256 of the host's TLS certificate.
    pub fingerprint: CertFingerprint,
}

impl HostAdvert {
    /// Reads a TXT record (`key=value` entries, keys compared case-insensitively).
    pub fn from_txt(service_name: &str, txt: &[(String, String)]) -> Result<Self, PairingError> {
        let get = |key: &str| txt.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str());
        let need =
            |key: &'static str| get(key).filter(|v| !v.is_empty()).ok_or(PairingError::MissingField(key));
        let version: u32 = need("v")?.parse().map_err(|_| malformed("bad version"))?;
        if version == 0 {
            return Err(malformed("bad version"));
        }
        if version > PROTOCOL_VERSION {
            return Err(PairingError::UnsupportedVersion(version));
        }
        let name = get("name").filter(|n| !n.trim().is_empty()).unwrap_or(service_name).to_owned();
        Ok(Self {
            service_name: service_name.to_owned(),
            host_id: need("id")?.parse().map_err(|_| malformed("bad host id"))?,
            name,
            host: need("host")?.to_owned(),
            port: get("port").map_or(Ok(DEFAULT_PORT), parse_port)?,
            fingerprint: parse_hex_fingerprint(need("fp")?)?,
        })
    }

    /// The target for this host with the code the user typed.
    pub fn target(&self, code: ShortCode) -> PairingTarget {
        PairingTarget {
            host_id: self.host_id,
            name: self.name.clone(),
            host: self.host.clone(),
            port: self.port,
            fingerprint: self.fingerprint,
            code,
        }
    }
}

// ---- the handshake ----------------------------------------------------------------------------

/// The client's request, sent on `Drift::Mac::v1` once the certificate checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairRequest {
    /// [`PROTOCOL_VERSION`].
    pub version: u32,
    /// The code the user saw on the Mac.
    pub code: ShortCode,
    /// The device's name, shown in the host's list of paired devices.
    pub device_name: String,
    /// The device's public key (uncompressed P-256 point), which the host stores.
    pub device_key: Vec<u8>,
}

/// The host's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairReply {
    /// The code matched; the device key is stored.
    Accepted,
    /// Refused.
    Rejected(RejectReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowState {
    AwaitingTls,
    AwaitingReply,
    Done,
}

/// The client side of one pairing attempt.
#[derive(Debug, Clone)]
pub struct PairingFlow {
    target: PairingTarget,
    device_name: String,
    device_key: Vec<u8>,
    state: FlowState,
}

impl PairingFlow {
    /// A new attempt for `target`, introducing this device as `device_name` with `device_key`.
    pub fn new(target: PairingTarget, device_name: impl Into<String>, device_key: Vec<u8>) -> Self {
        Self { target, device_name: device_name.into(), device_key, state: FlowState::AwaitingTls }
    }

    /// The TLS handshake finished with a certificate whose fingerprint is `seen`. Returns the
    /// request to send; a different certificate ends the attempt without sending the code.
    pub fn on_tls(&mut self, seen: CertFingerprint) -> Result<PairRequest, PairingError> {
        if self.state != FlowState::AwaitingTls {
            self.state = FlowState::Done;
            return Err(PairingError::UnexpectedMessage);
        }
        if seen != self.target.fingerprint {
            self.state = FlowState::Done;
            return Err(PairingError::FingerprintMismatch);
        }
        self.state = FlowState::AwaitingReply;
        Ok(PairRequest {
            version: PROTOCOL_VERSION,
            code: self.target.code.clone(),
            device_name: self.device_name.clone(),
            device_key: self.device_key.clone(),
        })
    }

    /// The host answered. Accepted yields the paired profile.
    pub fn on_reply(&mut self, reply: PairReply) -> Result<ConnectionProfile, PairingError> {
        let state = std::mem::replace(&mut self.state, FlowState::Done);
        if state != FlowState::AwaitingReply {
            return Err(PairingError::UnexpectedMessage);
        }
        match reply {
            PairReply::Accepted => Ok(self.target.profile()),
            PairReply::Rejected(reason) => Err(PairingError::Rejected(reason)),
        }
    }

    /// Whether the attempt is over (paired, refused or failed).
    pub fn is_done(&self) -> bool {
        self.state == FlowState::Done
    }
}

// ---- helpers ----------------------------------------------------------------------------------

fn malformed(why: &str) -> PairingError {
    PairingError::Malformed(why.to_owned())
}

fn parse_port(text: &str) -> Result<u16, PairingError> {
    match text.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(malformed("bad port")),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 64 hex digits (no separators) as a fingerprint.
fn parse_hex_fingerprint(text: &str) -> Result<CertFingerprint, PairingError> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err(malformed("bad certificate fingerprint"));
    }
    let mut out = [0_u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let pair = text.get(2 * i..2 * i + 2).ok_or_else(|| malformed("bad certificate fingerprint"))?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| malformed("bad certificate fingerprint"))?;
    }
    Ok(CertFingerprint::from_bytes(out))
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(text: &str) -> Result<String, PairingError> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let pair = text.get(i + 1..i + 3).ok_or_else(|| malformed("bad escape"))?;
            if !pair.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err(malformed("bad escape"));
            }
            out.push(u8::from_str_radix(pair, 16).map_err(|_| malformed("bad escape"))?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| malformed("not UTF-8"))
}
