//! HEVC through VideoToolbox for Drift's extension codec (task **EXT-5**, ADR `EXT-5-hevc-hdr`).
//!
//! * [`probe_encoder`] lists the HEVC profiles this machine's encoder has
//!   (`VTCopySupportedPropertyDictionaryForEncoder`): Main and Main 10 everywhere, and on Apple
//!   silicon Macs also 4:4:4 and 4:4:4 10-bit, which give full chroma in one stream.
//! * [`HevcEncoder`] is the host's real-time encoder: no frame reordering, low-latency rate
//!   control where VideoToolbox offers it, a bounded keyframe interval, keyframes on request, and
//!   BT.709 or BT.2100 (PQ / HLG, BT.2020 primaries) colour tags with HDR metadata insertion.
//!   Output is Annex-B, with VPS/SPS/PPS before every keyframe.
//! * [`VtHevcDecoder`] is the client's decoder ([`drift_core::video::HevcDecoder`]): 4:2:0 comes
//!   out as IOSurface-backed NV12 (the compositor's zero-copy path; 10-bit pictures are
//!   converted to 8 bits, so HDR shows as SDR for now), 4:4:4 as BGRA. The transfer function
//!   is read from the stream's colour description.
//!
//! The pure parts (NAL types, the SPS fields Drift needs) are at the top and tested in-module.

use std::ptr::NonNull;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use drift_core::video::{DecodeError, DecodedVideo, HevcDecoder, HevcProfile, Transfer, VideoPicture};
use drift_core::{BgraPicture, Nv12Frame, Nv12Planes, Size};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use objc2_core_media::{
    CMFormatDescription, CMTime, CMVideoFormatDescriptionCreateFromHEVCParameterSets,
    CMVideoFormatDescriptionGetHEVCParameterSetAtIndex, kCMFormatDescriptionExtension_TransferFunction,
    kCMTimeInvalid, kCMVideoCodecType_HEVC,
};
use objc2_core_video::{
    kCVImageBufferColorPrimaries_ITU_R_709_2, kCVImageBufferColorPrimaries_ITU_R_2020,
    kCVImageBufferTransferFunction_ITU_R_709_2, kCVImageBufferTransferFunction_ITU_R_2100_HLG,
    kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
    kCVImageBufferYCbCrMatrix_ITU_R_2020,
};
use objc2_video_toolbox::{
    VTCompressionSession, VTCopySupportedPropertyDictionaryForEncoder, VTDecodeFrameFlags, VTDecodeInfoFlags,
    VTDecompressionOutputCallbackRecord, VTDecompressionSession, VTSessionSetProperty,
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ColorPrimaries, kVTCompressionPropertyKey_ContentLightLevelInfo,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_HDRMetadataInsertionMode,
    kVTCompressionPropertyKey_MasteringDisplayColorVolume, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, kVTCompressionPropertyKey_ProfileLevel,
    kVTCompressionPropertyKey_RealTime, kVTCompressionPropertyKey_TransferFunction,
    kVTCompressionPropertyKey_YCbCrMatrix, kVTDecompressionPropertyKey_RealTime,
    kVTHDRMetadataInsertionMode_Auto, kVTPropertySupportedValueListKey,
    kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder,
    kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
    kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
};

use crate::annexb::{avcc_nals, split_nals};
use crate::cv::{self, SharedPixelBuffer};
use crate::decode::{DecodedPicture, OutputSlot, make_sample};
use crate::encode::{
    EncodeError, Outputs, PixelBuffer, Session, SharedSample, Timeline, force_keyframe_properties, os,
    realtime_bitrate,
};

// ---- pure: NAL units and the SPS ------------------------------------------------------------

/// HEVC NAL unit types Drift looks at (H.265 table 7-1).
pub mod nal {
    /// Video parameter set.
    pub const VPS: u8 = 32;
    /// Sequence parameter set.
    pub const SPS: u8 = 33;
    /// Picture parameter set.
    pub const PPS: u8 = 34;
    /// Access unit delimiter.
    pub const AUD: u8 = 35;
}

/// The `nal_unit_type` of an HEVC NAL unit (bits 1..=6 of the first header byte).
pub fn nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| (b >> 1) & 0x3F)
}

/// The NAL unit types of an Annex-B access unit, in order.
pub fn nal_types(annex_b: &[u8]) -> Vec<u8> {
    split_nals(annex_b).into_iter().filter_map(nal_type).collect()
}

/// An intra random access point picture (BLA, IDR or CRA: types 16..=23).
pub const fn is_irap(t: u8) -> bool {
    t >= 16 && t <= 23
}

/// A picture (VCL) NAL unit.
pub const fn is_vcl(t: u8) -> bool {
    t < 32
}

/// The fields of an SPS Drift needs to pick the decoder's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpsInfo {
    /// 0 monochrome, 1 4:2:0, 2 4:2:2, 3 4:4:4.
    pub chroma_format_idc: u32,
    /// Luma bit depth.
    pub bit_depth: u32,
}

/// Bits of an RBSP (emulation prevention removed), most significant first.
struct Bits {
    data: Vec<u8>,
    pos: usize,
}

impl Bits {
    fn new(nal_payload: &[u8]) -> Self {
        let mut data = Vec::with_capacity(nal_payload.len());
        let mut zeros = 0;
        for &b in nal_payload {
            if zeros >= 2 && b == 3 {
                zeros = 0;
                continue;
            }
            zeros = if b == 0 { zeros + 1 } else { 0 };
            data.push(b);
        }
        Self { data, pos: 0 }
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(u32::from(bit))
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        self.pos = self.pos.checked_add(n)?;
        (self.pos <= self.data.len() * 8).then_some(())
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let rest = self.bits(zeros)?;
        Some((1u32 << zeros) - 1 + rest)
    }
}

