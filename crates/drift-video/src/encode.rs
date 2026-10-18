//! H.264 encoder via `VTCompressionSession` (tasks **M8-2**, **HOST-4**).
//!
//! Two profiles share one encoder:
//!
//! * **Recording** ([`EncoderConfig::new`], plan M8-2): hardware encoder, High profile,
//!   real-time, no frame reordering, 8 Mbit/s average, a keyframe at least every 2 s.
//! * **Real-time** ([`EncoderConfig::realtime`], HOST-4, ADR `HOST-4-realtime-encoder`): what
//!   the Mac host streams with. Adds VideoToolbox's low-latency rate control (Constrained High,
//!   no B-frames, one frame in flight), a bit rate scaled to the picture ([`realtime_bitrate`]),
//!   and a keyframe at least every 4 s or on request ([`VtEncoder::request_keyframe`]).
//!
//! Both use a variable frame rate: each frame's presentation time comes from a
//! [`drift_core::Clock`] instant, relative to the first frame. A frame whose size differs from
//! the session's rebuilds the session, and the first frame of every session is a keyframe. Every
//! picture records when VideoToolbox finished it ([`EncodedFrame::completed_at`]) for the host's
//! latency statistics.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use drift_core::Size;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{
    CMFormatDescription, CMSampleBuffer, CMTime, CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
    kCMTimeInvalid, kCMVideoCodecType_H264,
};
use objc2_core_video::{
    CVPixelBuffer, kCVImageBufferColorPrimaries_ITU_R_709_2, kCVImageBufferTransferFunction_ITU_R_709_2,
    kCVImageBufferYCbCrMatrix_ITU_R_709_2,
};
use objc2_video_toolbox::{
    VTCompressionSession, VTEncodeInfoFlags, VTSessionCopyProperty, VTSessionSetProperty,
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ColorPrimaries, kVTCompressionPropertyKey_ExpectedFrameRate,
    kVTCompressionPropertyKey_MaxKeyFrameInterval, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
    kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime,
    kVTCompressionPropertyKey_TransferFunction,
    kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder, kVTCompressionPropertyKey_YCbCrMatrix,
    kVTEncodeFrameOptionKey_ForceKeyFrame, kVTProfileLevel_H264_ConstrainedHigh_AutoLevel,
    kVTProfileLevel_H264_High_AutoLevel, kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
    kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
};

use crate::annexb::{avcc_nals, avcc_to_annex_b, nal_type, nal_unit_type};
use crate::cv::{self, SharedPixelBuffer};
use crate::params::ParameterSets;

/// Errors from the encoder.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    /// A VideoToolbox / CoreMedia / CoreVideo call failed.
    #[error("{call} failed: OSStatus {status}")]
    Os {
        /// The failing call.
        call: &'static str,
        /// Its status code.
        status: i32,
    },
    /// Timestamps must strictly increase.
    #[error("non-monotonic timestamp: {pts:?} after {previous:?}")]
    NonMonotonic {
        /// The rejected presentation time.
        pts: Duration,
        /// The previous presentation time.
        previous: Duration,
    },
    /// The frame is not usable (zero size, wrong pixel format, …).
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    /// An encoded sample could not be read back.
    #[error("bad encoded sample: {0}")]
    BadSample(String),
}

/// Encoder settings. [`EncoderConfig::new`] gives the recording profile, [`EncoderConfig::realtime`]
/// the host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    /// Frame size in pixels.
    pub size: Size<u32>,
    /// Average bit rate in bits per second (8 Mbit/s).
    pub average_bitrate: u32,
    /// Maximum time between keyframes (2 s).
    pub max_keyframe_interval: Duration,
    /// Expected frame rate, a hint for rate control (60).
    pub expected_fps: u32,
    /// VideoToolbox's low-latency rate control (real-time profile).
    pub low_latency: bool,
}

/// Lowest and highest bit rate [`realtime_bitrate`] picks.
pub const REALTIME_BITRATE_RANGE: (u32, u32) = (2_000_000, 80_000_000);

/// The real-time profile's average bit rate: 1/20 bit per pixel per frame, so 4K at 60 fps gets
/// about 25 Mbit/s and 1080p60 about 6 Mbit/s, clamped to [`REALTIME_BITRATE_RANGE`]. A starting
/// point; quality hints (EXT-4) adjust it.
pub fn realtime_bitrate(size: Size<u32>, fps: u32) -> u32 {
    let bits = u64::from(size.width) * u64::from(size.height) * u64::from(fps) / 20;
    let (lo, hi) = REALTIME_BITRATE_RANGE;
    u32::try_from(bits).unwrap_or(u32::MAX).clamp(lo, hi)
}

