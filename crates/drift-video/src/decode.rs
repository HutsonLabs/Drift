//! AVC420 decode via `VTDecompressionSession`. Owned by task **M1-3**.
//!
//! [`VtDecoder`] implements [`drift_core::video::H264Decoder`]: it takes one Annex-B access unit
//! (the bitstream after the `RFX_AVC420_METABLOCK`, see [`crate::metablock`]), converts it to
//! AVCC ([`crate::annexb`]), (re)builds the `CMVideoFormatDescription` + session when the SPS/PPS
//! change ([`crate::params`]), and decodes synchronously into an IOSurface-backed
//! `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange` `CVPixelBuffer` ([`DecodedPicture`]).
//!
//! Low latency: the session runs with `kVTDecompressionPropertyKey_RealTime` and every frame is
//! decoded synchronously (no `EnableAsynchronousDecompression`, no temporal processing), so the
//! picture is available when `decode` returns and the frame ack can follow the present
//! immediately (plan §1.4). VideoToolbox has no separate decoder "low latency" key; see
//! `docs/adr/M1-3-video-decode.md`.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Mutex;

use drift_core::video::{DecodeError, H264Decoder, Nv12Frame, Nv12Source};
use drift_core::{Nv12Planes, Size};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString, CFType};
use objc2_core_media::{
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMTime,
    CMVideoFormatDescriptionCreateFromH264ParameterSets, kCMBlockBufferAssureMemoryNowFlag,
};
use objc2_core_video::{CVImageBuffer, CVPixelBuffer};
use objc2_video_toolbox::{
    VTDecodeFrameFlags, VTDecodeInfoFlags, VTDecompressionOutputCallbackRecord, VTDecompressionSession,
    VTSessionSetProperty, kVTDecompressionPropertyKey_RealTime,
    kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder,
};

use crate::annexb::AccessUnit;
use crate::cv::{self, SharedPixelBuffer};
use crate::params::{ParamDecision, ParameterSetTracker, ParameterSets};

/// `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange` (`'420f'`): the decoder's output format.
pub const OUTPUT_PIXEL_FORMAT: u32 = cv::NV12_FULL_RANGE;

/// A decoded picture: an IOSurface-backed NV12 full-range `CVPixelBuffer`.
///
/// `drift-render` downcasts an [`Nv12Frame`] to this type and imports [`Self::pixel_buffer`]
/// through `CVMetalTextureCache` (zero copy).
#[derive(Debug, Clone)]
pub struct DecodedPicture {
    pub(crate) buffer: SharedPixelBuffer,
    pub(crate) size: Size<u32>,
}

impl DecodedPicture {
    /// The decoded `CVPixelBuffer`.
    pub fn pixel_buffer(&self) -> &CVPixelBuffer {
        &self.buffer.0
    }

    /// The picture's `CVPixelBuffer` pixel format (always [`OUTPUT_PIXEL_FORMAT`]).
    pub fn pixel_format(&self) -> u32 {
        cv::format_of(&self.buffer.0)
    }

    /// Whether the pixel buffer is backed by an IOSurface (required for zero-copy Metal import).
    pub fn is_iosurface_backed(&self) -> bool {
        cv::is_iosurface_backed(&self.buffer.0)
    }

    /// Copies the planes to CPU memory (tests, goldens, recording fallbacks).
    pub fn to_planes(&self) -> Result<Nv12Planes, DecodeError> {
        cv::read_nv12(&self.buffer.0).map_err(DecodeError)
    }
}