/// Reads `chroma_format_idc` and the luma bit depth from an SPS NAL unit (with its header).
pub fn sps_info(sps: &[u8]) -> Option<SpsInfo> {
    if nal_type(sps)? != nal::SPS {
        return None;
    }
    let mut b = Bits::new(sps.get(2..)?);
    b.bits(4)?; // sps_video_parameter_set_id
    let max_sub_layers_minus1 = b.bits(3)?;
    b.bit()?; // sps_temporal_id_nesting_flag
    // profile_tier_level(1, max_sub_layers_minus1): general part is 88 bits + level_idc 8.
    b.skip(96)?;
    let mut sub_profile = Vec::new();
    let mut sub_level = Vec::new();
    for _ in 0..max_sub_layers_minus1 {
        sub_profile.push(b.bit()?);
        sub_level.push(b.bit()?);
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            b.bits(2)?; // reserved_zero_2bits
        }
    }
    for (p, l) in sub_profile.iter().zip(&sub_level) {
        if *p == 1 {
            b.skip(88)?;
        }
        if *l == 1 {
            b.skip(8)?;
        }
    }
    b.ue()?; // sps_seq_parameter_set_id
    let chroma_format_idc = b.ue()?;
    if chroma_format_idc == 3 {
        b.bit()?; // separate_colour_plane_flag
    }
    b.ue()?; // pic_width_in_luma_samples
    b.ue()?; // pic_height_in_luma_samples
    if b.bit()? == 1 {
        for _ in 0..4 {
            b.ue()?; // conformance window offsets
        }
    }
    let bit_depth = b.ue()?.checked_add(8)?;
    (chroma_format_idc <= 3).then_some(SpsInfo { chroma_format_idc, bit_depth })
}

// ---- the encoder ------------------------------------------------------------------------------

/// The HEVC profile-level names VideoToolbox lists (the `AutoLevel` ones).
fn profile_name(profile: HevcProfile) -> &'static str {
    match profile {
        HevcProfile::Main => "HEVC_Main_AutoLevel",
        HevcProfile::Main10 => "HEVC_Main10_AutoLevel",
        HevcProfile::Main444 => "HEVC_Main444_AutoLevel",
        HevcProfile::Main44410 => "HEVC_Main44410_AutoLevel",
    }
}

/// The HEVC profiles this machine's encoder supports at `size`, from the plainest; empty when
/// there is no HEVC encoder.
pub fn probe_encoder(size: Size<u32>) -> Vec<HevcProfile> {
    let (Ok(w), Ok(h)) = (i32::try_from(size.width), i32::try_from(size.height)) else { return Vec::new() };
    let mut id: *const CFString = std::ptr::null();
    let mut dict: *const CFDictionary = std::ptr::null();
    // SAFETY: valid dimensions and codec; both out-pointers are valid; on success both results
    // are +1 retained (Copy rule).
    let status = unsafe {
        VTCopySupportedPropertyDictionaryForEncoder(w, h, kCMVideoCodecType_HEVC, None, &mut id, &mut dict)
    };
    if let Some(id) = NonNull::new(id.cast_mut()) {
        // SAFETY: +1 retained (Copy rule); released here.
        drop(unsafe { CFRetained::from_raw(id) });
    }
    let Some(dict) = NonNull::new(dict.cast_mut()) else { return Vec::new() };
    // SAFETY: +1 retained (Copy rule).
    let dict = unsafe { CFRetained::from_raw(dict) };
    if status != 0 {
        return Vec::new();
    }
    let names = supported_profile_names(&dict);
    HevcProfile::ALL.into_iter().filter(|p| names.iter().any(|n| n == profile_name(*p))).collect()
}

/// The `SupportedValueList` of `ProfileLevel` in an encoder's supported-property dictionary.
fn supported_profile_names(dict: &CFDictionary) -> Vec<String> {
    // SAFETY: framework constant keys; lookups on valid dictionaries; values are borrowed.
    unsafe {
        let entry = dict.value(std::ptr::from_ref(kVTCompressionPropertyKey_ProfileLevel).cast());
        let Some(entry) = entry.cast::<CFDictionary>().as_ref() else { return Vec::new() };
        let list = entry.value(std::ptr::from_ref(kVTPropertySupportedValueListKey).cast());
        let Some(list) = list.cast::<CFArray>().as_ref() else { return Vec::new() };
        (0..list.count())
            .filter_map(|i| list.value_at_index(i).cast::<CFString>().as_ref().map(ToString::to_string))
            .collect()
    }
}

/// What the host encodes with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HevcConfig {
    /// Picture size in pixels (follows the frames).
    pub size: Size<u32>,
    /// Expected frame rate.
    pub fps: u32,
    /// The profile.
    pub profile: HevcProfile,
    /// SDR (BT.709) or HDR (BT.2100 PQ/HLG, BT.2020 primaries).
    pub transfer: Transfer,
    /// Average bit rate (bit/s).
    pub average_bitrate: u32,
    /// A keyframe at least this often.
    pub max_keyframe_interval: Duration,
}

impl HevcConfig {
    /// The real-time profile: H.264's bit rate for the size (HEVC spends it on better pictures),
    /// half again for 4:4:4's extra chroma, a keyframe at least every 4 s.
    pub fn realtime(size: Size<u32>, fps: u32, profile: HevcProfile, transfer: Transfer) -> Self {
        let base = realtime_bitrate(size, fps);
        let average_bitrate = if profile.full_chroma() { base.saturating_add(base / 2) } else { base };
        Self { size, fps, profile, transfer, average_bitrate, max_keyframe_interval: Duration::from_secs(4) }
    }
}

