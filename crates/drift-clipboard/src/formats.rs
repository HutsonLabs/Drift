//! Pure clipboard format mapping (UTF-16LE/CRLF text, PNG, TIFF, DIB). Owned by task **M5-1**.
//!
//! | Local (NSPasteboard) | Remote (CLIPRDR) |
//! |---|---|
//! | `public.utf8-plain-text` | `CF_UNICODETEXT` (UTF-16LE + NUL, LF↔CRLF); `CF_TEXT` accepted only when Unicode is absent |
//! | `public.png` | named format `"image/png"` (bytes pass through unchanged) |
//! | `public.tiff` | `CF_TIFF` |
//! | `public.png` | `CF_DIB` (inbound: decoded and re-encoded as PNG; outbound: for compatibility) |

use drift_core::ClipboardPrefs;
use serde::{Deserialize, Serialize};

use crate::{ClipboardContents, ClipboardItem};

/// `CF_TEXT`: ANSI text.
pub const CF_TEXT: u32 = 1;
/// `CF_TIFF`: TIFF image.
pub const CF_TIFF: u32 = 6;
/// `CF_DIB`: packed device-independent bitmap (`BITMAPINFO` + bits).
pub const CF_DIB: u32 = 8;
/// `CF_UNICODETEXT`: UTF-16LE text with a terminating NUL, CRLF line endings.
pub const CF_UNICODETEXT: u32 = 13;
/// Name of the registered PNG format used by g-r-d (and by Drift outbound).
pub const PNG_FORMAT_NAME: &str = "image/png";
/// The id Drift registers for its outbound `"image/png"` format. Registered ids are per-side
/// (g-r-d uses `0xD011`), so inbound PNG is recognised by name, never by id.
pub const LOCAL_PNG_FORMAT_ID: u32 = 0xC0F0;
/// Largest clipboard payload Drift accepts or offers, in bytes (32 MiB). Also bounds the
/// decoded RGBA size of a DIB.
pub const MAX_CLIPBOARD_BYTES: usize = 32 * 1024 * 1024;

/// A CLIPRDR format as it appears in a Format List PDU.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClipFormat {
    /// Format id (standard `CF_*` or a registered id ≥ `0xC000`).
    pub id: u32,
    /// Format name for registered formats.
    pub name: Option<String>,
}

impl ClipFormat {
    /// A standard (unnamed) format.
    pub fn standard(id: u32) -> Self {
        Self { id, name: None }
    }

    /// A registered (named) format.
    pub fn named(id: u32, name: &str) -> Self {
        Self { id, name: Some(name.to_owned()) }
    }

    /// What Drift understands this format to be, if anything. Registered formats are matched
    /// by name (ids are per side); standard formats by id.
    pub fn kind(&self) -> Option<FormatKind> {
        if let Some(name) = &self.name {
            return (name == PNG_FORMAT_NAME).then_some(FormatKind::Png);
        }
        match self.id {
            CF_UNICODETEXT => Some(FormatKind::UnicodeText),
            CF_TEXT => Some(FormatKind::AnsiText),
            CF_TIFF => Some(FormatKind::Tiff),
            CF_DIB => Some(FormatKind::Dib),
            _ => None,
        }
    }
}

/// The formats Drift knows how to convert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FormatKind {
    /// `CF_UNICODETEXT`.
    UnicodeText,
    /// `CF_TEXT`.
    AnsiText,
    /// Named `"image/png"`.
    Png,
    /// `CF_TIFF`.
    Tiff,
    /// `CF_DIB`.
    Dib,
}

impl FormatKind {
    /// `true` for image formats (gated by [`ClipboardPrefs::TextAndImages`]).
    pub fn is_image(self) -> bool {
        matches!(self, Self::Png | Self::Tiff | Self::Dib)
    }

    /// `true` when `prefs` allows this kind to be synced.
    pub fn allowed_by(self, prefs: ClipboardPrefs) -> bool {
        match prefs {
            ClipboardPrefs::Off => false,
            ClipboardPrefs::Text => !self.is_image(),
            ClipboardPrefs::TextAndImages => true,
        }
    }
}