impl EncoderConfig {
    /// The plan's configuration for a given frame size.
    pub fn new(size: Size<u32>) -> Self {
        Self {
            size,
            average_bitrate: 8_000_000,
            max_keyframe_interval: Duration::from_secs(2),
            expected_fps: 60,
            low_latency: false,
        }
    }

    /// The Mac host's real-time profile for `size` at `fps`.
    pub fn realtime(size: Size<u32>, fps: u32) -> Self {
        Self {
            size,
            average_bitrate: realtime_bitrate(size, fps),
            max_keyframe_interval: Duration::from_secs(4),
            expected_fps: fps.max(1),
            low_latency: true,
        }
    }

    /// Keyframe interval in frames at the expected rate (`MaxKeyFrameInterval`).
    pub fn max_keyframe_interval_frames(&self) -> u32 {
        let frames = u128::from(self.expected_fps) * self.max_keyframe_interval.as_millis() / 1000;
        u32::try_from(frames).unwrap_or(u32::MAX).max(1)
    }
}

/// Maps clock instants to presentation times (pure): the first frame is at zero, and times must
/// strictly increase.
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    origin: Option<Instant>,
    last: Option<Duration>,
}

impl Timeline {
    /// A timeline whose origin is the first instant it sees.
    pub fn new() -> Self {
        Self::default()
    }

    /// The presentation time of a frame captured at `at`.
    pub fn pts(&mut self, at: Instant) -> Result<Duration, EncodeError> {
        let origin = *self.origin.get_or_insert(at);
        let pts = at.saturating_duration_since(origin);
        if let Some(previous) = self.last
            && pts <= previous
        {
            return Err(EncodeError::NonMonotonic { pts, previous });
        }
        self.last = Some(pts);
        Ok(pts)
    }
}

/// Gaps between consecutive keyframe presentation times.
pub fn keyframe_gaps(keyframe_pts: &[Duration]) -> Vec<Duration> {
    keyframe_pts.windows(2).map(|w| w[1].saturating_sub(w[0])).collect()
}

/// One encoded picture: an AVCC `CMSampleBuffer` as produced by VideoToolbox (passed through to
/// the MP4 writer unchanged), plus its presentation time and keyframe flag.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    sample: SharedSample,
    avcc: Vec<u8>,
    pts: Duration,
    keyframe: bool,
    completed_at: Instant,
}

impl EncodedFrame {
    /// Presentation time relative to the first frame of the recording.
    pub fn pts(&self) -> Duration {
        self.pts
    }

    /// Whether this is a sync sample (IDR).
    pub fn is_keyframe(&self) -> bool {
        self.keyframe
    }

    /// When VideoToolbox handed the picture back (the compression callback ran).
    pub fn completed_at(&self) -> Instant {
        self.completed_at
    }

    /// The compressed sample (AVCC, 4-byte NAL lengths) for passthrough muxing.
    pub fn sample_buffer(&self) -> &CMSampleBuffer {
        &self.sample.0
    }

    /// The sample's NAL units in AVCC form (4-byte lengths).
    pub fn avcc(&self) -> &[u8] {
        &self.avcc
    }

    /// The SPS/PPS of the sample's format description.
    pub fn parameter_sets(&self) -> Result<ParameterSets, EncodeError> {
        // SAFETY: the sample buffer is valid; the returned description is retained.
        let desc = unsafe { self.sample.0.format_description() }
            .ok_or_else(|| EncodeError::BadSample("no format description".into()))?;
        let sps = parameter_set(&desc, 0)?;
        let pps = parameter_set(&desc, 1)?;
        Ok(ParameterSets { sps, pps })
    }

    /// The picture as an Annex-B access unit (AUD first; SPS/PPS included on keyframes), the
    /// same shape g-r-d sends, so it can be fed to [`crate::decode::VtDecoder`].
    pub fn to_annex_b(&self) -> Result<Vec<u8>, EncodeError> {
        let mut out = vec![0, 0, 0, 1, 0x09, 0xF0];
        if self.keyframe {
            let ps = self.parameter_sets()?;
            for nal in [&ps.sps, &ps.pps] {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(nal);
            }
        }
        let body = avcc_to_annex_b(&self.avcc, 4).map_err(|e| EncodeError::BadSample(e.to_string()))?;
        out.extend_from_slice(&body);
        Ok(out)
    }
}