/// One encoded HEVC picture.
#[derive(Debug, Clone)]
pub struct HevcFrame {
    annex_b: Vec<u8>,
    keyframe: bool,
    pts: Duration,
    completed_at: Instant,
}

impl HevcFrame {
    /// The access unit, Annex-B (VPS/SPS/PPS first on keyframes).
    pub fn annex_b(&self) -> &[u8] {
        &self.annex_b
    }

    /// An intra random access picture.
    pub fn is_keyframe(&self) -> bool {
        self.keyframe
    }

    /// Presentation time relative to the first frame.
    pub fn pts(&self) -> Duration {
        self.pts
    }

    /// When VideoToolbox finished the picture.
    pub fn completed_at(&self) -> Instant {
        self.completed_at
    }

    /// Consumes the frame, returning the access unit.
    pub fn into_annex_b(self) -> Vec<u8> {
        self.annex_b
    }
}

/// `kCVPixelFormatType_444YpCbCr8BiPlanarFullRange` (`'444f'`).
const YUV444_8: u32 = 0x3434_3466;
/// `kCVPixelFormatType_444YpCbCr10BiPlanarFullRange` (`'xf44'`).
const YUV444_10: u32 = 0x7866_3434;
/// `kCVPixelFormatType_420YpCbCr10BiPlanarFullRange` (`'xf20'`).
const YUV420_10: u32 = 0x7866_3230;

/// The source format a profile needs: VideoToolbox encodes what it is given, so a BGRA or NV12
/// frame would come out 8-bit 4:2:0 whatever the profile says. `None`: encode the frame as is.
const fn source_format(profile: HevcProfile) -> Option<u32> {
    match profile {
        HevcProfile::Main => None,
        HevcProfile::Main10 => Some(YUV420_10),
        HevcProfile::Main444 => Some(YUV444_8),
        HevcProfile::Main44410 => Some(YUV444_10),
    }
}

/// Converts frames to the profile's source format on VideoToolbox's pixel-transfer path (the
/// GPU or the media engine, not a CPU loop).
struct PixelConverter {
    session: CFRetained<objc2_video_toolbox::VTPixelTransferSession>,
    format: u32,
}

impl PixelConverter {
    fn new(format: u32) -> Result<Self, EncodeError> {
        let mut out: *mut objc2_video_toolbox::VTPixelTransferSession = std::ptr::null_mut();
        // SAFETY: `out` is a valid out-pointer.
        let status =
            unsafe { objc2_video_toolbox::VTPixelTransferSession::create(None, NonNull::from(&mut out)) };
        os("VTPixelTransferSessionCreate", status)?;
        let out =
            NonNull::new(out).ok_or(EncodeError::Os { call: "VTPixelTransferSessionCreate", status: -1 })?;
        // SAFETY: Create rule: +1 retained.
        Ok(Self { session: unsafe { CFRetained::from_raw(out) }, format })
    }

    fn convert(&self, frame: &PixelBuffer) -> Result<PixelBuffer, EncodeError> {
        if cv::format_of(&frame.buffer.0) == self.format {
            return Ok(frame.clone());
        }
        let dest = cv::create_pixel_buffer(frame.size(), self.format)
            .map_err(|status| EncodeError::Os { call: "CVPixelBufferCreate", status })?;
        // SAFETY: valid session and buffers of the same size.
        let status = unsafe { self.session.transfer_image(&frame.buffer.0, &dest) };
        os("VTPixelTransferSessionTransferImage", status)?;
        Ok(PixelBuffer::from_cv(dest))
    }
}

impl Drop for PixelConverter {
    fn drop(&mut self) {
        // SAFETY: valid session; nothing uses it afterwards.
        unsafe { self.session.invalidate() };
    }
}

/// The host's real-time HEVC encoder.
pub struct HevcEncoder {
    config: HevcConfig,
    transfer: Option<PixelConverter>,
    timeline: Timeline,
    session: Option<Session>,
    force_keyframe: bool,
    // After `session` so sessions are invalidated before the callback target is freed.
    outputs: Box<Outputs>,
}

impl std::fmt::Debug for HevcEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HevcEncoder").field("config", &self.config).finish_non_exhaustive()
    }
}

// SAFETY: as `VtEncoder`: driven through `&mut self` only; callback results behind a Mutex.
unsafe impl Send for HevcEncoder {}

impl HevcEncoder {
    /// Creates the encoder and its first compression session.
    pub fn new(config: HevcConfig) -> Result<Self, EncodeError> {
        let transfer = source_format(config.profile).map(PixelConverter::new).transpose()?;
        let mut enc = Self {
            config,
            transfer,
            timeline: Timeline::new(),
            session: None,
            force_keyframe: true,
            outputs: Box::new(Mutex::new(Vec::new())),
        };
        enc.rebuild(config.size)?;
        Ok(enc)
    }

    /// The configuration (its size follows the last frame).
    pub fn config(&self) -> HevcConfig {
        self.config
    }

