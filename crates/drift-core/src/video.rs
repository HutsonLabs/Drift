//! Decoded-video handles shared by `drift-video`, `drift-gfx` and `drift-render`.
//!
//! [`Nv12Frame`] lives here (rather than in `drift-video` or `drift-gfx`) so that the
//! `FrameSink` trait in `drift-gfx` and its Metal implementation in `drift-render` can
//! name it without depending on each other or on VideoToolbox. The frame is an opaque,
//! cheaply clonable handle around an [`Nv12Source`]:
//!
//! - `drift-video` implements [`Nv12Source`] for its IOSurface-backed `CVPixelBuffer`
//!   wrapper; `drift-render` recovers it with [`Nv12Frame::downcast_ref`] for zero-copy
//!   `CVMetalTextureCache` import.
//! - [`Nv12Planes`] is a CPU implementation used by tests and golden images.
//!
//! See `docs/adr/M0-5-nv12-frame-location.md`.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use crate::geometry::Size;

/// Something that holds one decoded NV12 (Y plane + interleaved CbCr plane) picture,
/// BT.709 full range (plan §1.4).
pub trait Nv12Source: Send + Sync + fmt::Debug + 'static {
    /// Picture size in pixels (luma plane dimensions).
    fn size(&self) -> Size<u32>;

    /// `self` as [`Any`], so consumers can downcast to the concrete backing type.
    fn as_any(&self) -> &dyn Any;
}

/// Opaque, reference-counted handle to a decoded NV12 picture.
#[derive(Clone)]
pub struct Nv12Frame {
    source: Arc<dyn Nv12Source>,
}

impl Nv12Frame {
    /// Wraps a decoded picture.
    pub fn new(source: impl Nv12Source) -> Self {
        Self { source: Arc::new(source) }
    }

    /// Picture size in pixels.
    pub fn size(&self) -> Size<u32> {
        self.source.size()
    }

    /// Returns the backing picture if it is a `T`.
    pub fn downcast_ref<T: Nv12Source>(&self) -> Option<&T> {
        self.source.as_any().downcast_ref::<T>()
    }
}

impl fmt::Debug for Nv12Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Nv12Frame").field("source", &self.source).finish()
    }
}

/// A CPU-resident NV12 picture: `y` is `width × height` bytes, `uv` is
/// `width × ceil(height/2)` bytes of interleaved Cb,Cr at half resolution.
#[derive(Clone, PartialEq, Eq)]
pub struct Nv12Planes {
    size: Size<u32>,
    y: Vec<u8>,
    uv: Vec<u8>,
}

impl Nv12Planes {
    /// Builds a picture, validating plane lengths. Width must be even.
    pub fn new(size: Size<u32>, y: Vec<u8>, uv: Vec<u8>) -> Option<Self> {
        let w = usize::try_from(size.width).ok()?;
        let h = usize::try_from(size.height).ok()?;
        if w % 2 != 0 || y.len() != w.checked_mul(h)? || uv.len() != w.checked_mul(h.div_ceil(2))? {
            return None;
        }
        Some(Self { size, y, uv })
    }

    /// Luma plane (stride = width).
    pub fn y(&self) -> &[u8] {
        &self.y
    }

    /// Interleaved chroma plane (stride = width).
    pub fn uv(&self) -> &[u8] {
        &self.uv
    }
}

impl fmt::Debug for Nv12Planes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Nv12Planes").field("size", &self.size).finish_non_exhaustive()
    }
}

impl Nv12Source for Nv12Planes {
    fn size(&self) -> Size<u32> {
        self.size
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Error from an H.264 decoder.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("H.264 decode failed: {0}")]
pub struct DecodeError(pub String);

/// AVC420 decoder seam between `drift-gfx` (caller) and `drift-video` (VideoToolbox).
///
/// `drift-gfx` hands over one Annex-B access unit per `RFX_AVC420_BITMAP_STREAM`;
/// the decoder returns the decoded picture (or `None` if the decoder needs more data).
pub trait H264Decoder: Send {
    /// Decodes one Annex-B access unit.
    fn decode(&mut self, annex_b: &[u8]) -> Result<Option<Nv12Frame>, DecodeError>;

    /// Drops decoder state (e.g. after `ResetGraphics`).
    fn reset(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planes_validate_lengths() {
        let s = Size::new(4, 3);
        assert!(Nv12Planes::new(s, vec![0; 12], vec![0; 8]).is_some());
        assert!(Nv12Planes::new(s, vec![0; 11], vec![0; 8]).is_none());
        assert!(Nv12Planes::new(s, vec![0; 12], vec![0; 4]).is_none());
        assert!(Nv12Planes::new(Size::new(3, 2), vec![0; 6], vec![0; 3]).is_none());
    }

    #[test]
    fn frame_downcasts_to_backing() {
        let p = Nv12Planes::new(Size::new(2, 2), vec![1; 4], vec![2; 2]).unwrap();
        let f = Nv12Frame::new(p.clone());
        assert_eq!(f.size(), Size::new(2, 2));
        let back = f.downcast_ref::<Nv12Planes>().unwrap();
        assert_eq!(back.y(), &[1; 4]);
        assert_eq!(back.uv(), &[2; 2]);
        assert!(format!("{f:?}").contains("Nv12Planes"));
    }
}