impl Nv12Source for DecodedPicture {
    fn size(&self) -> Size<u32> {
        self.size
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Where the output callback leaves the decoded picture (or the error status).
pub(crate) type OutputSlot = Mutex<Option<Result<SharedPixelBuffer, i32>>>;

/// One decompression session bound to one format description.
pub(crate) struct Session {
    pub(crate) session: CFRetained<VTDecompressionSession>,
    pub(crate) format: CFRetained<CMFormatDescription>,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the session is valid; invalidating it guarantees no further callbacks touch the
        // output slot before the slot (owned by `VtDecoder`) can be freed.
        unsafe {
            self.session.wait_for_asynchronous_frames();
            self.session.invalidate();
        }
    }
}

/// VideoToolbox H.264 decoder for AVC420 surfaces.
pub struct VtDecoder {
    tracker: ParameterSetTracker,
    session: Option<Session>,
    builds: u64,
    // Boxed so its address stays stable for the C callback's refcon. Declared after `session` so
    // the session is invalidated (Drop) before the slot is freed.
    slot: Box<OutputSlot>,
}

impl std::fmt::Debug for VtDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VtDecoder")
            .field("active", &self.tracker.active().is_some())
            .field("session", &self.session.is_some())
            .field("builds", &self.builds)
            .finish()
    }
}

// SAFETY: a VTDecompressionSession may be used from any thread as long as calls are not
// concurrent; `VtDecoder` is only driven through `&mut self`. The output slot is a Mutex.
unsafe impl Send for VtDecoder {}

impl Default for VtDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl VtDecoder {
    /// Creates a decoder; the session is built lazily from the first SPS/PPS.
    pub fn new() -> Self {
        Self {
            tracker: ParameterSetTracker::new(),
            session: None,
            builds: 0,
            slot: Box::new(Mutex::new(None)),
        }
    }

    /// How many decompression sessions have been built so far (one per SPS/PPS change).
    pub fn session_builds(&self) -> u64 {
        self.builds
    }

    /// Decodes one Annex-B access unit.
    ///
    /// Returns `Ok(None)` when the access unit carries no picture or cannot be decoded yet
    /// (no SPS/PPS seen). Malformed input yields `Err`, never a panic.
    pub fn decode_picture(&mut self, annex_b: &[u8]) -> Result<Option<DecodedPicture>, DecodeError> {
        let au = AccessUnit::from_annex_b(annex_b).map_err(|e| DecodeError(e.to_string()))?;
        match self.tracker.observe(&au) {
            ParamDecision::NotReady => return Ok(None),
            ParamDecision::Keep if self.session.is_some() => {}
            ParamDecision::Keep => {
                // The previous rebuild failed; retry with the active parameter sets.
                let Some(ps) = self.tracker.active().cloned() else { return Ok(None) };
                self.rebuild(&ps)?;
            }
            ParamDecision::Rebuild(ps) => self.rebuild(&ps)?,
        }
        if !au.has_picture() {
            return Ok(None);
        }
        let Some(session) = &self.session else { return Ok(None) };
        let sample = make_sample(&au.avcc, &session.format)?;
        *lock(&self.slot) = None;
        let mut info = VTDecodeInfoFlags::empty();
        // SAFETY: session and sample are valid; flags request synchronous decode, so the output
        // callback (which writes `self.slot`) runs before this returns; `info` is a valid out-pointer.
        let status = unsafe {
            session.session.decode_frame(
                &sample,
                VTDecodeFrameFlags::empty(),
                std::ptr::null_mut(),
                &mut info,
            )
        };
        if status != 0 {
            return Err(DecodeError(format!("VTDecompressionSessionDecodeFrame: OSStatus {status}")));
        }
        // SAFETY: the session is valid; waits for any frame VideoToolbox chose to emit asynchronously.
        unsafe { session.session.wait_for_asynchronous_frames() };
        match lock(&self.slot).take() {
            Some(Ok(buffer)) => {
                let size = cv::size_of(&buffer.0);
                Ok(Some(DecodedPicture { buffer, size }))
            }
            Some(Err(status)) => Err(DecodeError(format!("VideoToolbox decode callback: OSStatus {status}"))),
            None if info.contains(VTDecodeInfoFlags::FrameDropped) => Ok(None),
            None => Err(DecodeError("VideoToolbox produced no picture".into())),
        }
    }