    /// Whether VideoToolbox's low-latency rate control is on.
    pub fn is_low_latency(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.low_latency)
    }

    /// Changes the average bit rate of the running session (EXT-4 quality steps).
    pub fn set_average_bitrate(&mut self, bitrate: u32) -> Result<(), EncodeError> {
        self.config.average_bitrate = bitrate;
        let Some(session) = &self.session else { return Ok(()) };
        let value = CFNumber::new_i32(i32::try_from(bitrate).unwrap_or(i32::MAX));
        // SAFETY: a valid session, a constant key and a CFNumber value.
        let status = unsafe {
            VTSessionSetProperty(
                &session.session,
                kVTCompressionPropertyKey_AverageBitRate,
                Some(value.as_ref()),
            )
        };
        os("VTSessionSetProperty", status)
    }

    /// Makes the next picture a keyframe.
    pub fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    /// Encodes one frame (NV12 or BGRA) captured at `at`; returns the pictures that completed.
    pub fn encode(&mut self, frame: &PixelBuffer, at: Instant) -> Result<Vec<HevcFrame>, EncodeError> {
        let converted = match &self.transfer {
            Some(t) => Some(t.convert(frame)?),
            None => None,
        };
        let frame = converted.as_ref().unwrap_or(frame);
        let size = frame.size();
        if size.width == 0 || size.height == 0 {
            return Err(EncodeError::InvalidFrame(format!("{}x{}", size.width, size.height)));
        }
        let pts = self.timeline.pts(at)?;
        if self.session.as_ref().is_none_or(|s| s.size != size) {
            if let Some(old) = &self.session {
                old.finish()?;
            }
            self.rebuild(size)?;
        }
        let Some(session) = &self.session else {
            return Err(EncodeError::InvalidFrame("no compression session".into()));
        };
        let props = self.force_keyframe.then(force_keyframe_properties);
        let micros =
            i64::try_from(pts.as_micros()).map_err(|_| EncodeError::InvalidFrame("pts overflow".into()))?;
        // SAFETY: CMTimeMake has no preconditions.
        let cm_pts = unsafe { CMTime::new(micros, 1_000_000) };
        // SAFETY: valid session and pixel buffer; variable frame rate; the properties dictionary
        // lives for the call; no refcon and no info out-pointer.
        let status = unsafe {
            session.session.encode_frame(
                &frame.buffer.0,
                cm_pts,
                kCMTimeInvalid,
                props.as_deref().map(|d| d.as_opaque()),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        os("VTCompressionSessionEncodeFrame", status)?;
        self.force_keyframe = false;
        self.drain()
    }

    /// Emits every pending picture.
    pub fn flush(&mut self) -> Result<Vec<HevcFrame>, EncodeError> {
        if let Some(session) = &self.session {
            session.finish()?;
        }
        self.drain()
    }

    fn drain(&mut self) -> Result<Vec<HevcFrame>, EncodeError> {
        let raw = std::mem::take(&mut *crate::encode::lock(&self.outputs));
        raw.into_iter()
            .map(|r| {
                let (sample, at) =
                    r.map_err(|status| EncodeError::Os { call: "VTCompressionOutputCallback", status })?;
                hevc_frame(&sample, at)
            })
            .collect()
    }

    fn rebuild(&mut self, size: Size<u32>) -> Result<(), EncodeError> {
        self.session = None;
        let config = HevcConfig { size, ..self.config };
        // Low-latency rate control makes each encode call wait for its picture; above
        // `LOW_LATENCY_MAX_PIXELS` that costs the frame rate (measured for H.264 in HOST-4 and
        // for HEVC in EXT-5), so large pictures use the normal one.
        let low_latency = crate::encode::low_latency_for(size);
        self.session = Some(match create_session(&config, &self.outputs, low_latency) {
            Ok(s) => s,
            Err(e) if low_latency => {
                tracing::debug!(error = %e, "HEVC low-latency rate control unavailable; using the normal one");
                create_session(&config, &self.outputs, false)?
            }
            Err(e) => return Err(e),
        });
        self.config = config;
        self.force_keyframe = true;
        Ok(())
    }
}

/// SMPTE ST 2086 mastering display colour volume for P3-D65 at 1000 nits (big-endian u16s:
/// G, B, R primaries, white point, then u32 max and min luminance in 0.0001 cd/m²), as Apple's
/// HDR displays are mastered.
const MASTERING_DISPLAY: [u8; 24] = [
    0x33, 0xC2, 0x86, 0xC4, // green 0.265, 0.690
    0x1D, 0x4C, 0x0B, 0xB8, // blue 0.150, 0.060
    0x84, 0xD0, 0x3E, 0x80, // red 0.680, 0.320
    0x3D, 0x13, 0x40, 0x42, // white 0.3127, 0.3290
    0x00, 0x98, 0x96, 0x80, // max 1000 cd/m²
    0x00, 0x00, 0x00, 0x32, // min 0.005 cd/m²
];

/// Content light level: max 1000 cd/m², max frame average 400 cd/m² (big-endian u16s).
const CONTENT_LIGHT: [u8; 4] = [0x03, 0xE8, 0x01, 0x90];

fn create_session(config: &HevcConfig, outputs: &Outputs, low_latency: bool) -> Result<Session, EncodeError> {
    let width = i32::try_from(config.size.width).map_err(|_| EncodeError::InvalidFrame("width".into()))?;
    let height = i32::try_from(config.size.height).map_err(|_| EncodeError::InvalidFrame("height".into()))?;
    let yes = CFBoolean::new(true);
    let on: &CFType = yes.as_ref();
    // SAFETY: framework constant keys.
    let (hw, ll) = unsafe {
        (
            kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
            kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
        )
    };
    let spec = if low_latency {
        CFDictionary::from_slices(&[hw, ll], &[on, on])
    } else {
        CFDictionary::from_slices(&[hw], &[on])
    };
    let mut out: *mut VTCompressionSession = std::ptr::null_mut();
    // SAFETY: valid dimensions, codec and dictionary; the refcon outlives the session (owned by
    // `HevcEncoder`, dropped after its sessions); `out` is a valid out-pointer.
    let status = unsafe {
        VTCompressionSession::create(
            None,
            width,
            height,
            kCMVideoCodecType_HEVC,
            Some(spec.as_opaque()),
            None,
            None,
            Some(crate::encode::output_callback),
            std::ptr::from_ref(outputs).cast_mut().cast(),
            NonNull::from(&mut out),
        )
    };
    os("VTCompressionSessionCreate (HEVC)", status)?;
    let out = NonNull::new(out).ok_or(EncodeError::Os { call: "VTCompressionSessionCreate", status: -1 })?;
    // SAFETY: Create rule: +1 retained session.
    let session = Session { session: unsafe { CFRetained::from_raw(out) }, size: config.size, low_latency };

    // The profile-level constants are these strings; the 4:4:4 ones have no SDK binding, so all
    // are built from the names `probe_encoder` found in the encoder's list.
    let profile = CFString::from_str(profile_name(config.profile));
    let bitrate = CFNumber::new_i32(i32::try_from(config.average_bitrate).unwrap_or(i32::MAX));
    let gop_seconds = CFNumber::new_f64(config.max_keyframe_interval.as_secs_f64());
    let gop_frames = CFNumber::new_i32(
        i32::try_from(config.max_keyframe_interval.as_secs().saturating_mul(u64::from(config.fps.max(1))))
            .unwrap_or(i32::MAX),
    );
    let fps = CFNumber::new_i32(i32::try_from(config.fps).unwrap_or(60));
    let no = CFBoolean::new(false);
    // SAFETY: framework constant colour tags.
    let (primaries, transfer, matrix) = unsafe {
        match config.transfer {
            Transfer::Sdr => (
                kCVImageBufferColorPrimaries_ITU_R_709_2,
                kCVImageBufferTransferFunction_ITU_R_709_2,
                kCVImageBufferYCbCrMatrix_ITU_R_709_2,
            ),
            Transfer::Pq => (
                kCVImageBufferColorPrimaries_ITU_R_2020,
                kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ,
                kCVImageBufferYCbCrMatrix_ITU_R_2020,
            ),
            Transfer::Hlg => (
                kCVImageBufferColorPrimaries_ITU_R_2020,
                kCVImageBufferTransferFunction_ITU_R_2100_HLG,
                kCVImageBufferYCbCrMatrix_ITU_R_2020,
            ),
        }
    };
    // SAFETY: framework constant keys.
    let mut properties: Vec<(&CFString, &CFType)> = unsafe {
        vec![
            (kVTCompressionPropertyKey_RealTime, yes.as_ref()),
            (kVTCompressionPropertyKey_ProfileLevel, profile.as_ref()),
            (kVTCompressionPropertyKey_AllowFrameReordering, no.as_ref()),
            (kVTCompressionPropertyKey_AverageBitRate, bitrate.as_ref()),
            (kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, gop_seconds.as_ref()),
            (kVTCompressionPropertyKey_MaxKeyFrameInterval, gop_frames.as_ref()),
            (kVTCompressionPropertyKey_ExpectedFrameRate, fps.as_ref()),
            (kVTCompressionPropertyKey_ColorPrimaries, primaries.as_ref()),
            (kVTCompressionPropertyKey_TransferFunction, transfer.as_ref()),
            (kVTCompressionPropertyKey_YCbCrMatrix, matrix.as_ref()),
        ]
    };
    let mastering = CFData::from_bytes(&MASTERING_DISPLAY);
    let light = CFData::from_bytes(&CONTENT_LIGHT);
    if config.transfer == Transfer::Pq {
        // SAFETY: framework constant keys and values.
        unsafe {
            properties.push((
                kVTCompressionPropertyKey_HDRMetadataInsertionMode,
                kVTHDRMetadataInsertionMode_Auto.as_ref(),
            ));
            properties.push((kVTCompressionPropertyKey_MasteringDisplayColorVolume, mastering.as_ref()));
            properties.push((kVTCompressionPropertyKey_ContentLightLevelInfo, light.as_ref()));
        }
    }
    for (key, value) in properties {
        // SAFETY: valid session, key and value.
        let status = unsafe { VTSessionSetProperty(&session.session, key, Some(value)) };
        if status != 0 {
            tracing::warn!(status, key = %key, "VTSessionSetProperty (HEVC) failed");
            os("VTSessionSetProperty", status)?;
        }
    }
    // SAFETY: valid session.
    let status = unsafe { session.session.prepare_to_encode_frames() };
    os("VTCompressionSessionPrepareToEncodeFrames", status)?;
    Ok(session)
}

/// The VPS/SPS/PPS of an HEVC format description.
fn hevc_parameter_sets(desc: &CMFormatDescription) -> Result<Vec<Vec<u8>>, EncodeError> {
    let mut count = 0usize;
    // SAFETY: valid HEVC description; only the count is asked for.
    let status = unsafe {
        CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
            desc,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut count,
            std::ptr::null_mut(),
        )
    };
    os("CMVideoFormatDescriptionGetHEVCParameterSetAtIndex", status)?;
    let mut sets = Vec::with_capacity(count);
    for index in 0..count {
        let mut ptr: *const u8 = std::ptr::null();
        let mut len = 0usize;
        // SAFETY: valid description, index below the count; the pointer stays valid while
        // `desc` lives and is copied at once.
        let status = unsafe {
            CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
                desc,
                index,
                &mut ptr,
                &mut len,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        os("CMVideoFormatDescriptionGetHEVCParameterSetAtIndex", status)?;
        if ptr.is_null() {
            return Err(EncodeError::BadSample("null HEVC parameter set".into()));
        }
        // SAFETY: CoreMedia returned `len` readable bytes at `ptr`.
        sets.push(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec());
    }
    Ok(sets)
}

fn hevc_frame(sample: &SharedSample, completed_at: Instant) -> Result<HevcFrame, EncodeError> {
    // SAFETY: valid sample buffer.
    let pts = unsafe { sample.0.presentation_time_stamp() };
    // SAFETY: CMTimeGetSeconds has no preconditions.
    let seconds = unsafe { pts.seconds() };
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(EncodeError::BadSample("invalid presentation time".into()));
    }
    // SAFETY: valid sample buffer; the block buffer is retained.
    let block = unsafe { sample.0.data_buffer() }.ok_or_else(|| EncodeError::BadSample("no data".into()))?;
    // SAFETY: valid block buffer.
    let len = unsafe { block.data_length() };
    let mut sized = vec![0u8; len];
    if let Some(dest) = NonNull::new(sized.as_mut_ptr())
        && len > 0
    {
        // SAFETY: `sized` has exactly `len` writable bytes; the block holds `len` bytes.
        let status = unsafe { block.copy_data_bytes(0, len, dest.cast()) };
        os("CMBlockBufferCopyDataBytes", status)?;
    }
    let nals = avcc_nals(&sized, 4).map_err(|e| EncodeError::BadSample(e.to_string()))?;
    let keyframe = nals.iter().filter_map(|n| nal_type(n)).any(is_irap);
    let mut annex_b = Vec::with_capacity(len + 256);
    if keyframe {
        // SAFETY: the sample's description is retained by the call.
        let desc = unsafe { sample.0.format_description() }
            .ok_or_else(|| EncodeError::BadSample("no format description".into()))?;
        for set in hevc_parameter_sets(&desc)? {
            annex_b.extend_from_slice(&[0, 0, 0, 1]);
            annex_b.extend_from_slice(&set);
        }
    }
    for nal in nals {
        annex_b.extend_from_slice(&[0, 0, 0, 1]);
        annex_b.extend_from_slice(nal);
    }
    Ok(HevcFrame { annex_b, keyframe, pts: Duration::from_secs_f64(seconds), completed_at })
}