fn parameter_set(desc: &CMFormatDescription, index: usize) -> Result<Vec<u8>, EncodeError> {
    let mut ptr: *const u8 = std::ptr::null();
    let mut len = 0usize;
    // SAFETY: `desc` is a valid H.264 format description; out-pointers are valid; the returned
    // pointer stays valid while `desc` is retained, and is copied immediately.
    let status = unsafe {
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            desc,
            index,
            &mut ptr,
            &mut len,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if status != 0 || ptr.is_null() {
        return Err(EncodeError::Os { call: "CMVideoFormatDescriptionGetH264ParameterSetAtIndex", status });
    }
    // SAFETY: CoreMedia returned `len` readable bytes at `ptr` (see above).
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec())
}

/// A retained `CMSampleBuffer` that may cross threads (encoder callback → caller → writer).
#[derive(Debug, Clone)]
struct SharedSample(CFRetained<CMSampleBuffer>);

// SAFETY: a CMSampleBuffer is an atomically reference-counted CF object; once VideoToolbox has
// emitted a compressed sample it is not mutated, and Drift only reads it.
unsafe impl Send for SharedSample {}
// SAFETY: as above.
unsafe impl Sync for SharedSample {}

/// Output of the compression callback: the sample and when it arrived, or the failing status.
type Outputs = Mutex<Vec<Result<(SharedSample, Instant), i32>>>;

struct Session {
    session: CFRetained<VTCompressionSession>,
    size: Size<u32>,
    low_latency: bool,
}

impl Session {
    fn finish(&self) -> Result<(), EncodeError> {
        // SAFETY: valid session; kCMTimeInvalid completes every pending frame.
        let status = unsafe { self.session.complete_frames(kCMTimeInvalid) };
        os("VTCompressionSessionCompleteFrames", status)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the session is valid; after invalidate no callback touches the outputs box,
        // which the owning `VtEncoder` frees only after its sessions are dropped.
        unsafe {
            self.session.complete_frames(kCMTimeInvalid);
            self.session.invalidate();
        }
    }
}

/// VideoToolbox H.264 encoder.
pub struct VtEncoder {
    config: EncoderConfig,
    timeline: Timeline,
    session: Option<Session>,
    builds: u64,
    force_keyframe: bool,
    // After `session` so sessions are invalidated before the callback target is freed.
    outputs: Box<Outputs>,
}

impl std::fmt::Debug for VtEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VtEncoder").field("config", &self.config).field("builds", &self.builds).finish()
    }
}

// SAFETY: a VTCompressionSession may be driven from any thread as long as calls are not
// concurrent (`&mut self`); callback results go through a Mutex.
unsafe impl Send for VtEncoder {}

impl VtEncoder {
    /// Creates the encoder and its first compression session.
    pub fn new(config: EncoderConfig) -> Result<Self, EncodeError> {
        let mut enc = Self {
            config,
            timeline: Timeline::new(),
            session: None,
            builds: 0,
            force_keyframe: true,
            outputs: Box::new(Mutex::new(Vec::new())),
        };
        enc.rebuild(config.size)?;
        Ok(enc)
    }

    /// Current configuration (its size follows the last frame).
    pub fn config(&self) -> EncoderConfig {
        self.config
    }

    /// Number of compression sessions built (1 + number of resizes).
    pub fn session_builds(&self) -> u64 {
        self.builds
    }

    /// Whether VideoToolbox reports a hardware encoder for the current session.
    pub fn is_hardware_accelerated(&self) -> bool {
        let Some(session) = &self.session else { return false };
        let mut value: *const CFType = std::ptr::null();
        // SAFETY: valid session and key; `value` receives a +1 retained CF object or stays null.
        let status = unsafe {
            VTSessionCopyProperty(
                &session.session,
                kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
                None,
                (&raw mut value).cast(),
            )
        };
        let Some(value) = NonNull::new(value.cast_mut()) else { return false };
        // SAFETY: Copy rule: we own the returned reference.
        let value: CFRetained<CFType> = unsafe { CFRetained::from_raw(value) };
        status == 0 && value.downcast_ref::<CFBoolean>().is_some_and(CFBoolean::as_bool)
    }