    fn rebuild(&mut self, ps: &ParameterSets) -> Result<(), DecodeError> {
        self.session = None;
        match build_session(ps, &self.slot) {
            Ok(session) => {
                self.session = Some(session);
                self.builds += 1;
                Ok(())
            }
            Err(e) => {
                // Forget the bad parameter sets so the next IDR retries cleanly.
                self.tracker.reset();
                Err(e)
            }
        }
    }
}

impl H264Decoder for VtDecoder {
    fn decode(&mut self, annex_b: &[u8]) -> Result<Option<Nv12Frame>, DecodeError> {
        Ok(self.decode_picture(annex_b)?.map(Nv12Frame::new))
    }

    fn reset(&mut self) {
        self.session = None;
        self.tracker.reset();
        *lock(&self.slot) = None;
    }
}

pub(crate) fn lock(slot: &OutputSlot) -> std::sync::MutexGuard<'_, Option<Result<SharedPixelBuffer, i32>>> {
    slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// VideoToolbox output callback: stores the decoded image (retained) or the error status.
pub(crate) unsafe extern "C-unwind" fn output_callback(
    refcon: *mut c_void,
    _source_frame_refcon: *mut c_void,
    status: i32,
    _info: VTDecodeInfoFlags,
    image: *mut CVImageBuffer,
    _pts: CMTime,
    _duration: CMTime,
) {
    // SAFETY: `refcon` is the `Box<OutputSlot>` owned by the `VtDecoder`, which outlives the
    // session (the session is invalidated before the box is dropped).
    let slot = unsafe { &*refcon.cast::<OutputSlot>() };
    let result = match NonNull::new(image) {
        // SAFETY: `image` is a valid CVImageBuffer for the callback's duration; retaining it
        // keeps it alive afterwards.
        Some(image) if status == 0 => Ok(SharedPixelBuffer(unsafe { CFRetained::retain(image) })),
        _ => Err(if status == 0 { -1 } else { status }),
    };
    *lock(slot) = Some(result);
}

fn build_session(ps: &ParameterSets, slot: &OutputSlot) -> Result<Session, DecodeError> {
    let format = format_description(ps)?;
    // SAFETY: the decoder-specification key is an immutable framework CFString constant.
    let spec_key: [&CFString; 1] =
        [unsafe { kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder }];
    let spec_val: [&CFType; 1] = [CFBoolean::new(true).as_ref()];
    let spec = CFDictionary::from_slices(&spec_key, &spec_val);
    let attrs = cv::pixel_buffer_attributes(OUTPUT_PIXEL_FORMAT);
    let record = VTDecompressionOutputCallbackRecord {
        decompressionOutputCallback: Some(output_callback),
        decompressionOutputRefCon: std::ptr::from_ref(slot).cast_mut().cast(),
    };
    let mut out: *mut VTDecompressionSession = std::ptr::null_mut();
    // SAFETY: all CF arguments are valid for the call; `record` is copied by VideoToolbox; the
    // refcon outlives the session (see `VtDecoder`); `out` is a valid out-pointer.
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
        return Err(DecodeError(format!("VTDecompressionSessionCreate: OSStatus {status}")));
    }
    let out =
        NonNull::new(out).ok_or_else(|| DecodeError("VTDecompressionSessionCreate returned null".into()))?;
    // SAFETY: VTDecompressionSessionCreate returns a +1 retained session (Create rule).
    let session = unsafe { CFRetained::from_raw(out) };
    // SAFETY: valid session, framework key constant and CFBoolean value.
    let status = unsafe {
        VTSessionSetProperty(
            &session,
            kVTDecompressionPropertyKey_RealTime,
            Some(CFBoolean::new(true).as_ref()),
        )
    };
    if status != 0 {
        tracing::debug!(status, "kVTDecompressionPropertyKey_RealTime not supported");
    }
    Ok(Session { session, format })
}

