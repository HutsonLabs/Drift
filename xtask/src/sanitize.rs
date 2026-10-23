//! Fixture sanitizer (task M0-3).
//!
//! Replaces every known secret in a fixture by a placeholder of the **same length**, so
//! offsets and length fields inside captured PDUs stay valid. Secrets come from two places:
//!
//! 1. the dev-machine secrets directory (`~/code/drift-spikes/secrets/*.txt`): every value
//!    line of at least [`MIN_TEXT_LEN`] characters (user names too, not only passwords),
//!    plus the decoded bytes of hex lines of at least [`MIN_HEX_BYTES`] bytes (the
//!    `lastredir.txt` one-time password and GUID blobs);
//! 2. the fixtures themselves: the one-time user name and password blob of every captured
//!    Server Redirection PDU and RDSTLS AuthRequest (plan §1.3). They are single-use and
//!    already invalid, but are removed anyway.
//!
//! Text secrets are matched as UTF-8 bytes and as UTF-16LE; byte secrets as raw bytes.
//! Placeholders cycle `REDACTED` (as UTF-16LE for UTF-16LE matches).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

/// Minimum length (in characters) of a text value treated as a secret.
pub const MIN_TEXT_LEN: usize = 6;
/// Minimum decoded length of a hex line treated as a byte secret.
pub const MIN_HEX_BYTES: usize = 8;
/// Placeholder pattern, cycled to the secret's length.
pub const PLACEHOLDER: &[u8] = b"REDACTED";

/// What a secret looks like.
#[derive(Clone, PartialEq, Eq)]
pub enum SecretValue {
    /// A string, matched as UTF-8 and as UTF-16LE.
    Text(String),
    /// An opaque blob, matched as raw bytes.
    Bytes(Vec<u8>),
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text(t) => write!(f, "Text(<{} chars redacted>)", t.chars().count()),
            Self::Bytes(b) => write!(f, "Bytes(<{} bytes redacted>)", b.len()),
        }
    }
}

/// A secret to remove from fixtures. `Debug` never shows the value.
#[derive(Clone, PartialEq, Eq)]
pub struct KnownSecret {
    /// Where the secret came from (`system.txt:2`, `pdus/x.bin: one-time password`).
    pub label: String,
    /// The value.
    pub value: SecretValue,
}

impl std::fmt::Debug for KnownSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KnownSecret({}, <redacted>)", self.label)
    }
}

/// Form in which a secret was matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Raw bytes (byte secrets).
    Bytes,
    /// UTF-8 text.
    Utf8,
    /// UTF-16 little endian text.
    Utf16Le,
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

impl KnownSecret {
    /// A text secret.
    pub fn text(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self { label: label.into(), value: SecretValue::Text(value.into()) }
    }

    /// A byte secret.
    pub fn bytes(label: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        Self { label: label.into(), value: SecretValue::Bytes(value.into()) }
    }

    /// `(form, needle, same-length placeholder)` for every form this secret is matched in.
    pub fn needles(&self) -> Vec<(Form, Vec<u8>, Vec<u8>)> {
        match &self.value {
            SecretValue::Text(t) => {
                let units = t.encode_utf16().count();
                let wide: Vec<u8> =
                    placeholder(units).iter().flat_map(|b| u16::from(*b).to_le_bytes()).collect();
                vec![
                    (Form::Utf8, t.as_bytes().to_vec(), placeholder(t.len())),
                    (Form::Utf16Le, utf16le(t), wide),
                ]
            }
            SecretValue::Bytes(b) => vec![(Form::Bytes, b.clone(), placeholder(b.len()))],
        }
    }
}