/// Why a clipboard payload was refused. Surfaced to the actor as a rejection event.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum ClipError {
    /// The payload exceeds [`MAX_CLIPBOARD_BYTES`].
    #[error("clipboard payload of {size} bytes exceeds the {max}-byte limit")]
    TooLarge {
        /// Payload (or decoded) size in bytes.
        size: usize,
        /// The limit.
        max: usize,
    },
    /// The image bytes could not be decoded or encoded.
    #[error("invalid clipboard image: {0}")]
    InvalidImage(String),
    /// The requested format is not one Drift offered.
    #[error("clipboard format {0:#x} is not offered")]
    Unavailable(u32),
    /// The remote side answered a data request with an error.
    #[error("the remote clipboard returned an error")]
    RemoteError,
}

/// Fails with [`ClipError::TooLarge`] when `size` exceeds [`MAX_CLIPBOARD_BYTES`].
pub fn check_size(size: usize) -> Result<(), ClipError> {
    if size > MAX_CLIPBOARD_BYTES {
        Err(ClipError::TooLarge { size, max: MAX_CLIPBOARD_BYTES })
    } else {
        Ok(())
    }
}

/// Local text → `CF_UNICODETEXT` bytes: LF becomes CRLF (existing CRLF is kept), UTF-16LE,
/// terminated by a NUL code unit.
pub fn encode_unicode_text(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2 + 2);
    let mut prev = '\0';
    let mut buf = [0u16; 2];
    for ch in text.chars() {
        if ch == '\n' && prev != '\r' {
            out.extend_from_slice(&u16::from(b'\r').to_le_bytes());
        }
        for unit in ch.encode_utf16(&mut buf) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        prev = ch;
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// `CF_UNICODETEXT` bytes → local text: stops at the first NUL, CRLF becomes LF, unpaired
/// surrogates become U+FFFD, a trailing odd byte is ignored.
pub fn decode_unicode_text(bytes: &[u8]) -> String {
    let units =
        bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0);
    let text: String = char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect();
    crlf_to_lf(&text)
}

/// `CF_TEXT` bytes → local text: stops at the first NUL, CRLF becomes LF. UTF-8 is used when
/// valid (g-r-d synthesises it from UTF-8), otherwise the bytes are read as Latin-1.
pub fn decode_ansi_text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let bytes = &bytes[..end];
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    };
    crlf_to_lf(&text)
}

fn crlf_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn invalid(e: impl std::fmt::Display) -> ClipError {
    ClipError::InvalidImage(e.to_string())
}

/// Size of a `BITMAPINFOHEADER`.
const BITMAPINFOHEADER_LEN: u32 = 40;

/// Decodes a packed DIB (`CF_DIB`: `BITMAPINFOHEADER` [+ masks/palette] + pixels; 24/32 bpp,
/// top-down or bottom-up) and re-encodes it as PNG.
pub fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>, ClipError> {
    use image::ImageDecoder;

    check_size(dib.len())?;
    let header = dib
        .get(..BITMAPINFOHEADER_LEN as usize)
        .ok_or_else(|| invalid("DIB shorter than BITMAPINFOHEADER"))?;
    // Bound the decoded size from the header before allocating anything.
    let dim = |at: usize| {
        u64::from(
            i32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]).unsigned_abs(),
        )
    };
    let decoded = dim(4).saturating_mul(dim(8)).saturating_mul(4);
    if decoded > MAX_CLIPBOARD_BYTES as u64 {
        return Err(ClipError::TooLarge {
            size: usize::try_from(decoded).unwrap_or(usize::MAX),
            max: MAX_CLIPBOARD_BYTES,
        });
    }
    let decoder = image::codecs::bmp::BmpDecoder::new_without_file_header(std::io::Cursor::new(dib))
        .map_err(invalid)?;
    let (width, height) = decoder.dimensions();
    let color = decoder.color_type();
    let mut pixels = vec![0u8; usize::try_from(decoder.total_bytes()).map_err(invalid)?];
    decoder.read_image(&mut pixels).map_err(invalid)?;
    let rgba = match color {
        image::ColorType::Rgba8 => image::RgbaImage::from_raw(width, height, pixels),
        image::ColorType::Rgb8 => image::RgbImage::from_raw(width, height, pixels)
            .map(|rgb| image::DynamicImage::ImageRgb8(rgb).to_rgba8()),
        image::ColorType::L8 => image::GrayImage::from_raw(width, height, pixels)
            .map(|l| image::DynamicImage::ImageLuma8(l).to_rgba8()),
        other => return Err(invalid(format!("unsupported DIB colour type {other:?}"))),
    }
    .ok_or_else(|| invalid("DIB pixel buffer size mismatch"))?;
    encode_png(&rgba)
}

fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>, ClipError> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(invalid)?;
    Ok(out.into_inner())
}

/// Encodes a PNG as a 32 bpp bottom-up `CF_DIB` (for remote apps that only read DIBs).
pub fn png_to_dib(png: &[u8]) -> Result<Vec<u8>, ClipError> {
    png_passthrough(png)?;
    let rgba = image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(invalid)?.to_rgba8();
    let (w, h) = rgba.dimensions();
    let image_size = u64::from(w) * u64::from(h) * 4;
    check_size(usize::try_from(image_size).unwrap_or(usize::MAX))?;
    let image_size = u32::try_from(image_size).map_err(invalid)?;
    let mut out = Vec::with_capacity((BITMAPINFOHEADER_LEN + image_size) as usize);
    out.extend_from_slice(&BITMAPINFOHEADER_LEN.to_le_bytes()); // biSize
    out.extend_from_slice(&i32::try_from(w).map_err(invalid)?.to_le_bytes()); // biWidth
    out.extend_from_slice(&i32::try_from(h).map_err(invalid)?.to_le_bytes()); // biHeight > 0: bottom-up
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    out.extend_from_slice(&image_size.to_le_bytes()); // biSizeImage
    out.extend_from_slice(&2835i32.to_le_bytes()); // biXPelsPerMeter (72 dpi)
    out.extend_from_slice(&2835i32.to_le_bytes()); // biYPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, a] = rgba.get_pixel(x, y).0;
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    Ok(out)
}

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Validates a PNG payload (signature and size) and returns it unchanged.
pub fn png_passthrough(png: &[u8]) -> Result<Vec<u8>, ClipError> {
    check_size(png.len())?;
    if !png.starts_with(PNG_SIGNATURE) {
        return Err(invalid("missing PNG signature"));
    }
    Ok(png.to_vec())
}

fn item_kind(item: &ClipboardItem) -> FormatKind {
    match item {
        ClipboardItem::Text(_) => FormatKind::UnicodeText,
        ClipboardItem::Png(_) => FormatKind::Png,
        ClipboardItem::Tiff(_) => FormatKind::Tiff,
    }
}

fn item_size(item: &ClipboardItem) -> usize {
    match item {
        ClipboardItem::Text(t) => t.len(),
        ClipboardItem::Png(b) | ClipboardItem::Tiff(b) => b.len(),
    }
}

/// Keeps only the items `prefs` allows and that fit under the size cap. Returns the kept
/// contents and one rejection per oversized item.
pub fn filter_local(
    contents: &ClipboardContents,
    prefs: ClipboardPrefs,
) -> (ClipboardContents, Vec<ClipError>) {
    let mut kept = ClipboardContents::empty();
    let mut rejected = Vec::new();
    for item in contents.items.iter().filter(|i| item_kind(i).allowed_by(prefs)) {
        match check_size(item_size(item)) {
            Ok(()) => kept.items.push(item.clone()),
            Err(e) => rejected.push(e),
        }
    }
    (kept, rejected)
}

