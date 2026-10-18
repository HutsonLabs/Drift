//! System audio for Drift Host's rdpsnd stream (task **HOST-11**, ADR
//! `HOST-11-clipboard-and-audio`).
//!
//! ScreenCaptureKit delivers 32-bit float samples, interleaved or one plane per channel;
//! [`to_s16_stereo`] turns them into the 16-bit interleaved stereo PCM that rdpsnd carries. The
//! macOS part, [`AudioCapture`], is a humble object around an audio-only `SCStream`.

/// How the channels of a float buffer are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `L R L R …`
    Interleaved,
    /// All of channel 0, then all of channel 1, …
    Planar,
}

fn s16(sample: f32) -> i16 {
    let scaled = (sample.clamp(-1.0, 1.0) * 32767.0).round();
    // In range after the clamp; NaN becomes 0.
    if scaled.is_nan() { 0 } else { scaled as i16 }
}

/// Converts `channels` channels of float samples to 16-bit little-endian interleaved stereo:
/// mono is copied to both sides, channels past the second are dropped, and a trailing partial
/// frame is ignored.
pub fn to_s16_stereo(samples: &[f32], channels: usize, layout: Layout) -> Vec<u8> {
    if channels == 0 {
        return Vec::new();
    }
    let frames = samples.len() / channels;
    let at = |frame: usize, channel: usize| match layout {
        Layout::Interleaved => samples[frame * channels + channel],
        Layout::Planar => samples[channel * frames + frame],
    };
    let mut out = Vec::with_capacity(frames * 4);
    for frame in 0..frames {
        let left = at(frame, 0);
        let right = if channels > 1 { at(frame, 1) } else { left };
        out.extend_from_slice(&s16(left).to_le_bytes());
        out.extend_from_slice(&s16(right).to_le_bytes());
    }
    out
}