/// `n` bytes of the cycled [`PLACEHOLDER`].
pub fn placeholder(n: usize) -> Vec<u8> {
    PLACEHOLDER.iter().copied().cycle().take(n).collect()
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// Secrets in one secrets file's text (see module docs).
pub fn secrets_from_text(source: &str, text: &str) -> Vec<KnownSecret> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        let value = match line.split_once(": ") {
            Some((key, v))
                if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ') =>
            {
                v.trim()
            }
            _ => line,
        };
        if value.chars().count() < MIN_TEXT_LEN {
            continue;
        }
        let label = format!("{source}:{}", i + 1);
        if let Some(bytes) = decode_hex(value).filter(|b| b.len() >= MIN_HEX_BYTES) {
            out.push(KnownSecret::bytes(format!("{label} (hex-decoded)"), bytes));
        }
        out.push(KnownSecret::text(label, value));
    }
    // Text entries first keeps `secrets_from_text` output ordered by line for readers.
    out.sort_by_key(|s| matches!(s.value, SecretValue::Bytes(_)));
    out
}

/// Secrets from every `*.txt` in `dir`. A missing directory yields none.
pub fn load_known_secrets(dir: &Path) -> Result<Vec<KnownSecret>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "txt") && p.is_file())
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        let text = String::from_utf8_lossy(&std::fs::read(&p)?).into_owned();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        out.extend(secrets_from_text(&name, &text));
    }
    Ok(out)
}

/// All `(needle, placeholder)` pairs, longest needle first so that a secret containing
/// another secret is replaced as a whole.
fn replacement_table(secrets: &[KnownSecret]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut table: Vec<(Vec<u8>, Vec<u8>)> = secrets
        .iter()
        .flat_map(KnownSecret::needles)
        .filter(|(_, n, _)| !n.is_empty())
        .map(|(_, n, p)| (n, p))
        .collect();
    table.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    table.dedup_by(|a, b| a.0 == b.0);
    table
}

/// Replaces every occurrence of every secret. Returns the sanitized bytes and the number
/// of replacements. The output always has the input's length.
pub fn sanitize(data: &[u8], secrets: &[KnownSecret]) -> (Vec<u8>, usize) {
    let mut out = data.to_vec();
    let mut count = 0;
    for (needle, repl) in replacement_table(secrets) {
        let finder = memchr::memmem::Finder::new(&needle);
        let mut pos = 0;
        while let Some(i) = out.get(pos..).and_then(|hay| finder.find(hay)) {
            let at = pos + i;
            out[at..at + needle.len()].copy_from_slice(&repl);
            count += 1;
            pos = at + needle.len();
        }
    }
    (out, count)
}

/// Secrets (label, form) still present in `data`.
pub fn find(data: &[u8], secrets: &[KnownSecret]) -> Vec<(String, Form)> {
    let mut hits = Vec::new();
    for s in secrets {
        for (form, needle, _) in s.needles() {
            if !needle.is_empty() && memchr::memmem::find(data, &needle).is_some() {
                hits.push((s.label.clone(), form));
            }
        }
    }
    hits
}

