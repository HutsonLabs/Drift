//! Remote pointer decoding (task **M2-5**, pure part).
//!
//! g-r-d sends pointer shapes as fast-path output updates (MS-RDPBCGR 2.2.9.1.2.1): `PTR_NULL`
//! (hidden), `PTR_DEFAULT`, `POSITION`, `COLOR` (24 bpp), `POINTER` ("new", any bpp; 32 bpp
//! with alpha from g-r-d), `LARGE` (up to 384×384) and `CACHED` (re-use a cache slot). Large
//! shapes arrive split in `FIRST`/`NEXT`/`LAST` fragments (the 86×86 pointer at scale 200 does).
//!
//! [`PointerDecoder`] reassembles fragments, decodes the XOR/AND masks with
//! `ironrdp_graphics::pointer` into straight RGBA, and keeps the pointer cache: every
//! colour/new/large update stores its image in its cache slot (evicting the previous one) and
//! `CACHED` re-uses it. Output images are **premultiplied BGRA, top-down** (the same layout as
//! `drift_rdp::CursorBitmap`).
//!
//! g-r-d already scales pointer bitmaps with the desktop scale factor (43×43 at 100, 86×86 at
//! 200, plan §1.4), so the on-screen size in points is `bitmap / (scale / 100)`
//! ([`CursorImage::size_points`]).

use std::sync::Arc;

use drift_core::{Point, Size};
use ironrdp_graphics::pointer::{DecodedPointer, PointerBitmapTarget};
use ironrdp_pdu::fast_path::{FastPathHeader, FastPathUpdate, FastPathUpdatePdu, Fragmentation, UpdateCode};
use ironrdp_pdu::pointer::PointerUpdateData;
use ironrdp_pdu::{Decode as _, ReadCursor};

/// Default number of pointer cache slots Drift advertises (`TS_POINTER_CAPABILITYSET`).
pub const DEFAULT_CACHE_SIZE: usize = 25;

/// A decoded pointer image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    /// Bitmap size in desktop pixels.
    pub size: Size<u32>,
    /// Hotspot in bitmap pixels (inside the bitmap).
    pub hotspot: Point<u32>,
    /// Premultiplied BGRA8 pixels, top-down, `size.width * 4` bytes per row.
    pub bgra: Arc<[u8]>,
}

/// Points per bitmap pixel for a desktop scale factor in percent (bogus scales count as 100).
fn points_per_pixel(scale_percent: u32) -> f64 {
    if scale_percent == 0 { 1.0 } else { 100.0 / f64::from(scale_percent) }
}

impl CursorImage {
    /// Size in view points for a desktop scale factor in percent (`bitmap * 100 / scale`).
    pub fn size_points(&self, scale_percent: u32) -> Size<f64> {
        let k = points_per_pixel(scale_percent);
        Size::new(f64::from(self.size.width) * k, f64::from(self.size.height) * k)
    }

    /// Hotspot in view points for a desktop scale factor in percent.
    pub fn hotspot_points(&self, scale_percent: u32) -> Point<f64> {
        let k = points_per_pixel(scale_percent);
        Point::new(f64::from(self.hotspot.x) * k, f64::from(self.hotspot.y) * k)
    }

    /// The pixel at `(x, y)` as premultiplied `[b, g, r, a]`, or `None` outside the bitmap.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.size.width || y >= self.size.height {
            return None;
        }
        let i = (y as usize * self.size.width as usize + x as usize) * 4;
        let p = self.bgra.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }

    /// Converts IronRDP's straight-alpha RGBA ([`PointerBitmapTarget::Accelerated`]) into a
    /// premultiplied BGRA image with the hotspot clamped into the bitmap. `None` for an empty
    /// (invisible) pointer.
    pub fn from_decoded(decoded: &DecodedPointer) -> Option<Self> {
        let (w, h) = (u32::from(decoded.width), u32::from(decoded.height));
        if w == 0 || h == 0 || decoded.bitmap_data.len() != (w * h * 4) as usize {
            return None;
        }
        let bgra: Vec<u8> = decoded
            .bitmap_data
            .chunks_exact(4)
            .flat_map(|p| {
                let a = u32::from(p[3]);
                // (c * a + 127) / 255 <= a <= 255, so the cast is exact.
                let m = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
                [m(p[2]), m(p[1]), m(p[0]), p[3]]
            })
            .collect();
        Some(Self {
            size: Size::new(w, h),
            hotspot: Point::new(
                u32::from(decoded.hotspot_x).min(w - 1),
                u32::from(decoded.hotspot_y).min(h - 1),
            ),
            bgra: bgra.into(),
        })
    }
}