    /// Makes the next encoded frame a keyframe (a new client, a lost frame, a decoder reset).
    pub fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    /// Whether the current session runs VideoToolbox's low-latency rate control (the
    /// real-time profile asks for it; a VideoToolbox without it falls back to the normal one).
    pub fn is_low_latency(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.low_latency)
    }

    /// Encodes one frame captured at `at`. Returns the pictures that completed so far.
    pub fn encode(&mut self, frame: &PixelBuffer, at: Instant) -> Result<Vec<EncodedFrame>, EncodeError> {
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
        // SAFETY: CMTimeMake (CMTime::new) has no preconditions.
        let cm_pts = unsafe { CMTime::new(micros, 1_000_000) };
        // SAFETY: valid session and pixel buffer; kCMTimeInvalid duration (variable frame rate);
        // the frame-properties dictionary is valid for the call; no refcon, no info out-pointer.
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
    pub fn flush(&mut self) -> Result<Vec<EncodedFrame>, EncodeError> {
        if let Some(session) = &self.session {
            session.finish()?;
        }
        self.drain()
    }

    fn drain(&mut self) -> Result<Vec<EncodedFrame>, EncodeError> {
        let raw = std::mem::take(&mut *lock(&self.outputs));
        raw.into_iter()
            .map(|r| {
                let (sample, at) =
                    r.map_err(|status| EncodeError::Os { call: "VTCompressionOutputCallback", status })?;
                frame_from_sample(sample, at)
            })
            .collect()
    }

    fn rebuild(&mut self, size: Size<u32>) -> Result<(), EncodeError> {
        self.session = None;
        let config = EncoderConfig { size, ..self.config };
        self.session = Some(create_session(&config, &self.outputs)?);
        self.config = config;
        self.builds += 1;
        self.force_keyframe = true;
        Ok(())
    }
}

fn lock(outputs: &Outputs) -> std::sync::MutexGuard<'_, Vec<Result<(SharedSample, Instant), i32>>> {
    outputs.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn os(call: &'static str, status: i32) -> Result<(), EncodeError> {
    if status == 0 { Ok(()) } else { Err(EncodeError::Os { call, status }) }
}

fn force_keyframe_properties() -> CFRetained<CFDictionary<CFString, CFType>> {
    // SAFETY: framework constant key.
    let key: [&CFString; 1] = [unsafe { kVTEncodeFrameOptionKey_ForceKeyFrame }];
    let value: [&CFType; 1] = [CFBoolean::new(true).as_ref()];
    CFDictionary::from_slices(&key, &value)
}

fn frame_from_sample(sample: SharedSample, completed_at: Instant) -> Result<EncodedFrame, EncodeError> {
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
    let mut avcc = vec![0u8; len];
    if let Some(dest) = NonNull::new(avcc.as_mut_ptr())
        && len > 0
    {
        // SAFETY: `avcc` has exactly `len` writable bytes; the block holds `len` bytes.
        let status = unsafe { block.copy_data_bytes(0, len, dest.cast()) };
        os("CMBlockBufferCopyDataBytes", status)?;
    }
    let keyframe = avcc_nals(&avcc, 4)
        .map_err(|e| EncodeError::BadSample(e.to_string()))?
        .iter()
        .any(|n| nal_unit_type(n) == Some(nal_type::IDR));
    Ok(EncodedFrame { sample, avcc, pts: Duration::from_secs_f64(seconds), keyframe, completed_at })
}

/// VideoToolbox compression callback: keeps the (retained) sample or the error status.
unsafe extern "C-unwind" fn output_callback(
    refcon: *mut c_void,
    _source_frame_refcon: *mut c_void,
    status: i32,
    _info: VTEncodeInfoFlags,
    sample: *mut CMSampleBuffer,
) {
    // SAFETY: `refcon` is the `Box<Outputs>` owned by the `VtEncoder`, which outlives its sessions.
    let outputs = unsafe { &*refcon.cast::<Outputs>() };
    let result = match NonNull::new(sample) {
        Some(sample) if status == 0 => {
            // SAFETY: valid sample for the callback's duration; retained to keep it.
            let sample = unsafe { CFRetained::retain(sample) };
            Ok((SharedSample(sample), Instant::now()))
        }
        // A dropped frame (status 0, no sample) is not an error: nothing to emit.
        None if status == 0 => return,
        _ => Err(status),
    };
    lock(outputs).push(result);
}

