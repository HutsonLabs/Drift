//! `RDPGFX_CODECID_CLEARCODEC` ([MS-RDPEGFX] 2.2.4.1).

use drift_core::Rect;
use ironrdp_graphics::clearcodec::ClearCodecDecoder;

use crate::error::{CodecError, CodecKind};
use crate::tile::{BgraTile, validate_dest};

/// A ClearCodec decoder. ClearCodec keeps V-bar and glyph caches across commands, so a
/// session holds one instance for its whole lifetime (caches are reset in-band by
/// `CLEARCODEC_FLAG_CACHE_RESET`). The ~1 MiB cache spine is allocated on first use.
#[derive(Default)]
pub struct ClearCodec {
    inner: Option<ClearCodecDecoder>,
}

impl std::fmt::Debug for ClearCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClearCodec").field("allocated", &self.inner.is_some()).finish()
    }
}

impl ClearCodec {
    /// Creates a decoder; caches are allocated lazily.
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes a ClearCodec `WireToSurface1` payload for `dest` into an opaque BGRA tile.
    ///
    /// # Errors
    /// [`CodecError::InvalidRect`] for an empty or oversized `dest`, [`CodecError::Malformed`]
    /// when the stream does not decode.
    pub fn decode(&mut self, dest: Rect, data: &[u8]) -> Result<BgraTile, CodecError> {
        let (w, h) = validate_dest(CodecKind::ClearCodec, dest)?;
        let bgra = self
            .inner
            .get_or_insert_with(ClearCodecDecoder::new)
            .decode(data, w, h)
            .map_err(|e| CodecError::malformed(CodecKind::ClearCodec, e))?;
        let expected = usize::from(w) * usize::from(h) * 4;
        if bgra.len() != expected {
            return Err(CodecError::SizeMismatch {
                codec: CodecKind::ClearCodec,
                expected,
                actual: bgra.len(),
            });
        }
        Ok(BgraTile::full(dest, bgra))
    }
}

#[cfg(test)]
mod tests {
    use ironrdp_graphics::clearcodec::ClearCodecEncoder;

    use super::*;

    #[test]
    fn clearcodec_round_trips_colour_and_is_opaque() {
        let (w, h) = (6u16, 4u16);
        let bgra: Vec<u8> =
            (0..u32::from(w) * u32::from(h)).flat_map(|i| [i as u8, 0x40, 0x80, 0x10]).collect();
        let stream = ClearCodecEncoder::new().encode(&bgra, w, h);
        let mut codec = ClearCodec::new();
        assert!(format!("{codec:?}").contains("allocated: false"));
        let tile = codec.decode(Rect::new(1, 2, 6, 4), &stream).ok();
        let expected: Vec<u8> =
            bgra.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        assert_eq!(tile.as_ref().map(|t| &t.data), Some(&expected));
        assert_eq!(tile.map(|t| t.update_rects), Some(vec![Rect::new(1, 2, 6, 4)]));
        assert!(format!("{codec:?}").contains("allocated: true"));
    }

    #[test]
    fn garbage_and_bad_rects_are_errors() {
        let mut codec = ClearCodec::new();
        assert!(matches!(codec.decode(Rect::new(0, 0, 2, 2), &[1]), Err(CodecError::Malformed { .. })));
        assert!(matches!(codec.decode(Rect::new(0, 0, 0, 2), &[]), Err(CodecError::InvalidRect { .. })));
        assert!(matches!(codec.decode(Rect::new(0, 0, 70_000, 2), &[]), Err(CodecError::InvalidRect { .. })));
    }
}