/// What the local pointer should look like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorShape {
    /// Hide the pointer over the remote view (`PTR_NULL`).
    Hidden,
    /// The system arrow (`PTR_DEFAULT`).
    Default,
    /// A remote pointer image.
    Image(Arc<CursorImage>),
}

/// One decoded pointer update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointerEvent {
    /// The pointer shape changed.
    Shape(CursorShape),
    /// The server moved the pointer (desktop pixels). Drift does not warp the Mac pointer.
    Position(Point<u16>),
}

/// Malformed or inconsistent pointer data (never a panic).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PointerError {
    /// The fast-path PDU or update could not be parsed.
    #[error("malformed pointer update: {0}")]
    Malformed(String),
    /// The XOR/AND masks could not be decoded.
    #[error("undecodable pointer bitmap: {0}")]
    Bitmap(String),
    /// A cache index outside the advertised cache.
    #[error("pointer cache index {index} out of range (cache size {size})")]
    CacheIndexOutOfRange {
        /// The index sent by the server.
        index: u16,
        /// The cache size.
        size: usize,
    },
    /// `CACHED` referenced an empty slot.
    #[error("pointer cache slot {0} is empty")]
    CacheMiss(u16),
    /// A `NEXT`/`LAST` fragment without a `FIRST`, or fragments of different update types.
    #[error("pointer update fragment out of sequence")]
    Fragment,
}

/// Largest reassembled update accepted (a 384×384 32 bpp large pointer is ~600 KB).
const MAX_REASSEMBLED: usize = 1 << 20;

/// Stateful fast-path pointer decoder with the pointer cache.
#[derive(Debug, Clone)]
pub struct PointerDecoder {
    /// One entry per cache slot: `Hidden` for an empty pointer image, `Image` otherwise.
    cache: Vec<Option<CursorShape>>,
    /// A `FIRST`/`NEXT` fragment sequence being reassembled.
    partial: Option<(UpdateCode, Vec<u8>)>,
}

impl Default for PointerDecoder {
    fn default() -> Self {
        Self::new(DEFAULT_CACHE_SIZE)
    }
}

impl PointerDecoder {
    /// A decoder with `cache_size` slots (as advertised in the pointer capability set).
    pub fn new(cache_size: usize) -> Self {
        Self { cache: vec![None; cache_size], partial: None }
    }

    /// Number of cache slots.
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    /// The image in cache slot `index`, if any.
    pub fn cached(&self, index: u16) -> Option<Arc<CursorImage>> {
        match self.cache.get(usize::from(index)) {
            Some(Some(CursorShape::Image(img))) => Some(img.clone()),
            _ => None,
        }
    }

    /// Clears the cache and any partial fragment (new connection / deactivation-reactivation).
    pub fn reset(&mut self) {
        self.cache.iter_mut().for_each(|slot| *slot = None);
        self.partial = None;
    }

    /// Decodes one complete fast-path output PDU (`TS_FP_UPDATE_PDU`: header, length, updates).
    /// Non-pointer updates are skipped. Fragments are kept until their `LAST` part arrives.
    pub fn decode_output_pdu(&mut self, pdu: &[u8]) -> Result<Vec<PointerEvent>, PointerError> {
        let mut src = ReadCursor::new(pdu);
        let header = FastPathHeader::decode(&mut src).map_err(|e| PointerError::Malformed(e.to_string()))?;
        if header.data_length > src.len() {
            return Err(PointerError::Malformed(format!(
                "fast-path length {} exceeds the {} bytes present",
                header.data_length,
                src.len()
            )));
        }
        let mut body = ReadCursor::new(src.read_slice(header.data_length));
        let mut events = Vec::new();
        while !body.is_empty() {
            let update =
                FastPathUpdatePdu::decode(&mut body).map_err(|e| PointerError::Malformed(e.to_string()))?;
            if let Some(event) = self.decode_update(&update)? {
                events.push(event);
            }
        }
        Ok(events)
    }

