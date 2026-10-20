//! # drift-video
//!
//! VideoToolbox H.264: AVC420 decode into IOSurface-backed NV12 `CVPixelBuffer`s
//! (implementing [`drift_core::video::H264Decoder`] and [`drift_core::Nv12Source`]), and
//! the recording encoder, whose MP4 writer sits behind the `recording` feature.
//!
//! | Module | Task | Kind |
//! |---|---|---|
//! | [`metablock`] | M1-3 | pure: `RFX_AVC420_METABLOCK` parser |
//! | [`annexb`] | M1-3 | pure: Annex-B ↔ AVCC, parameter-set extraction |
//! | [`params`] | M1-3 | pure: SPS/PPS change → session rebuild decision |
//! | [`quality`] | M1-3 | pure: PSNR |
//! | [`decode`] | M1-3 | FFI: `VTDecompressionSession` |
//! | [`encode`] | M8-2 | FFI: `VTCompressionSession` (recording profile) |
//! | `mp4` (feature `recording`) | M8-3 | FFI: `AVAssetWriter` passthrough |

pub mod annexb;
mod cv;
pub mod decode;
pub mod metablock;
pub mod params;
pub mod quality;
pub mod sps;

pub mod encode;
#[cfg(feature = "recording")]
pub mod mp4;

pub use decode::{DecodedPicture, VtDecoder};
pub use metablock::{Avc420Metablock, QuantQuality, parse_avc420_bitmap_stream};
