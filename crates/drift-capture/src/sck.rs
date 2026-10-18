//! ScreenCaptureKit: displays, one `SCStream` per display, and the Screen Recording permission
//! (humble object; decisions live in [`crate::frame`], [`crate::rect`], [`crate::display`]).

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use drift_core::Size;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGDisplayCopyDisplayMode, CGDisplayMode, CGMainDisplayID, CGPreflightScreenCaptureAccess,
};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetIOSurface, CVPixelBufferGetWidth,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamFrameInfoDirtyRects, SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType,
};

use crate::config::{CaptureConfig, ConfigError};
use crate::display::{DisplayInfo, select};
use crate::frame::{FrameMeta, FrameStatus, damage};
use crate::rect::Damage;

/// How long to wait for ScreenCaptureKit's completion handlers.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A capture failure.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The process lacks the Screen Recording permission.
    #[error("Drift Host needs the Screen Recording permission (System Settings › Privacy & Security)")]
    NoPermission,
    /// ScreenCaptureKit reported an error.
    #[error("ScreenCaptureKit: {0}")]
    Sck(String),
    /// A completion handler never ran.
    #[error("ScreenCaptureKit did not answer within {0:?}")]
    Timeout(Duration),
    /// No display matches the configuration.
    #[error("no display to capture")]
    NoDisplay,
    /// The configuration is invalid.
    #[error(transparent)]
    Config(#[from] ConfigError),
}

/// Whether this process may capture the screen (does not prompt).
pub fn has_permission() -> bool {
    CGPreflightScreenCaptureAccess()
}

fn error_text(error: *mut NSError) -> String {
    // SAFETY: ScreenCaptureKit passes a valid NSError or null for the duration of the call.
    unsafe { error.as_ref() }
        .map_or_else(|| "unknown error".to_owned(), |e| e.localizedDescription().to_string())
}

/// Wrapper to move a retained object across the completion handler's thread.
struct Sent<T>(T);
// SAFETY: the wrapped ScreenCaptureKit objects are thread-safe value objects (content lists,
// displays); they are only read after crossing the channel.
unsafe impl<T> Send for Sent<T> {}

fn shareable_content() -> Result<Retained<SCShareableContent>, CaptureError> {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        // SAFETY: a valid (autoreleased) content object or null; retained before it leaves.
        let result = match unsafe { Retained::retain(content) } {
            Some(c) => Ok(Sent(c)),
            None => Err(error_text(error)),
        };
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(result);
        }
    });
    // SAFETY: a class method taking a completion block that it copies.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    match rx.recv_timeout(TIMEOUT) {
        Ok(Ok(c)) => Ok(c.0),
        Ok(Err(e)) => Err(CaptureError::Sck(e)),
        Err(_) => Err(CaptureError::Timeout(TIMEOUT)),
    }
}

fn display_info(display: &SCDisplay) -> DisplayInfo {
    // SAFETY: plain getters on a valid SCDisplay.
    let (id, w, h, frame) =
        unsafe { (display.displayID(), display.width(), display.height(), display.frame()) };
    let points = Size::new(u32::try_from(w).unwrap_or(0), u32::try_from(h).unwrap_or(0));
    let mode = CGDisplayCopyDisplayMode(id);
    let pixels = mode.map_or(points, |m| {
        Size::new(
            u32::try_from(CGDisplayMode::pixel_width(Some(&m))).unwrap_or(points.width),
            u32::try_from(CGDisplayMode::pixel_height(Some(&m))).unwrap_or(points.height),
        )
    });
    DisplayInfo { id, origin: (frame.origin.x, frame.origin.y), points, pixels }
}

/// Display `id` from CoreGraphics alone (no ScreenCaptureKit, no permission needed): bounds in
/// global points and the current mode's pixel size.
pub fn cg_display(id: u32) -> DisplayInfo {
    let bounds = objc2_core_graphics::CGDisplayBounds(id);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let points = Size::new(bounds.size.width.max(0.0) as u32, bounds.size.height.max(0.0) as u32);
    let pixels = CGDisplayCopyDisplayMode(id).map_or(points, |m| {
        Size::new(
            u32::try_from(CGDisplayMode::pixel_width(Some(&m))).unwrap_or(points.width),
            u32::try_from(CGDisplayMode::pixel_height(Some(&m))).unwrap_or(points.height),
        )
    });
    DisplayInfo { id, origin: (bounds.origin.x, bounds.origin.y), points, pixels }
}

/// The main display, from CoreGraphics (see [`cg_display`]).
pub fn main_display() -> DisplayInfo {
    cg_display(CGMainDisplayID())
}