// ---- the decoder ------------------------------------------------------------------------------

/// What the session hands out: NV12 for 4:2:0 (zero-copy into the compositor), BGRA otherwise.
fn output_format(info: Option<SpsInfo>) -> u32 {
    match info {
        Some(SpsInfo { chroma_format_idc: 1 | 0, .. }) | None => cv::NV12_FULL_RANGE,
        Some(_) => cv::BGRA,
    }
}

struct DecodeSession {
    inner: crate::decode::Session,
    output: u32,
    transfer: Transfer,
}

/// The client's HEVC decoder (EXT-5).
pub struct VtHevcDecoder {
    params: Option<Vec<Vec<u8>>>,
    session: Option<DecodeSession>,
    // After `session`: the session is invalidated before the slot is freed.
    slot: Box<OutputSlot>,
}

impl std::fmt::Debug for VtHevcDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VtHevcDecoder").field("session", &self.session.is_some()).finish_non_exhaustive()
    }
}

// SAFETY: as `VtDecoder`: driven through `&mut self` only; the output slot is a Mutex.
unsafe impl Send for VtHevcDecoder {}

impl Default for VtHevcDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl VtHevcDecoder {
    /// A decoder; the session is built from the first VPS/SPS/PPS.
    pub fn new() -> Self {
        Self { params: None, session: None, slot: Box::new(Mutex::new(None)) }
    }

