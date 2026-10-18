//! The screen as a [`RawFrames`] source: `drift-capture`'s ScreenCaptureKit stream into a
//! one-slot mailbox (HOST-3, HOST-5), and the pointer sampled beside it.
//!
//! Needs Screen Recording. Without it [`ScreenFrames::start`] fails with
//! `CaptureError::NoPermission`, and the host falls back to its test pattern.

use std::time::{Duration, Instant};

use drift_capture::config::CaptureConfig;
use drift_capture::cursor::{CursorEvent, CursorTracker};
use drift_capture::display::DisplayInfo;
use drift_capture::sck::{CaptureError, CaptureStream, CapturedFrame};
use drift_core::Size;
use drift_core::mailbox::{Taker, mailbox};
use drift_inject::plan::DisplayMap;
use drift_video::encode::PixelBuffer;

use crate::pipeline::{RawFrame, RawFrames};

/// How often the pointer is sampled (it moves independently of the picture).
pub const POINTER_INTERVAL: Duration = Duration::from_millis(8);

/// The main display, captured.
pub struct ScreenFrames {
    stream: CaptureStream,
    frames: Taker<CapturedFrame>,
    tracker: CursorTracker,
    last_pointer: Option<Instant>,
}

impl std::fmt::Debug for ScreenFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenFrames").field("display", self.stream.display()).finish_non_exhaustive()
    }
}

impl ScreenFrames {
    /// Starts capturing with `config` (NV12 at up to its `max_fps`).
    pub fn start(config: CaptureConfig) -> Result<Self, CaptureError> {
        let (poster, frames) = mailbox();
        let stream = CaptureStream::start_into(config, poster)?;
        Ok(Self { stream, frames, tracker: CursorTracker::default(), last_pointer: None })
    }

    /// The display being captured.
    pub fn display(&self) -> &DisplayInfo {
        self.stream.display()
    }
}

/// Where a captured display's pixels sit in the Mac's global point space (for injection).
pub fn display_map(display: &DisplayInfo) -> DisplayMap {
    DisplayMap {
        origin: display.origin,
        desktop: (display.pixels.width, display.pixels.height),
        points_per_pixel: 1.0 / display.scale(),
    }
}

impl RawFrames for ScreenFrames {
    fn size(&self) -> Size<u32> {
        self.stream.display().pixels
    }

    fn next(&mut self, timeout: Duration) -> Option<RawFrame> {
        let frame = self.frames.take_timeout(timeout).ok()?;
        Some(RawFrame { buffer: PixelBuffer::from_cv(frame.buffer), damage: frame.damage })
    }

    fn pointer(&mut self) -> Vec<CursorEvent> {
        let now = Instant::now();
        if self.last_pointer.is_some_and(|t| now.duration_since(t) < POINTER_INTERVAL) {
            return Vec::new();
        }
        self.last_pointer = Some(now);
        let sample = drift_capture::cursor_sample::sample(self.stream.display());
        self.tracker.update(&sample)
    }
}