fn find_png(contents: &ClipboardContents) -> Option<&[u8]> {
    contents.items.iter().find_map(|i| match i {
        ClipboardItem::Png(p) => Some(p.as_slice()),
        _ => None,
    })
}

fn find_tiff(contents: &ClipboardContents) -> Option<&[u8]> {
    contents.items.iter().find_map(|i| match i {
        ClipboardItem::Tiff(t) => Some(t.as_slice()),
        _ => None,
    })
}

/// The Format List Drift advertises for local `contents` under `prefs`:
/// `CF_UNICODETEXT` for text; `"image/png"` + `CF_DIB` for PNG; `CF_TIFF` for a TIFF-only
/// clipboard. Empty when `prefs` is `Off`.
pub fn outbound_formats(contents: &ClipboardContents, prefs: ClipboardPrefs) -> Vec<ClipFormat> {
    let mut out = Vec::new();
    if FormatKind::UnicodeText.allowed_by(prefs) && contents.text().is_some() {
        out.push(ClipFormat::standard(CF_UNICODETEXT));
    }
    if FormatKind::Png.allowed_by(prefs) {
        if find_png(contents).is_some() {
            out.push(ClipFormat::named(LOCAL_PNG_FORMAT_ID, PNG_FORMAT_NAME));
            out.push(ClipFormat::standard(CF_DIB));
        } else if find_tiff(contents).is_some() {
            out.push(ClipFormat::standard(CF_TIFF));
        }
    }
    out
}

/// The remote formats to fetch after a remote copy, in fetch order: at most one text format
/// (`CF_UNICODETEXT`, else `CF_TEXT`) then, if `prefs` allows images, at most one image format
/// (`"image/png"`, else `CF_TIFF`, else `CF_DIB`).
pub fn select_inbound(formats: &[ClipFormat], prefs: ClipboardPrefs) -> Vec<(ClipFormat, FormatKind)> {
    const PREFERENCE: [&[FormatKind]; 2] = [
        &[FormatKind::UnicodeText, FormatKind::AnsiText],
        &[FormatKind::Png, FormatKind::Tiff, FormatKind::Dib],
    ];
    PREFERENCE
        .iter()
        .filter_map(|group| {
            group
                .iter()
                .filter(|k| k.allowed_by(prefs))
                .find_map(|&k| formats.iter().find(|f| f.kind() == Some(k)).map(|f| (f.clone(), k)))
        })
        .collect()
}

/// Converts fetched remote bytes of `kind` into a local item (size-capped).
pub fn decode_inbound(kind: FormatKind, data: &[u8]) -> Result<ClipboardItem, ClipError> {
    check_size(data.len())?;
    Ok(match kind {
        FormatKind::UnicodeText => ClipboardItem::Text(decode_unicode_text(data)),
        FormatKind::AnsiText => ClipboardItem::Text(decode_ansi_text(data)),
        FormatKind::Png => ClipboardItem::Png(png_passthrough(data)?),
        FormatKind::Tiff => ClipboardItem::Tiff(data.to_vec()),
        FormatKind::Dib => ClipboardItem::Png(dib_to_png(data)?),
    })
}

/// Produces the bytes for a server Format Data Request of `format_id`, from local `contents`
/// under `prefs` (size-capped).
pub fn encode_outbound(
    format_id: u32,
    contents: &ClipboardContents,
    prefs: ClipboardPrefs,
) -> Result<Vec<u8>, ClipError> {
    let unavailable = ClipError::Unavailable(format_id);
    if !outbound_formats(contents, prefs).iter().any(|f| f.id == format_id) {
        return Err(unavailable);
    }
    let data = match format_id {
        CF_UNICODETEXT => contents.text().map(encode_unicode_text),
        LOCAL_PNG_FORMAT_ID => find_png(contents).map(<[u8]>::to_vec),
        CF_DIB => find_png(contents).map(png_to_dib).transpose()?,
        CF_TIFF => find_tiff(contents).map(<[u8]>::to_vec),
        _ => None,
    }
    .ok_or(unavailable)?;
    check_size(data.len())?;
    Ok(data)
}