fn create_session(config: &EncoderConfig, outputs: &Outputs) -> Result<Session, EncodeError> {
    if config.low_latency {
        match create_session_with(config, outputs, true) {
            Ok(session) => return Ok(session),
            Err(e) => {
                tracing::warn!(error = %e, "low-latency rate control unavailable; using the normal one")
            }
        }
    }
    create_session_with(config, outputs, false)
}

fn create_session_with(
    config: &EncoderConfig,
    outputs: &Outputs,
    low_latency: bool,
) -> Result<Session, EncodeError> {
    let width = i32::try_from(config.size.width).map_err(|_| EncodeError::InvalidFrame("width".into()))?;
    let height = i32::try_from(config.size.height).map_err(|_| EncodeError::InvalidFrame("height".into()))?;
    let yes = CFBoolean::new(true);
    // SAFETY: framework constant keys.
    let (hw_key, ll_key) = unsafe {
        (
            kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
            kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
        )
    };
    let on: &CFType = yes.as_ref();
    let spec = if low_latency {
        let keys: [&CFString; 2] = [hw_key, ll_key];
        CFDictionary::from_slices(&keys, &[on, on])
    } else {
        let keys: [&CFString; 1] = [hw_key];
        CFDictionary::from_slices(&keys, &[on])
    };
    let mut out: *mut VTCompressionSession = std::ptr::null_mut();
    // SAFETY: valid dimensions, codec and dictionaries; the refcon outlives the session (see
    // `VtEncoder`); `out` is a valid out-pointer.
    let status = unsafe {
        VTCompressionSession::create(
            None,
            width,
            height,
            kCMVideoCodecType_H264,
            Some(spec.as_opaque()),
            None,
            None,
            Some(output_callback),
            std::ptr::from_ref(outputs).cast_mut().cast(),
            NonNull::from(&mut out),
        )
    };
    os("VTCompressionSessionCreate", status)?;
    let out = NonNull::new(out).ok_or(EncodeError::Os { call: "VTCompressionSessionCreate", status: -1 })?;
    // SAFETY: Create rule: +1 retained session.
    let session = Session { session: unsafe { CFRetained::from_raw(out) }, size: config.size, low_latency };

    let bitrate = CFNumber::new_i32(i32::try_from(config.average_bitrate).unwrap_or(i32::MAX));
    let gop_seconds = CFNumber::new_f64(config.max_keyframe_interval.as_secs_f64());
    let gop_frames =
        CFNumber::new_i32(i32::try_from(config.max_keyframe_interval_frames()).unwrap_or(i32::MAX));
    let fps = CFNumber::new_i32(i32::try_from(config.expected_fps).unwrap_or(60));
    let no = CFBoolean::new(false);
    // Low-latency rate control supports Constrained Baseline and Constrained High only.
    // SAFETY: framework constant profile names.
    let profile = unsafe {
        if low_latency {
            kVTProfileLevel_H264_ConstrainedHigh_AutoLevel
        } else {
            kVTProfileLevel_H264_High_AutoLevel
        }
    };
    // SAFETY: framework constant keys and values.
    let properties: [(&CFString, &CFType); 10] = unsafe {
        [
            (kVTCompressionPropertyKey_RealTime, yes.as_ref()),
            (kVTCompressionPropertyKey_ProfileLevel, profile.as_ref()),
            (kVTCompressionPropertyKey_AllowFrameReordering, no.as_ref()),
            (kVTCompressionPropertyKey_AverageBitRate, bitrate.as_ref()),
            (kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, gop_seconds.as_ref()),
            (kVTCompressionPropertyKey_MaxKeyFrameInterval, gop_frames.as_ref()),
            (kVTCompressionPropertyKey_ExpectedFrameRate, fps.as_ref()),
            (kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2.as_ref()),
            (kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2.as_ref()),
            (kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2.as_ref()),
        ]
    };
    for (key, value) in properties {
        // SAFETY: valid session, key and value.
        let status = unsafe { VTSessionSetProperty(&session.session, key, Some(value)) };
        if status != 0 {
            tracing::warn!(status, key = %key, "VTSessionSetProperty failed");
            os("VTSessionSetProperty", status)?;
        }
    }
    // SAFETY: valid session.
    let status = unsafe { session.session.prepare_to_encode_frames() };
    os("VTCompressionSessionPrepareToEncodeFrames", status)?;
    Ok(session)
}

/// An IOSurface-backed `CVPixelBuffer` to encode (BGRA from the compositor, or NV12).
#[derive(Debug, Clone)]
pub struct PixelBuffer {
    buffer: SharedPixelBuffer,
}

