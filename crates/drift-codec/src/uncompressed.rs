//! `RDPGFX_CODECID_UNCOMPRESSED`: 32 bpp little-endian pixels, already in BGRA byte order.

use drift_core::Rect;

use crate::error::{CodecError, CodecKind};
use crate::tile::{BPP, BgraTile, validate_dest};

/// Pixel format of an uncompressed payload (`RDPGFX_PIXELFORMAT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UncompressedFormat {
    /// `PIXEL_FORMAT_XRGB_8888` (0x20): the fourth byte is undefined; output is opaque.
    Xrgb,
    /// `PIXEL_FORMAT_ARGB_8888` (0x21): the fourth byte is alpha and is kept.
    Argb,
}

/// Decodes an uncompressed `WireToSurface1` payload for `dest` into a BGRA tile.
///
/// # Errors
/// [`CodecError::InvalidRect`] for an empty `dest`, [`CodecError::SizeMismatch`] when
/// `data` is not exactly `width × height × 4` bytes.
pub fn decode_uncompressed(
    dest: Rect,
    format: UncompressedFormat,
    data: &[u8],
) -> Result<BgraTile, CodecError> {
    let (w, h) = validate_dest(CodecKind::Uncompressed, dest)?;
    let expected = usize::from(w) * usize::from(h) * BPP;
    if data.len() != expected {
        return Err(CodecError::SizeMismatch {
            codec: CodecKind::Uncompressed,
            expected,
            actual: data.len(),
        });
    }
    let mut px = data.to_vec();
    if format == UncompressedFormat::Xrgb {
        for p in px.as_chunks_mut::<BPP>().0 {
            p[3] = 0xFF;
        }
    }
    Ok(BgraTile::full(dest, px))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xrgb_is_forced_opaque_and_keeps_byte_order() {
        let data = [1, 2, 3, 0, 4, 5, 6, 7];
        let tile = decode_uncompressed(Rect::new(10, 20, 2, 1), UncompressedFormat::Xrgb, &data);
        let tile = tile.ok();
        assert_eq!(tile.as_ref().map(|t| t.data.clone()), Some(vec![1, 2, 3, 255, 4, 5, 6, 255]));
        assert_eq!(tile.map(|t| t.update_rects), Some(vec![Rect::new(10, 20, 2, 1)]));
    }

    #[test]
    fn argb_keeps_alpha() {
        let data = [1, 2, 3, 0, 4, 5, 6, 7];
        let tile = decode_uncompressed(Rect::new(0, 0, 1, 2), UncompressedFormat::Argb, &data).ok();
        assert_eq!(tile.map(|t| t.data), Some(data.to_vec()));
    }

    #[test]
    fn wrong_length_and_empty_rect_are_errors() {
        assert_eq!(
            decode_uncompressed(Rect::new(0, 0, 2, 2), UncompressedFormat::Xrgb, &[0; 15]),
            Err(CodecError::SizeMismatch { codec: CodecKind::Uncompressed, expected: 16, actual: 15 })
        );
        assert!(matches!(
            decode_uncompressed(Rect::new(0, 0, 0, 2), UncompressedFormat::Xrgb, &[]),
            Err(CodecError::InvalidRect { .. })
        ));
        assert!(matches!(
            decode_uncompressed(Rect::new(0, 0, 70_000, 1), UncompressedFormat::Xrgb, &[]),
            Err(CodecError::InvalidRect { .. })
        ));
    }
}