#[cfg(target_os = "macos")]
pub use macos::{AudioCapture, AudioChunk};

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::Arc;

    use dispatch2::{DispatchQueue, DispatchRetained};
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
    use objc2_core_audio_types::kAudioFormatFlagIsNonInterleaved;
    use objc2_core_media::{CMAudioFormatDescriptionGetStreamBasicDescription, CMSampleBuffer};
    use objc2_foundation::{NSArray, NSObject, NSObjectProtocol};
    use objc2_screen_capture_kit::{
        SCContentFilter, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType,
    };

    use super::{Layout, to_s16_stereo};
    use crate::sck::{CaptureError, completion, has_permission, shareable_content, wait};

    /// 16-bit interleaved stereo PCM at [`AudioCapture::SAMPLE_RATE`].
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct AudioChunk {
        /// The samples.
        pub pcm: Vec<u8>,
    }

    type ChunkSink = Arc<dyn Fn(AudioChunk) + Send + Sync>;

    struct Ivars {
        sink: ChunkSink,
    }

    define_class!(
        /// The audio stream's sample handler.
        #[unsafe(super(NSObject))]
        #[name = "DriftAudioCaptureOutput"]
        #[ivars = Ivars]
        struct Output;

        unsafe impl NSObjectProtocol for Output {}

        unsafe impl SCStreamOutput for Output {
            #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
            fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
                if kind != SCStreamOutputType::Audio {
                    return;
                }
                if let Some(pcm) = pcm_from(sample) {
                    (self.ivars().sink)(AudioChunk { pcm });
                }
            }
        }
    );

    /// Reads a float sample buffer as 16-bit stereo PCM.
    fn pcm_from(sample: &CMSampleBuffer) -> Option<Vec<u8>> {
        // SAFETY: a valid audio sample buffer; both getters return retained objects or None.
        let (desc, block) = unsafe { (sample.format_description()?, sample.data_buffer()?) };
        // SAFETY: an audio format description; the pointer stays valid while `desc` lives.
        let asbd = unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&desc).as_ref() }?;
        if asbd.mBitsPerChannel != 32 {
            return None;
        }
        let channels = asbd.mChannelsPerFrame as usize;
        let layout = if asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0 {
            Layout::Planar
        } else {
            Layout::Interleaved
        };
        // SAFETY: a plain getter on a valid block buffer.
        let len = unsafe { block.data_length() };
        let mut floats = vec![0.0_f32; len / 4];
        let dest = NonNull::new(floats.as_mut_ptr().cast::<c_void>())?;
        // SAFETY: `floats` holds at least `len / 4 * 4` bytes; copying exactly that many.
        let status = unsafe { block.copy_data_bytes(0, floats.len() * 4, dest) };
        (status == 0).then(|| to_s16_stereo(&floats, channels, layout))
    }

    /// An audio-only ScreenCaptureKit stream of the system's output (Drift Host's own sounds
    /// excluded). Stopped on drop.
    pub struct AudioCapture {
        stream: Retained<SCStream>,
        _output: Retained<Output>,
        _queue: DispatchRetained<DispatchQueue>,
        stopped: bool,
    }

    impl std::fmt::Debug for AudioCapture {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("AudioCapture").finish_non_exhaustive()
        }
    }

    // SAFETY: SCStream is thread-safe; the output only holds a `Send + Sync` sink.
    unsafe impl Send for AudioCapture {}

    impl AudioCapture {
        /// The rate ScreenCaptureKit is asked to deliver.
        pub const SAMPLE_RATE: u32 = 48_000;

        /// Starts capturing system audio; `sink` gets 16-bit stereo PCM chunks on a private queue.
        pub fn start(sink: impl Fn(AudioChunk) + Send + Sync + 'static) -> Result<Self, CaptureError> {
            if !has_permission() {
                return Err(CaptureError::NoPermission);
            }
            let content = shareable_content()?;
            // SAFETY: a getter on a valid content object.
            let display = unsafe { content.displays() }.firstObject().ok_or(CaptureError::NoDisplay)?;
            let excluded = NSArray::new();
            // SAFETY: designated initializer with a valid display and an empty window list.
            let filter = unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &excluded,
                )
            };
            // SAFETY: `new`, then plain setters. A 2×2 picture keeps the unavoidable video side cheap.
            let config = unsafe { SCStreamConfiguration::new() };
            // SAFETY: plain property setters on a fresh configuration.
            unsafe {
                config.setCapturesAudio(true);
                config.setExcludesCurrentProcessAudio(true);
                config.setSampleRate(Self::SAMPLE_RATE as isize);
                config.setChannelCount(2);
                config.setWidth(2);
                config.setHeight(2);
            }
            // SAFETY: designated initializer; no delegate.
            let stream = unsafe {
                SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None)
            };
            let output = Output::alloc().set_ivars(Ivars { sink: Arc::new(sink) });
            // SAFETY: NSObject's `init` on a freshly allocated subclass instance.
            let output: Retained<Output> = unsafe { msg_send![super(output), init] };
            let queue = DispatchQueue::new("com.hutsonlabs.drift.capture-audio", None);
            // SAFETY: the stream retains the output and uses the queue we keep alive.
            unsafe {
                stream
                    .addStreamOutput_type_sampleHandlerQueue_error(
                        ProtocolObject::from_ref(&*output),
                        SCStreamOutputType::Audio,
                        Some(&queue),
                    )
                    .map_err(|e| CaptureError::Sck(e.localizedDescription().to_string()))?;
            }
            let (block, rx) = completion();
            // SAFETY: the stream copies the completion block.
            unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };
            wait(&rx)?;
            Ok(Self { stream, _output: output, _queue: queue, stopped: false })
        }

        fn stop_inner(&mut self) {
            if self.stopped {
                return;
            }
            self.stopped = true;
            let (block, rx) = completion();
            // SAFETY: the stream copies the completion block.
            unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&block)) };
            let _ = wait(&rx);
        }
    }

    impl Drop for AudioCapture {
        fn drop(&mut self) {
            self.stop_inner();
        }
    }
}