impl PixelBuffer {
    /// A new IOSurface-backed BGRA buffer.
    pub fn new_bgra(size: Size<u32>) -> Result<Self, EncodeError> {
        Self::new(size, cv::BGRA)
    }

    /// A new IOSurface-backed NV12 full-range buffer.
    pub fn new_nv12(size: Size<u32>) -> Result<Self, EncodeError> {
        Self::new(size, cv::NV12_FULL_RANGE)
    }

    fn new(size: Size<u32>, format: u32) -> Result<Self, EncodeError> {
        let buffer = cv::create_pixel_buffer(size, format)
            .map_err(|status| EncodeError::Os { call: "CVPixelBufferCreate", status })?;
        Ok(Self { buffer: SharedPixelBuffer(buffer) })
    }

    /// Wraps an existing pixel buffer (e.g. from the compositor's `CVPixelBufferPool`, M8-1).
    pub fn from_cv(buffer: CFRetained<CVPixelBuffer>) -> Self {
        Self { buffer: SharedPixelBuffer(buffer) }
    }

    /// Size in pixels.
    pub fn size(&self) -> Size<u32> {
        cv::size_of(&self.buffer.0)
    }

    /// Fills a BGRA buffer: `f(x, y)` returns `[b, g, r, a]`.
    pub fn fill_bgra(&self, f: impl Fn(u32, u32) -> [u8; 4]) -> Result<(), EncodeError> {
        if cv::format_of(&self.buffer.0) != cv::BGRA {
            return Err(EncodeError::InvalidFrame("not a BGRA buffer".into()));
        }
        cv::fill_bgra(&self.buffer.0, f).map_err(EncodeError::InvalidFrame)
    }

    /// Writes NV12 planes into an NV12 buffer.
    pub fn write_nv12(&self, planes: &drift_core::Nv12Planes) -> Result<(), EncodeError> {
        cv::write_nv12(&self.buffer.0, planes).map_err(EncodeError::InvalidFrame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_matches_plan() {
        let c = EncoderConfig::new(Size::new(1280, 800));
        assert_eq!(c.average_bitrate, 8_000_000);
        assert_eq!(c.max_keyframe_interval, Duration::from_secs(2));
        assert_eq!(c.expected_fps, 60);
        assert_eq!(c.size, Size::new(1280, 800));
        assert_eq!(c.max_keyframe_interval_frames(), 120);
        assert!(!c.low_latency);
    }

    #[test]
    fn realtime_profile() {
        let c = EncoderConfig::realtime(Size::new(1920, 1080), 60);
        assert!(c.low_latency);
        assert_eq!(c.average_bitrate, 6_220_800);
        assert_eq!(c.max_keyframe_interval_frames(), 240);
        assert_eq!(EncoderConfig::realtime(Size::new(16, 16), 0).expected_fps, 1);
    }

    #[test]
    fn timeline_starts_at_zero_and_is_variable_rate() {
        let t0 = Instant::now();
        let mut tl = Timeline::new();
        assert_eq!(tl.pts(t0 + Duration::from_millis(500)).unwrap(), Duration::ZERO);
        assert_eq!(tl.pts(t0 + Duration::from_millis(516)).unwrap(), Duration::from_millis(16));
        assert_eq!(tl.pts(t0 + Duration::from_millis(900)).unwrap(), Duration::from_millis(400));
    }

    #[test]
    fn timeline_rejects_non_increasing_times() {
        let t0 = Instant::now();
        let mut tl = Timeline::new();
        tl.pts(t0).unwrap();
        tl.pts(t0 + Duration::from_millis(10)).unwrap();
        assert_eq!(
            tl.pts(t0 + Duration::from_millis(10)),
            Err(EncodeError::NonMonotonic {
                pts: Duration::from_millis(10),
                previous: Duration::from_millis(10)
            })
        );
        // an instant before the origin saturates to zero, which is not after the last pts
        assert!(tl.pts(t0 - Duration::from_millis(1)).is_err());
        assert_eq!(tl.pts(t0 + Duration::from_millis(11)).unwrap(), Duration::from_millis(11));
    }

    #[test]
    fn gaps() {
        let ms = Duration::from_millis;
        assert_eq!(keyframe_gaps(&[ms(0), ms(2000), ms(3500)]), vec![ms(2000), ms(1500)]);
        assert!(keyframe_gaps(&[ms(5)]).is_empty());
    }
}
