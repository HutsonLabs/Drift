//! Stream configuration.

use crate::display::DisplayChoice;

/// The pixel format frames arrive in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureFormat {
    /// `420f`: NV12, full range — what VideoToolbox's H.264 encoder takes directly (AVC420).
    #[default]
    Nv12FullRange,
    /// `BGRA`: full chroma, for AVC444 (HOST-4 splits it into luma and chroma streams).
    Bgra,
}

impl CaptureFormat {
    /// The CoreVideo pixel format type.
    pub const fn fourcc(self) -> u32 {
        match self {
            Self::Nv12FullRange => u32::from_be_bytes(*b"420f"),
            Self::Bgra => u32::from_be_bytes(*b"BGRA"),
        }
    }
}

/// How to capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Which display.
    pub display: DisplayChoice,
    /// The frame format.
    pub format: CaptureFormat,
    /// Frame-rate cap (`minimumFrameInterval`).
    pub max_fps: u32,
    /// Surfaces ScreenCaptureKit may keep in flight (3–8).
    pub queue_depth: u32,
    /// More dirty rects than this become their bounding box.
    pub max_dirty_rects: usize,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            display: DisplayChoice::Main,
            format: CaptureFormat::default(),
            max_fps: 60,
            queue_depth: 4,
            max_dirty_rects: 16,
        }
    }
}

/// A configuration ScreenCaptureKit would refuse or that makes no sense.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid capture configuration: {0}")]
pub struct ConfigError(pub &'static str);

impl CaptureConfig {
    /// Checks the ranges.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=240).contains(&self.max_fps) {
            return Err(ConfigError("max_fps must be 1–240"));
        }
        if !(3..=8).contains(&self.queue_depth) {
            return Err(ConfigError("queue_depth must be 3–8"));
        }
        if self.max_dirty_rects == 0 {
            return Err(ConfigError("max_dirty_rects must be at least 1"));
        }
        Ok(())
    }

    /// `minimumFrameInterval` as a `CMTime` `(value, timescale)`.
    pub const fn min_frame_interval(&self) -> (i64, i32) {
        (1, self.max_fps as i32)
    }
}
