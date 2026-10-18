//! # drift-capture
//!
//! Screen capture for Drift Host (task HOST-3; ADR `HOST-3-capture`): one ScreenCaptureKit
//! `SCStream` per display, IOSurface-backed `CVPixelBuffer` frames ready for VideoToolbox, damage
//! from ScreenCaptureKit's dirty rects, and the pointer sampled separately (RDP sends pointer
//! position and shape as their own updates).
//!
//! | Module | Area |
//! |---|---|
//! | [`config`] | frame format, rate cap, queue depth |
//! | [`display`] | display geometry and selection |
//! | [`frame`] | `SCStreamFrameInfo` status and damage |
//! | [`rect`] | dirty rects in output pixels: clip, merge, cap |
//! | [`cursor`] | pointer shapes and change detection |
//! | `sck` (macOS) | ScreenCaptureKit: displays, streams, permission |
//! | `cursor_sample` (macOS) | the current pointer position and shape |
//!
//! The pure modules build everywhere and carry the logic; the macOS modules are humble objects.
//! Every `unsafe` block has a `// SAFETY:` comment.

pub mod config;
pub mod cursor;
pub mod display;
pub mod frame;
pub mod rect;

#[cfg(target_os = "macos")]
pub mod cursor_sample;
#[cfg(target_os = "macos")]
pub mod sck;