    /// Decodes one `TS_FP_UPDATE` (with fragment reassembly). Non-pointer updates yield `None`.
    pub fn decode_update(
        &mut self,
        update: &FastPathUpdatePdu<'_>,
    ) -> Result<Option<PointerEvent>, PointerError> {
        let code = update.update_code;
        if !is_pointer_code(code) {
            return Ok(None);
        }
        if update.compression_flags.is_some() {
            return Err(PointerError::Malformed("bulk-compressed pointer updates are not supported".into()));
        }
        let whole: Vec<u8>;
        let data: &[u8] = match update.fragmentation {
            Fragmentation::Single => {
                self.partial = None;
                update.data
            }
            Fragmentation::First => {
                self.partial = Some((code, update.data.to_vec()));
                return Ok(None);
            }
            Fragmentation::Next | Fragmentation::Last => {
                let Some((first_code, mut buf)) = self.partial.take() else {
                    return Err(PointerError::Fragment);
                };
                if first_code != code {
                    return Err(PointerError::Fragment);
                }
                buf.extend_from_slice(update.data);
                if buf.len() > MAX_REASSEMBLED {
                    return Err(PointerError::Malformed("reassembled pointer update too large".into()));
                }
                if update.fragmentation == Fragmentation::Next {
                    self.partial = Some((code, buf));
                    return Ok(None);
                }
                whole = buf;
                &whole
            }
        };
        let parsed = FastPathUpdate::decode_with_code(data, code)
            .map_err(|e| PointerError::Malformed(e.to_string()))?;
        match parsed {
            FastPathUpdate::Pointer(p) => self.apply(&p).map(Some),
            _ => Ok(None),
        }
    }

    /// Applies one parsed pointer update (for callers that already parsed the PDU).
    pub fn apply(&mut self, update: &PointerUpdateData<'_>) -> Result<PointerEvent, PointerError> {
        let target = PointerBitmapTarget::Accelerated;
        let (index, decoded) = match update {
            PointerUpdateData::SetHidden => return Ok(PointerEvent::Shape(CursorShape::Hidden)),
            PointerUpdateData::SetDefault => return Ok(PointerEvent::Shape(CursorShape::Default)),
            PointerUpdateData::SetPosition(p) => return Ok(PointerEvent::Position(Point::new(p.x, p.y))),
            PointerUpdateData::Cached(c) => {
                return match self.slot(c.cache_index)? {
                    Some(shape) => Ok(PointerEvent::Shape(shape.clone())),
                    None => Err(PointerError::CacheMiss(c.cache_index)),
                };
            }
            PointerUpdateData::Color(c) => {
                (c.cache_index, DecodedPointer::decode_color_pointer_attribute(c, target))
            }
            PointerUpdateData::New(n) => {
                (n.color_pointer.cache_index, DecodedPointer::decode_pointer_attribute(n, target))
            }
            PointerUpdateData::Large(l) => {
                (l.cache_index, DecodedPointer::decode_large_pointer_attribute(l, target))
            }
        };
        // Validate the slot before the bitmap so an out-of-range index is reported as such.
        self.slot(index)?;
        let decoded = decoded.map_err(|e| PointerError::Bitmap(e.to_string()))?;
        let shape = match CursorImage::from_decoded(&decoded) {
            Some(img) => CursorShape::Image(Arc::new(img)),
            None => CursorShape::Hidden,
        };
        if let Some(slot) = self.cache.get_mut(usize::from(index)) {
            *slot = Some(shape.clone());
        }
        Ok(PointerEvent::Shape(shape))
    }

    fn slot(&self, index: u16) -> Result<&Option<CursorShape>, PointerError> {
        self.cache
            .get(usize::from(index))
            .ok_or(PointerError::CacheIndexOutOfRange { index, size: self.cache.len() })
    }
}

fn is_pointer_code(code: UpdateCode) -> bool {
    matches!(
        code,
        UpdateCode::HiddenPointer
            | UpdateCode::DefaultPointer
            | UpdateCode::PositionPointer
            | UpdateCode::ColorPointer
            | UpdateCode::CachedPointer
            | UpdateCode::NewPointer
            | UpdateCode::LargePointer
    )
}