    fn rebuild(&mut self, params: &[Vec<u8>]) -> Result<(), DecodeError> {
        self.session = None;
        let format = hevc_format_description(params)?;
        let info = params.iter().find(|p| nal_type(p) == Some(nal::SPS)).and_then(|s| sps_info(s));
        let output = output_format(info);
        // SAFETY: framework constant key.
        let spec_key: [&CFString; 1] =
            [unsafe { kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder }];
        let spec_val: [&CFType; 1] = [CFBoolean::new(true).as_ref()];
        let spec = CFDictionary::from_slices(&spec_key, &spec_val);
        let attrs = cv::pixel_buffer_attributes(output);
        let record = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(crate::decode::output_callback),
            decompressionOutputRefCon: std::ptr::from_ref(&*self.slot).cast_mut().cast(),
        };
        let mut out: *mut VTDecompressionSession = std::ptr::null_mut();
        // SAFETY: valid CF arguments; `record` is copied; the refcon outlives the session (the
        // session field is dropped before the slot); `out` is a valid out-pointer.
        let status = unsafe {
            VTDecompressionSession::create(
                None,
                &format,
                Some(spec.as_opaque()),
                Some(&attrs),
                &record,
                NonNull::from(&mut out),
            )
        };
        if status != 0 {
            return Err(DecodeError(format!("VTDecompressionSessionCreate (HEVC): OSStatus {status}")));
        }
        let out = NonNull::new(out).ok_or_else(|| DecodeError("null HEVC decompression session".into()))?;
        // SAFETY: Create rule: +1 retained session.
        let session = unsafe { CFRetained::from_raw(out) };
        // SAFETY: valid session, framework key and CFBoolean value.
        let status = unsafe {
            VTSessionSetProperty(
                &session,
                kVTDecompressionPropertyKey_RealTime,
                Some(CFBoolean::new(true).as_ref()),
            )
        };
        if status != 0 {
            tracing::debug!(status, "kVTDecompressionPropertyKey_RealTime not supported (HEVC)");
        }
        let transfer = transfer_of(&format);
        self.session =
            Some(DecodeSession { inner: crate::decode::Session { session, format }, output, transfer });
        Ok(())
    }

    /// Decodes one Annex-B access unit.
    pub fn decode_video(&mut self, annex_b: &[u8]) -> Result<Option<DecodedVideo>, DecodeError> {
        let nals = split_nals(annex_b);
        let params: Vec<Vec<u8>> = nals
            .iter()
            .filter(|n| matches!(nal_type(n), Some(nal::VPS | nal::SPS | nal::PPS)))
            .map(|n| n.to_vec())
            .collect();
        let has = |t: u8| params.iter().any(|p| nal_type(p) == Some(t));
        if has(nal::VPS) && has(nal::SPS) && has(nal::PPS) && self.params.as_ref() != Some(&params) {
            if let Err(e) = self.rebuild(&params) {
                self.params = None;
                return Err(e);
            }
            self.params = Some(params);
        }
        let pictures: Vec<&[u8]> = nals.iter().copied().filter(|n| nal_type(n).is_some_and(is_vcl)).collect();
        if pictures.is_empty() {
            return Ok(None);
        }
        let Some(session) = &self.session else { return Ok(None) };
        let mut sized = Vec::with_capacity(annex_b.len());
        for nal in
            nals.iter().filter(|n| !matches!(nal_type(n), Some(nal::VPS | nal::SPS | nal::PPS | nal::AUD)))
        {
            let len = u32::try_from(nal.len()).map_err(|_| DecodeError("NAL unit too large".into()))?;
            sized.extend_from_slice(&len.to_be_bytes());
            sized.extend_from_slice(nal);
        }
        let sample = make_sample(&sized, &session.inner.format)?;
        *crate::decode::lock(&self.slot) = None;
        let mut info = VTDecodeInfoFlags::empty();
        // SAFETY: valid session and sample; synchronous decode (no async flag), so the callback
        // runs before this returns; `info` is a valid out-pointer.
        let status = unsafe {
            session.inner.session.decode_frame(
                &sample,
                VTDecodeFrameFlags::empty(),
                std::ptr::null_mut(),
                &mut info,
            )
        };
        if status != 0 {
            return Err(DecodeError(format!("VTDecompressionSessionDecodeFrame (HEVC): OSStatus {status}")));
        }
        // SAFETY: valid session.
        unsafe { session.inner.session.wait_for_asynchronous_frames() };
        let buffer = match crate::decode::lock(&self.slot).take() {
            Some(Ok(buffer)) => buffer,
            Some(Err(status)) => return Err(DecodeError(format!("HEVC decode callback: OSStatus {status}"))),
            None if info.contains(VTDecodeInfoFlags::FrameDropped) => return Ok(None),
            None => return Err(DecodeError("VideoToolbox produced no HEVC picture".into())),
        };
        let picture = picture_from(buffer, session.output)?;
        Ok(Some(DecodedVideo { picture, transfer: session.transfer }))
    }
}