/// The displays ScreenCaptureKit can capture.
pub fn displays() -> Result<Vec<DisplayInfo>, CaptureError> {
    if !has_permission() {
        return Err(CaptureError::NoPermission);
    }
    let content = shareable_content()?;
    // SAFETY: a getter on a valid content object.
    let list = unsafe { content.displays() };
    Ok(list.iter().map(|d| display_info(&d)).collect())
}

/// One captured frame.
#[derive(Debug)]
pub struct CapturedFrame {
    /// The pixels (IOSurface-backed; hand to `drift_video::encode::PixelBuffer::from_cv`).
    pub buffer: CFRetained<CVPixelBuffer>,
    /// Size in pixels.
    pub size: Size<u32>,
    /// What changed.
    pub damage: Damage,
    /// The frame's status.
    pub status: FrameStatus,
}

impl CapturedFrame {
    /// Whether the pixels live in an IOSurface (zero-copy to VideoToolbox and Metal).
    pub fn is_iosurface_backed(&self) -> bool {
        CVPixelBufferGetIOSurface(Some(&self.buffer)).is_some()
    }
}

type FrameSink = Arc<dyn Fn(CapturedFrame) + Send + Sync>;

/// Instance variables of [`Output`].
struct OutputIvars {
    sink: FrameSink,
    max_dirty_rects: usize,
}

define_class!(
    /// The stream's sample handler.
    #[unsafe(super(NSObject))]
    #[name = "DriftCaptureOutput"]
    #[ivars = OutputIvars]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            if let Some(frame) = frame_from(sample, self.ivars().max_dirty_rects) {
                (self.ivars().sink)(frame);
            }
        }
    }
);

/// Reads `SCStreamFrameInfo` from the first sample attachment dictionary.
fn frame_meta(sample: &CMSampleBuffer) -> Option<FrameMeta> {
    // SAFETY: a valid sample buffer; `false` means "do not create" (read-only).
    let attachments = unsafe { sample.sample_attachments_array(false) }?;
    // SAFETY: CFArray and NSArray are toll-free bridged; the array holds CFDictionaries, which
    // are toll-free bridged to NSDictionary. The array outlives the borrow.
    let array: &NSArray<NSDictionary<NSString, AnyObject>> =
        unsafe { &*CFRetained::as_ptr(&attachments).as_ptr().cast() };
    let info = array.firstObject()?;
    // SAFETY: constant keys exported by ScreenCaptureKit.
    let (status_key, dirty_key) = unsafe { (SCStreamFrameInfoStatus, SCStreamFrameInfoDirtyRects) };
    let status = info
        .objectForKey(status_key)
        .and_then(|v| v.downcast::<NSNumber>().ok())
        .and_then(|n| FrameStatus::from_raw(n.integerValue() as i64))?;
    let mut dirty = Vec::new();
    if let Some(rects) = info.objectForKey(dirty_key).and_then(|v| v.downcast::<NSArray>().ok()) {
        for rect in rects.iter() {
            let Ok(rect) = rect.downcast::<NSDictionary>() else { continue };
            // SAFETY: a CGRect dictionary representation: NSString keys, NSNumber values.
            let rect: &NSDictionary<NSString, AnyObject> = unsafe { &*Retained::as_ptr(&rect).cast() };
            let get = |key: &str| {
                rect.objectForKey(&NSString::from_str(key))
                    .and_then(|v| v.downcast::<NSNumber>().ok())
                    .map(|n| n.doubleValue())
            };
            if let (Some(x), Some(y), Some(w), Some(h)) = (get("X"), get("Y"), get("Width"), get("Height")) {
                dirty.push((x, y, w, h));
            }
        }
    }
    // Dirty rects are in the output frame's pixel coordinates (verified in the HOST-3 run).
    Some(FrameMeta { status, scale: 1.0, dirty })
}

fn frame_from(sample: &CMSampleBuffer, max_dirty_rects: usize) -> Option<CapturedFrame> {
    let meta = frame_meta(sample)?;
    if !meta.status.has_pixels() {
        return None;
    }
    // SAFETY: a valid sample buffer; the image buffer is retained by the returned value.
    let image = unsafe { sample.image_buffer() }?;
    // SAFETY: a CVImageBuffer from a screen sample is a CVPixelBuffer (same CF type).
    let buffer: CFRetained<CVPixelBuffer> = unsafe { CFRetained::cast_unchecked(image) };
    let size = Size::new(
        u32::try_from(CVPixelBufferGetWidth(&buffer)).unwrap_or(0),
        u32::try_from(CVPixelBufferGetHeight(&buffer)).unwrap_or(0),
    );
    let damage = damage(&meta, size, max_dirty_rects);
    Some(CapturedFrame { buffer, size, damage, status: meta.status })
}

