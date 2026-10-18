//! # drift-video
//!
//! VideoToolbox H.264: AVC420 decode into IOSurface-backed NV12 `CVPixelBuffer`s
//! (implementing [`drift_core::video::H264Decoder`] and [`drift_core::Nv12Source`]), and
//! the encoder: a real-time profile for the Mac host (HOST-4) and the recording profile, whose
//! MP4 writer sits behind the `recording` feature.
//!
//! | Module | Task | Kind |
//! |---|---|---|
//! | [`metablock`] | M1-3, HOST-4 | pure: `RFX_AVC420_METABLOCK` / AVC444 stream parser and writers |
//! | [`annexb`] | M1-3 | pure: Annex-B ↔ AVCC, parameter-set extraction |
//! | [`params`] | M1-3 | pure: SPS/PPS change → session rebuild decision |
//! | [`quality`] | M1-3 | pure: PSNR |
//! | [`decode`] | M1-3 | FFI: `VTDecompressionSession` |
//! | [`encode`] | M8-2, HOST-4 | FFI: `VTCompressionSession` (recording and real-time profiles) |
//! | [`avc444`] | HOST-4 | pure: YUV444 ↔ main + auxiliary YUV420 views; FFI: the two-stream encoder |
//! | [`hevc`] | EXT-5 | pure: HEVC NAL types, SPS fields; FFI: encoder probe, real-time encoder, decoder |
//! | `mp4` (feature `recording`) | M8-3 | FFI: `AVAssetWriter` passthrough |

pub mod annexb;
pub mod avc444;
mod cv;
pub mod decode;
pub mod metablock;
pub mod params;
pub mod quality;
pub mod sps;

pub mod encode;
pub mod hevc;
#[cfg(feature = "recording")]
pub mod mp4;

pub use decode::{DecodedPicture, VtDecoder};
pub use metablock::{
    Avc420Metablock, Avc444Layout, QuantQuality, parse_avc420_bitmap_stream, parse_avc444_bitmap_stream,
    write_avc420_bitmap_stream, write_avc444_bitmap_stream,
};