/// Bounds-checked little-endian reader.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).context("length overflow")?;
        let s = self
            .data
            .get(self.pos..end)
            .with_context(|| format!("truncated: need {n} bytes at offset {}", self.pos))?;
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16le(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32le(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Decodes a UTF-16LE user name, dropping trailing NULs.
fn utf16_name(bytes: &[u8]) -> Result<String> {
    ensure!(bytes.len().is_multiple_of(2), "odd UTF-16 length");
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let s = String::from_utf16(&units).context("invalid UTF-16 user name")?;
    Ok(s.trim_end_matches('\0').to_owned())
}

fn one_time(label: &str, user: String, password: &[u8]) -> Vec<KnownSecret> {
    let mut out = Vec::new();
    if !user.is_empty() {
        out.push(KnownSecret::text(format!("{label}: one-time user name"), user));
    }
    if !password.is_empty() {
        out.push(KnownSecret::bytes(format!("{label}: one-time password"), password.to_vec()));
    }
    out
}

/// Redirection flags (MS-RDPBCGR 2.2.13.1) that carry a `u32 length + bytes` field, in wire
/// order.
const LB_TARGET_NET_ADDRESS: u32 = 0x0001;
const LB_LOAD_BALANCE_INFO: u32 = 0x0002;
const LB_USERNAME: u32 = 0x0004;
const LB_DOMAIN: u32 = 0x0008;
const LB_PASSWORD: u32 = 0x0010;
const LB_TARGET_FQDN: u32 = 0x0100;
const LB_TARGET_NETBIOS_NAME: u32 = 0x0200;
const LB_CLIENT_TSV_URL: u32 = 0x1000;
const LB_REDIRECTION_GUID: u32 = 0x8000;
const LB_TARGET_CERTIFICATE: u32 = 0x1_0000;
const LB_TARGET_NET_ADDRESSES: u32 = 0x0800;

/// The one-time user name and password blob of a captured Server Redirection PDU frame
/// (TPKT + X.224 + MCS Send Data Indication + Share Control PDU of type 0xA).
pub fn one_time_from_server_redirection(label: &str, frame: &[u8]) -> Result<Vec<KnownSecret>> {
    let mut r = Reader::new(frame);
    ensure!(r.u8()? == 0x03, "not a TPKT frame");
    r.take(3)?; // reserved + length
    let li = r.u8()?;
    ensure!(r.u8()? == 0xF0, "not an X.224 data TPDU");
    r.take(usize::from(li).saturating_sub(1))?;
    ensure!(r.u8()? >> 2 == 26, "not an MCS Send Data Indication");
    r.take(5)?; // initiator, channel id, data priority/segmentation
    let len0 = r.u8()?;
    if len0 & 0x80 != 0 {
        r.u8()?;
    }
    let _total = r.u16le()?;
    let pdu_type = r.u16le()?;
    ensure!(pdu_type & 0x0F == 0x0A, "share control PDU type {:#x} is not a redirection", pdu_type & 0x0F);
    r.u16le()?; // pduSource
    r.take(2)?; // pad2Octets
    let flags = r.u16le()?;
    ensure!(flags == 0x0400, "bad SEC_REDIRECTION_PKT flags {flags:#x}");
    r.u16le()?; // length
    r.u32le()?; // sessionId
    let redir = r.u32le()?;

    let mut user = String::new();
    let mut password: &[u8] = &[];
    for bit in [
        LB_TARGET_NET_ADDRESS,
        LB_LOAD_BALANCE_INFO,
        LB_USERNAME,
        LB_DOMAIN,
        LB_PASSWORD,
        LB_TARGET_FQDN,
        LB_TARGET_NETBIOS_NAME,
        LB_CLIENT_TSV_URL,
        LB_REDIRECTION_GUID,
        LB_TARGET_CERTIFICATE,
        LB_TARGET_NET_ADDRESSES,
    ] {
        if redir & bit == 0 {
            continue;
        }
        let n = usize::try_from(r.u32le()?).context("field length")?;
        let v = r.take(n)?;
        match bit {
            LB_USERNAME => user = utf16_name(v)?,
            LB_PASSWORD => password = v,
            _ => {}
        }
    }
    if redir & (LB_USERNAME | LB_PASSWORD) == 0 {
        bail!("redirection PDU carries no credentials (redirFlags {redir:#x})");
    }
    Ok(one_time(label, user, password))
}

/// The one-time user name and password blob of a captured RDSTLS AuthRequest.
pub fn one_time_from_rdstls_auth_request(label: &str, pdu: &[u8]) -> Result<Vec<KnownSecret>> {
    let mut r = Reader::new(pdu);
    let version = r.u16le()?;
    let pdu_type = r.u16le()?;
    let data_type = r.u16le()?;
    ensure!(
        version == 1 && pdu_type == 2 && data_type == 1,
        "not an RDSTLS password AuthRequest (version {version}, type {pdu_type}, dataType {data_type})"
    );
    let mut field = || -> Result<&[u8]> {
        let n = usize::from(r.u16le()?);
        r.take(n)
    };
    let _guid = field()?;
    let user = utf16_name(field()?)?;
    let _domain = field()?;
    let password = field()?;
    ensure!(r.pos == pdu.len(), "{} trailing bytes after the AuthRequest", pdu.len() - r.pos);
    Ok(one_time(label, user, password))
}