fn picture_from(buffer: SharedPixelBuffer, output: u32) -> Result<VideoPicture, DecodeError> {
    let size = cv::size_of(&buffer.0);
    if output == cv::BGRA {
        let data = cv::read_bgra(&buffer.0).map_err(DecodeError)?;
        let picture =
            BgraPicture::new(size, data).ok_or_else(|| DecodeError("BGRA picture size mismatch".into()))?;
        Ok(VideoPicture::Bgra(picture))
    } else {
        Ok(VideoPicture::Nv12(Nv12Frame::new(DecodedPicture { buffer, size })))
    }
}

impl HevcDecoder for VtHevcDecoder {
    fn decode(&mut self, annex_b: &[u8]) -> Result<Option<DecodedVideo>, DecodeError> {
        self.decode_video(annex_b)
    }

    fn reset(&mut self) {
        self.session = None;
        self.params = None;
        *crate::decode::lock(&self.slot) = None;
    }
}

/// An HEVC format description (4-byte NAL lengths) from VPS/SPS/PPS.
fn hevc_format_description(params: &[Vec<u8>]) -> Result<CFRetained<CMFormatDescription>, DecodeError> {
    if params.iter().any(Vec::is_empty) {
        return Err(DecodeError("empty HEVC parameter set".into()));
    }
    let pointers: Vec<NonNull<u8>> = params.iter().map(|p| NonNull::from(&p[0])).collect();
    let sizes: Vec<usize> = params.iter().map(Vec::len).collect();
    let mut out: *const CMFormatDescription = std::ptr::null();
    let ptrs =
        NonNull::new(pointers.as_ptr().cast_mut()).ok_or_else(|| DecodeError("no parameter sets".into()))?;
    let lens =
        NonNull::new(sizes.as_ptr().cast_mut()).ok_or_else(|| DecodeError("no parameter sets".into()))?;
    // SAFETY: `params.len()` valid pointers with matching sizes, alive for the call; CoreMedia
    // copies them; `out` is a valid out-pointer.
    let status = unsafe {
        CMVideoFormatDescriptionCreateFromHEVCParameterSets(
            None,
            params.len(),
            ptrs,
            lens,
            4,
            None,
            NonNull::from(&mut out),
        )
    };
    if status != 0 {
        return Err(DecodeError(format!(
            "CMVideoFormatDescriptionCreateFromHEVCParameterSets: OSStatus {status}"
        )));
    }
    let out =
        NonNull::new(out.cast_mut()).ok_or_else(|| DecodeError("null HEVC format description".into()))?;
    // SAFETY: Create rule: +1 retained.
    Ok(unsafe { CFRetained::from_raw(out) })
}