/// Builds an H.264 `CMVideoFormatDescription` (4-byte NAL lengths) from one SPS/PPS pair.
///
/// The SPS is first rewritten to signal BT.709 full range ([`crate::sps`]): g-r-d's SPS omits
/// the colour description although its samples are full range (plan §1.4); untagged,
/// VideoToolbox would expand them as video range into the `420f` output.
pub(crate) fn format_description(ps: &ParameterSets) -> Result<CFRetained<CMFormatDescription>, DecodeError> {
    let sps = crate::sps::with_bt709_full_range(&ps.sps).map_err(|e| DecodeError(format!("SPS: {e}")))?;
    if ps.pps.is_empty() {
        return Err(DecodeError("empty PPS".into()));
    }
    let pointers = [NonNull::from(&sps[0]), NonNull::from(&ps.pps[0])];
    let sizes = [sps.len(), ps.pps.len()];
    let mut out: *const CMFormatDescription = std::ptr::null();
    // SAFETY: two valid parameter-set pointers with matching sizes (live for the call); `out` is
    // a valid out-pointer. CoreMedia copies the bytes.
    let status = unsafe {
        CMVideoFormatDescriptionCreateFromH264ParameterSets(
            None,
            2,
            NonNull::from(&pointers).cast(),
            NonNull::from(&sizes).cast(),
            4,
            NonNull::from(&mut out),
        )
    };
    if status != 0 {
        return Err(DecodeError(format!(
            "CMVideoFormatDescriptionCreateFromH264ParameterSets: OSStatus {status}"
        )));
    }
    let out = NonNull::new(out.cast_mut()).ok_or_else(|| DecodeError("null format description".into()))?;
    // SAFETY: the Create function returned a +1 retained format description.
    Ok(unsafe { CFRetained::from_raw(out) })
}

/// Wraps an AVCC sample in a `CMSampleBuffer` (data copied into a CoreMedia-owned block).
pub(crate) fn make_sample(
    avcc: &[u8],
    format: &CMFormatDescription,
) -> Result<CFRetained<CMSampleBuffer>, DecodeError> {
    let mut block: *mut CMBlockBuffer = std::ptr::null_mut();
    // SAFETY: a null memory block with the default allocator makes CoreMedia allocate `len`
    // bytes; `block` is a valid out-pointer.
    let status = unsafe {
        CMBlockBuffer::create_with_memory_block(
            None,
            std::ptr::null_mut(),
            avcc.len(),
            None,
            std::ptr::null(),
            0,
            avcc.len(),
            kCMBlockBufferAssureMemoryNowFlag,
            NonNull::from(&mut block),
        )
    };
    if status != 0 {
        return Err(DecodeError(format!("CMBlockBufferCreateWithMemoryBlock: OSStatus {status}")));
    }
    let block = NonNull::new(block).ok_or_else(|| DecodeError("null block buffer".into()))?;
    // SAFETY: +1 retained block buffer from a Create function.
    let block = unsafe { CFRetained::from_raw(block) };
    let src = NonNull::new(avcc.as_ptr().cast_mut()).ok_or_else(|| DecodeError("empty sample".into()))?;
    // SAFETY: the block owns exactly `avcc.len()` bytes; `src` points to that many readable bytes.
    let status = unsafe { CMBlockBuffer::replace_data_bytes(src.cast(), &block, 0, avcc.len()) };
    if status != 0 {
        return Err(DecodeError(format!("CMBlockBufferReplaceDataBytes: OSStatus {status}")));
    }
    let sizes = [avcc.len()];
    let mut sample: *mut CMSampleBuffer = std::ptr::null_mut();
    // SAFETY: valid block and format description, one sample with one size entry and no timing;
    // `sample` is a valid out-pointer.
    let status = unsafe {
        CMSampleBuffer::create_ready(
            None,
            Some(&block),
            Some(format),
            1,
            0,
            std::ptr::null(),
            1,
            sizes.as_ptr(),
            NonNull::from(&mut sample),
        )
    };
    if status != 0 {
        return Err(DecodeError(format!("CMSampleBufferCreateReady: OSStatus {status}")));
    }
    let sample = NonNull::new(sample).ok_or_else(|| DecodeError("null sample buffer".into()))?;
    // SAFETY: +1 retained sample buffer from a Create function.
    Ok(unsafe { CFRetained::from_raw(sample) })
}
