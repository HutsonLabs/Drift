//! Frame metadata from ScreenCaptureKit's per-frame attachments (`SCStreamFrameInfo`).

use drift_core::Size;

use crate::rect::{Damage, PixelRect, coalesce};

/// `SCFrameStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameStatus {
    /// New content.
    Complete,
    /// Nothing changed since the previous frame.
    Idle,
    /// The display is blank.
    Blank,
    /// The stream is suspended.
    Suspended,
    /// The first frame after the stream started.
    Started,
    /// The stream stopped.
    Stopped,
}

impl FrameStatus {
    /// From the raw `SCFrameStatus` value.
    pub const fn from_raw(raw: i64) -> Option<Self> {
        Some(match raw {
            0 => Self::Complete,
            1 => Self::Idle,
            2 => Self::Blank,
            3 => Self::Suspended,
            4 => Self::Started,
            5 => Self::Stopped,
            _ => return None,
        })
    }

    /// Whether the frame carries new pixels worth encoding.
    pub const fn has_pixels(self) -> bool {
        matches!(self, Self::Complete | Self::Started)
    }
}

/// The parts of `SCStreamFrameInfo` capture uses, as plain values.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameMeta {
    /// `SCStreamFrameInfoStatus`.
    pub status: FrameStatus,
    /// Output pixels per unit of the dirty-rect coordinates.
    pub scale: f64,
    /// `SCStreamFrameInfoDirtyRects` as `(x, y, width, height)`.
    pub dirty: Vec<(f64, f64, f64, f64)>,
}

/// The damage of a frame of `size` output pixels: a started stream (or a complete frame without
/// dirty rects) is a full update, a complete frame its coalesced dirty rects, anything else none.
pub fn damage(meta: &FrameMeta, size: Size<u32>, max_rects: usize) -> Damage {
    match meta.status {
        FrameStatus::Started => Damage::Full,
        FrameStatus::Complete if meta.dirty.is_empty() => Damage::Full,
        FrameStatus::Complete => {
            let rects: Vec<PixelRect> = meta
                .dirty
                .iter()
                .filter_map(|&(x, y, w, h)| PixelRect::from_frame_coords(x, y, w, h, meta.scale, size))
                .collect();
            coalesce(&rects, size, max_rects)
        }
        _ => Damage::None,
    }
}