/// The transfer function the stream's colour description signals.
fn transfer_of(format: &CMFormatDescription) -> Transfer {
    // SAFETY: framework constant key; the extension is borrowed from a live description.
    let Some(ext) = (unsafe { format.extension(kCMFormatDescriptionExtension_TransferFunction) }) else {
        return Transfer::Sdr;
    };
    let ext: &CFType = ext.as_ref();
    let Some(name) = ext.downcast_ref::<CFString>() else { return Transfer::Sdr };
    // SAFETY: framework constants.
    let (pq, hlg) = unsafe {
        (kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ, kCVImageBufferTransferFunction_ITU_R_2100_HLG)
    };
    if name == pq {
        Transfer::Pq
    } else if name == hlg {
        Transfer::Hlg
    } else {
        Transfer::Sdr
    }
}

/// A decoded picture as BGRA, `width × 4` bytes per row (tests, thumbnails): BGRA pictures as
/// they are, NV12 converted with BT.709 full range.
pub fn to_bgra(video: &DecodedVideo) -> Result<Vec<u8>, DecodeError> {
    match &video.picture {
        VideoPicture::Bgra(p) => Ok(p.data().to_vec()),
        VideoPicture::Nv12(frame) => {
            let planes = if let Some(p) = frame.downcast_ref::<DecodedPicture>() {
                p.to_planes()?
            } else if let Some(p) = frame.downcast_ref::<Nv12Planes>() {
                p.clone()
            } else {
                return Err(DecodeError("unknown NV12 backing".into()));
            };
            let yuv = crate::avc444::upsample_main(&planes)
                .ok_or_else(|| DecodeError("NV12 picture of odd size".into()))?;
            Ok(crate::avc444::yuv444_to_bgra(&yuv))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPSs from this Mac's VideoToolbox HEVC encoder at 640×368 (header included): Main
    /// (8-bit 4:2:0) and Main 4:4:4 (RExt, 8-bit 4:4:4).
    const SPS_MAIN: [u8; 47] = [
        0x42, 0x01, 0x03, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0xb0, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00,
        0x3f, 0x00, 0x00, 0xa0, 0x05, 0x02, 0x01, 0x71, 0x62, 0x06, 0xee, 0x45, 0x23, 0x2e, 0x7e, 0x13, 0xd0,
        0xbe, 0xa1, 0xbd, 0x52, 0x9a, 0x80, 0x80, 0x80, 0x81, 0xfc, 0x20, 0x10, 0x40,
    ];
    const SPS_444: [u8; 46] = [
        0x42, 0x01, 0x03, 0x04, 0x08, 0x00, 0x00, 0x03, 0x00, 0xbe, 0x08, 0x00, 0x00, 0x03, 0x00, 0x00, 0x3f,
        0x00, 0x00, 0x90, 0x00, 0xa0, 0x40, 0x2e, 0x2c, 0x40, 0xdd, 0xc8, 0xa4, 0x65, 0xcf, 0xc2, 0x7a, 0x17,
        0xd4, 0x37, 0xaa, 0x53, 0x70, 0x10, 0x10, 0x10, 0x3f, 0x84, 0x02, 0x08,
    ];

    #[test]
    fn nal_types_read_the_hevc_header() {
        assert_eq!(nal_type(&[0x40, 0x01]), Some(nal::VPS));
        assert_eq!(nal_type(&[0x42, 0x01]), Some(nal::SPS));
        assert_eq!(nal_type(&[0x44, 0x01]), Some(nal::PPS));
        assert_eq!(nal_type(&[0x26, 0x01]), Some(19), "IDR_W_RADL");
        assert!(is_irap(19) && is_irap(21) && !is_irap(1));
        assert!(is_vcl(1) && is_vcl(19) && !is_vcl(nal::SPS));
        assert_eq!(
            nal_types(&[0, 0, 0, 1, 0x40, 1, 0, 0, 1, 0x42, 1, 0, 0, 0, 1, 0x26, 1, 9]),
            vec![32, 33, 19]
        );
    }

    #[test]
    fn sps_info_reads_chroma_and_bit_depth() {
        let main = sps_info(&SPS_MAIN).unwrap();
        assert_eq!(main, SpsInfo { chroma_format_idc: 1, bit_depth: 8 });
        assert_eq!(output_format(Some(main)), cv::NV12_FULL_RANGE);
        let full = sps_info(&SPS_444).unwrap();
        assert_eq!(full, SpsInfo { chroma_format_idc: 3, bit_depth: 8 });
        assert_eq!(output_format(Some(full)), cv::BGRA);
    }

    #[test]
    fn malformed_sps_is_none() {
        assert_eq!(sps_info(&[]), None);
        assert_eq!(sps_info(&[0x40, 0x01, 0xFF]), None, "a VPS");
        assert_eq!(sps_info(&[0x42, 0x01, 0x01]), None, "truncated");
        assert_eq!(sps_info(&[0x42, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn emulation_prevention_bytes_are_removed() {
        let mut b = Bits::new(&[0x00, 0x00, 0x03, 0x01]);
        assert_eq!(b.bits(24), Some(1));
    }

    #[test]
    fn realtime_config_spends_more_on_full_chroma() {
        let s = Size::new(1920, 1080);
        let main = HevcConfig::realtime(s, 60, HevcProfile::Main, Transfer::Sdr);
        let full = HevcConfig::realtime(s, 60, HevcProfile::Main444, Transfer::Sdr);
        assert!(full.average_bitrate > main.average_bitrate);
        assert!(main.max_keyframe_interval <= Duration::from_secs(4));
    }
}
