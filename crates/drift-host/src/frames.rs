//! What the host shows. A [`FrameSource`] turns a desktop size into an AVC420 access unit; the
//! skeleton's [`TestPattern`] encodes colour bars with VideoToolbox. Capture (HOST-3) and the
//! real-time encoder (HOST-4) replace it with the screen.

use std::time::Instant;

use drift_core::Size;
use drift_video::encode::{EncoderConfig, PixelBuffer, VtEncoder};

/// Producing a picture failed.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct FrameError(pub String);

/// Produces the pictures a session shows.
pub trait FrameSource: Send {
    /// One self-contained AVC420 access unit (Annex-B, a keyframe with SPS/PPS) at `size`.
    fn keyframe(&mut self, size: Size<u32>) -> Result<Vec<u8>, FrameError>;
}

/// Eight vertical colour bars over a grey ramp, encoded once per size.
#[derive(Default)]
pub struct TestPattern {
    cached: Option<(Size<u32>, Vec<u8>)>,
}

impl TestPattern {
    /// A pattern source with nothing encoded yet.
    pub fn new() -> Self {
        Self::default()
    }
}

/// BGRA of the pattern at `(x, y)` in a `size` picture.
pub fn pattern_pixel(size: Size<u32>, x: u32, y: u32) -> [u8; 4] {
    const BARS: [[u8; 3]; 8] = [
        [235, 235, 235],
        [235, 235, 16],
        [16, 235, 235],
        [16, 235, 16],
        [235, 16, 235],
        [235, 16, 16],
        [16, 16, 235],
        [16, 16, 16],
    ];
    if y >= size.height * 3 / 4 {
        let level = u8::try_from(x * 255 / size.width.max(1)).unwrap_or(255);
        return [level, level, level, 255];
    }
    let bar = usize::try_from(x * 8 / size.width.max(1)).unwrap_or(0).min(7);
    let [r, g, b] = BARS[bar];
    [b, g, r, 255]
}

impl FrameSource for TestPattern {
    fn keyframe(&mut self, size: Size<u32>) -> Result<Vec<u8>, FrameError> {
        if let Some((cached, au)) = &self.cached
            && *cached == size
        {
            return Ok(au.clone());
        }
        let err = |e: drift_video::encode::EncodeError| FrameError(e.to_string());
        let mut encoder = VtEncoder::new(EncoderConfig::new(size)).map_err(err)?;
        let picture = PixelBuffer::new_bgra(size).map_err(err)?;
        picture.fill_bgra(|x, y| pattern_pixel(size, x, y)).map_err(err)?;
        let mut frames = encoder.encode(&picture, Instant::now()).map_err(err)?;
        frames.extend(encoder.flush().map_err(err)?);
        let frame = frames
            .iter()
            .find(|f| f.is_keyframe())
            .ok_or_else(|| FrameError("the encoder produced no keyframe".into()))?;
        let au = frame.to_annex_b().map_err(err)?;
        self.cached = Some((size, au.clone()));
        Ok(au)
    }
}