/// A running capture of one display. Stopped on drop.
#[derive(Debug)]
pub struct CaptureStream {
    stream: Retained<SCStream>,
    _output: Retained<Output>,
    _queue: DispatchRetained<DispatchQueue>,
    display: DisplayInfo,
    stopped: bool,
}

// SAFETY: SCStream is thread-safe (its methods dispatch internally); the output object only
// holds a `Send + Sync` sink.
unsafe impl Send for CaptureStream {}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Output").finish_non_exhaustive()
    }
}

/// A completion handler and the receiver it reports to.
type Completion = (RcBlock<dyn Fn(*mut NSError)>, mpsc::Receiver<Result<(), String>>);

fn completion() -> Completion {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let block = RcBlock::new(move |error: *mut NSError| {
        let result = if error.is_null() { Ok(()) } else { Err(error_text(error)) };
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(result);
        }
    });
    (block, rx)
}

fn wait(rx: &mpsc::Receiver<Result<(), String>>) -> Result<(), CaptureError> {
    match rx.recv_timeout(TIMEOUT) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(CaptureError::Sck(e)),
        Err(_) => Err(CaptureError::Timeout(TIMEOUT)),
    }
}

impl CaptureStream {
    /// Starts capturing the configured display at its native pixel size, without the pointer;
    /// `sink` receives every frame with new pixels on a private serial queue.
    pub fn start(
        config: CaptureConfig,
        sink: impl Fn(CapturedFrame) + Send + Sync + 'static,
    ) -> Result<Self, CaptureError> {
        config.validate()?;
        if !has_permission() {
            return Err(CaptureError::NoPermission);
        }
        let content = shareable_content()?;
        // SAFETY: a getter on a valid content object.
        let sc_displays = unsafe { content.displays() };
        let infos: Vec<DisplayInfo> = sc_displays.iter().map(|d| display_info(&d)).collect();
        let info =
            select(&infos, config.display, CGMainDisplayID()).cloned().ok_or(CaptureError::NoDisplay)?;
        // SAFETY: a plain getter.
        let sc_display = sc_displays.iter().find(|d| unsafe { d.displayID() } == info.id);
        let sc_display = sc_display.ok_or(CaptureError::NoDisplay)?;

        let excluded = NSArray::new();
        // SAFETY: designated initializer with a valid display and an empty window list.
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &sc_display,
                &excluded,
            )
        };
        // SAFETY: `new` on a configuration object, then plain setters.
        let sc_config = unsafe { SCStreamConfiguration::new() };
        let (value, timescale) = config.min_frame_interval();
        // SAFETY: plain property setters on a fresh configuration.
        unsafe {
            sc_config.setWidth(info.pixels.width as usize);
            sc_config.setHeight(info.pixels.height as usize);
            sc_config.setPixelFormat(config.format.fourcc());
            sc_config.setShowsCursor(false);
            sc_config.setQueueDepth(config.queue_depth as isize);
            sc_config.setMinimumFrameInterval(CMTime {
                value,
                timescale,
                flags: CMTimeFlags::Valid,
                epoch: 0,
            });
        }
        // SAFETY: designated initializer; no delegate.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &sc_config, None)
        };
        let output = Output::alloc()
            .set_ivars(OutputIvars { sink: Arc::new(sink), max_dirty_rects: config.max_dirty_rects });
        // SAFETY: NSObject's `init` on a freshly allocated subclass instance.
        let output: Retained<Output> = unsafe { msg_send![super(output), init] };
        let queue = DispatchQueue::new("com.hutsonlabs.drift.capture", None);
        // SAFETY: the stream retains the output and uses the queue we keep alive.
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*output),
                    SCStreamOutputType::Screen,
                    Some(&queue),
                )
                .map_err(|e| CaptureError::Sck(e.localizedDescription().to_string()))?;
        }
        let (block, rx) = completion();
        // SAFETY: the stream copies the completion block.
        unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };
        wait(&rx)?;
        Ok(Self { stream, _output: output, _queue: queue, display: info, stopped: false })
    }

    /// The display being captured.
    pub fn display(&self) -> &DisplayInfo {
        &self.display
    }

    /// Stops the stream and waits for ScreenCaptureKit to confirm.
    pub fn stop(mut self) -> Result<(), CaptureError> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<(), CaptureError> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        let (block, rx) = completion();
        // SAFETY: the stream copies the completion block.
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&block)) };
        wait(&rx)
    }
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}
