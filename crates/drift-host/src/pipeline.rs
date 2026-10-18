//! The live picture: raw frames → the real-time encoder → AVC420 frames for EGFX (ADR
//! `HOST-pipeline`).
//!
//! A [`RawFrames`] source (the screen through `drift-capture`, or [`MovingSquare`] in tests)
//! hands over the newest frame with what changed. [`LiveEncoder`] encodes it with HOST-4's
//! real-time profile and pairs each access unit with its AVC420 regions, which tell the client
//! which parts of the decoded picture to show. Frames are only taken while the client keeps up:
//! under backpressure the encoder waits and the capture mailbox merges what changed meanwhile, so
//! no picture is encoded and then dropped (a dropped P-frame would break the reference chain).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use drift_capture::rect::Damage;
use drift_core::video::{HevcProfile, Transfer};
use drift_core::{Nv12Planes, Size};
use drift_video::avc444::Avc444Encoder;
use drift_video::encode::{EncoderConfig, PixelBuffer, VtEncoder, realtime_bitrate};
use drift_video::hevc::{HevcConfig, HevcEncoder};
use ironrdp_egfx::pdu::Avc420Region;

use crate::frames::FrameError;

/// The quantisation parameter written into each region (the encoder chooses the real one).
pub const REGION_QP: u8 = 22;

/// The AVC420 regions for `damage` in a `size` picture, clipped to it.
pub fn regions_for(damage: &Damage, size: Size<u32>) -> Vec<Avc420Region> {
    let clip = |v: u32, max: u32| u16::try_from(v.min(max)).unwrap_or(u16::MAX);
    let region = |left: u32, top: u32, right: u32, bottom: u32| Avc420Region {
        left: clip(left, size.width),
        top: clip(top, size.height),
        right: clip(right, size.width),
        bottom: clip(bottom, size.height),
        quantization_parameter: REGION_QP,
        quality: 100,
    };
    match damage {
        Damage::None => Vec::new(),
        Damage::Full => vec![region(0, 0, size.width, size.height)],
        Damage::Rects(rects) => rects
            .iter()
            .filter(|r| r.x < size.width && r.y < size.height && r.width > 0 && r.height > 0)
            .map(|r| region(r.x, r.y, r.right(), r.bottom()))
            .collect(),
    }
}

/// What the pump does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing to send, or the client is behind: wait.
    Wait,
    /// Encode the newest frame (as a keyframe when one is due).
    Encode {
        /// Force an IDR picture.
        keyframe: bool,
    },
}

/// Whether to encode: never under backpressure, otherwise when something changed or a keyframe
/// is due (a new client, a resize, or a frame the server could not queue).
pub fn decide(backpressure: bool, has_damage: bool, keyframe_due: bool) -> Step {
    if backpressure {
        Step::Wait
    } else if keyframe_due {
        Step::Encode { keyframe: true }
    } else if has_damage {
        Step::Encode { keyframe: false }
    } else {
        Step::Wait
    }
}

/// Keeps a frame-rate cap (EXT-4's lower quality levels): a frame is taken only once the
/// interval since the last one has passed; until then the capture mailbox merges what changed.
#[derive(Debug, Clone, Copy)]
pub struct Pacer {
    interval: Duration,
    last: Option<Instant>,
}

impl Pacer {
    /// No cap beyond `fps`.
    pub fn new(fps: u32) -> Self {
        Self { interval: Self::interval_for(fps), last: None }
    }

    fn interval_for(fps: u32) -> Duration {
        Duration::from_secs(1) / fps.max(1)
    }

    /// Caps the rate at `fps`.
    pub fn set_fps(&mut self, fps: u32) {
        self.interval = Self::interval_for(fps);
    }

    /// The wait before the next frame may be taken at `now` (zero when it may).
    pub fn wait(&self, now: Instant) -> Duration {
        self.last.map_or(Duration::ZERO, |t| (t + self.interval).saturating_duration_since(now))
    }

    /// A frame was taken at `now`.
    pub fn taken(&mut self, now: Instant) {
        self.last = Some(now);
    }
}

/// Waits out `pacer` within `timeout`; `true` when a frame may be taken now.
fn paced(pacer: &Pacer, timeout: Duration) -> bool {
    let wait = pacer.wait(Instant::now());
    if wait.is_zero() {
        return true;
    }
    std::thread::sleep(wait.min(timeout));
    false
}

/// What a live source should capture and how to encode it (the glue between HOST-10's virtual
/// displays, AVC444 and the pipeline).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiveRequest {
    /// A `CGDirectDisplayID` to capture, or `None` for the main display.
    pub display: Option<u32>,
    /// Encode AVC444 (two views from BGRA) instead of AVC420.
    pub avc444: bool,
    /// Encode Drift's HEVC extension codec instead of H.264 (EXT-5): the profile and transfer
    /// the host picked from a paired client's offer. 4:4:4 profiles are encoded from BGRA.
    pub hevc: Option<HevcSelection>,
}

/// The HEVC profile and transfer function a live picture is encoded with (EXT-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HevcSelection {
    /// The profile.
    pub profile: HevcProfile,
    /// SDR or HDR.
    pub transfer: Transfer,
}

/// One raw frame and what changed since the previous one.
#[derive(Debug)]
pub struct RawFrame {
    /// The pixels (NV12, IOSurface-backed).
    pub buffer: PixelBuffer,
    /// What changed.
    pub damage: Damage,
}

/// Where raw frames come from.
pub trait RawFrames: Send {
    /// The frames' size in pixels.
    fn size(&self) -> Size<u32>;
    /// The newest frame, waiting up to `timeout`.
    fn next(&mut self, timeout: Duration) -> Option<RawFrame>;
    /// Pointer changes since the last call (the screen's pointer is not in the picture).
    fn pointer(&mut self) -> Vec<drift_capture::cursor::CursorEvent> {
        Vec::new()
    }
}

/// One encoded picture ready for `send_avc420_frame`.
#[derive(Debug, Clone)]
pub struct LiveFrame {
    /// The access unit (Annex-B).
    pub au: Vec<u8>,
    /// What the client should show from it.
    pub regions: Vec<Avc420Region>,
    /// An IDR picture.
    pub keyframe: bool,
    /// AVC444's second view (`avc420EncodedBitstream2`, the chroma detail), when the frame is
    /// AVC444; `au` is then the main view.
    pub chroma: Option<Vec<u8>>,
    /// `au` is an HEVC access unit for Drift's private codec (EXT-5), not H.264.
    pub hevc: bool,
}

/// A live picture for a session.
pub trait LiveSource: Send {
    /// The desktop size.
    fn size(&self) -> Size<u32>;
    /// Make the next picture a keyframe (new client, resize, a frame the server dropped).
    fn request_keyframe(&mut self);
    /// The pictures that completed, waiting up to `timeout` for a frame. Under `backpressure`
    /// nothing is taken or encoded.
    fn next(&mut self, timeout: Duration, backpressure: bool) -> Result<Vec<LiveFrame>, FrameError>;
    /// Pointer changes since the last call.
    fn pointer(&mut self) -> Vec<drift_capture::cursor::CursorEvent> {
        Vec::new()
    }
    /// Whether the frames are AVC444.
    fn avc444(&self) -> bool {
        false
    }
    /// Applies a quality level (EXT-4): bit rate and frame-rate cap.
    fn set_quality(&mut self, level: crate::quality::QualityLevel) {
        let _ = level;
    }
    /// The HEVC profile the frames are encoded with, if they are HEVC (EXT-5).
    fn hevc(&self) -> Option<HevcSelection> {
        None
    }
}

/// [`RawFrames`] through HOST-4's real-time encoder.
pub struct LiveEncoder<R> {
    raw: R,
    encoder: VtEncoder,
    fps: u32,
    pacer: Pacer,
    /// Regions of pictures submitted but not yet out of the encoder, in order (no reordering).
    pending: VecDeque<Vec<Avc420Region>>,
    keyframe_due: bool,
    /// The newest frame, for a keyframe when nothing new arrives.
    last: Option<PixelBuffer>,
}

impl<R> std::fmt::Debug for LiveEncoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveEncoder").field("keyframe_due", &self.keyframe_due).finish_non_exhaustive()
    }
}

impl<R: RawFrames> LiveEncoder<R> {
    /// An encoder for `raw` at up to `fps` frames a second; the first picture is a keyframe.
    pub fn new(raw: R, fps: u32) -> Result<Self, FrameError> {
        let encoder = VtEncoder::new(EncoderConfig::realtime(raw.size(), fps))
            .map_err(|e| FrameError(e.to_string()))?;
        Ok(Self {
            raw,
            encoder,
            fps,
            pacer: Pacer::new(fps),
            pending: VecDeque::new(),
            keyframe_due: true,
            last: None,
        })
    }

    fn encode(
        &mut self,
        buffer: &PixelBuffer,
        damage: &Damage,
        keyframe: bool,
    ) -> Result<Vec<LiveFrame>, FrameError> {
        let size = buffer.size();
        if keyframe {
            self.encoder.request_keyframe();
            self.keyframe_due = false;
        }
        let regions = if keyframe { regions_for(&Damage::Full, size) } else { regions_for(damage, size) };
        let err = |e: drift_video::encode::EncodeError| FrameError(e.to_string());
        self.pending.push_back(regions);
        let encoded = self.encoder.encode(buffer, Instant::now()).map_err(err)?;
        let mut out = Vec::with_capacity(encoded.len());
        for frame in encoded {
            let regions = self.pending.pop_front().unwrap_or_default();
            let keyframe = frame.is_keyframe();
            let regions = if keyframe { regions_for(&Damage::Full, size) } else { regions };
            out.push(LiveFrame {
                au: frame.to_annex_b().map_err(err)?,
                regions,
                keyframe,
                chroma: None,
                hevc: false,
            });
        }
        Ok(out)
    }
}

impl<R: RawFrames> LiveSource for LiveEncoder<R> {
    fn size(&self) -> Size<u32> {
        self.raw.size()
    }

    fn request_keyframe(&mut self) {
        self.keyframe_due = true;
    }

    fn next(&mut self, timeout: Duration, backpressure: bool) -> Result<Vec<LiveFrame>, FrameError> {
        if backpressure {
            std::thread::sleep(timeout.min(Duration::from_millis(4)));
            return Ok(Vec::new());
        }
        if !self.keyframe_due && !paced(&self.pacer, timeout) {
            return Ok(Vec::new());
        }
        match self.raw.next(timeout) {
            Some(RawFrame { buffer, damage }) => {
                self.pacer.taken(Instant::now());
                let step = decide(false, damage != Damage::None, self.keyframe_due);
                let out = match step {
                    Step::Encode { keyframe } => self.encode(&buffer, &damage, keyframe)?,
                    Step::Wait => Vec::new(),
                };
                self.last = Some(buffer);
                Ok(out)
            }
            None => match (self.keyframe_due, self.last.clone()) {
                (true, Some(last)) => self.encode(&last, &Damage::Full, true),
                _ => Ok(Vec::new()),
            },
        }
    }

    fn pointer(&mut self) -> Vec<drift_capture::cursor::CursorEvent> {
        self.raw.pointer()
    }

    fn set_quality(&mut self, level: crate::quality::QualityLevel) {
        let q = crate::quality::settings_for(level, realtime_bitrate(self.raw.size(), self.fps), self.fps);
        if let Err(e) = self.encoder.set_average_bitrate(q.bitrate) {
            tracing::warn!(error = %e, "could not change the bit rate");
        }
        self.pacer.set_fps(q.fps);
    }
}

/// BGRA [`RawFrames`] through HOST-4's two-view AVC444 encoder (ADR `AVC444-client`): full
/// chroma for clients that confirmed AVC444, up to [`crate::codec::AVC444_MAX_PIXELS`]. Both views
/// carry the same regions; keyframes cover the whole picture.
pub struct LiveAvc444Encoder<R> {
    raw: R,
    encoder: Avc444Encoder,
    fps: u32,
    pacer: Pacer,
    pending: VecDeque<Vec<Avc420Region>>,
    keyframe_due: bool,
    last: Option<PixelBuffer>,
}

impl<R: RawFrames> LiveAvc444Encoder<R> {
    /// Two real-time encoders for `raw`'s size at `fps`; `raw` must deliver BGRA frames.
    pub fn new(raw: R, fps: u32) -> Result<Self, FrameError> {
        let encoder = Avc444Encoder::new(raw.size(), fps).map_err(|e| FrameError(e.to_string()))?;
        Ok(Self {
            raw,
            encoder,
            fps,
            pacer: Pacer::new(fps),
            pending: VecDeque::new(),
            keyframe_due: true,
            last: None,
        })
    }

    fn encode(
        &mut self,
        buffer: &PixelBuffer,
        damage: &Damage,
        keyframe: bool,
    ) -> Result<Vec<LiveFrame>, FrameError> {
        let size = buffer.size();
        if keyframe {
            self.encoder.request_keyframe();
            self.keyframe_due = false;
        }
        let regions = if keyframe { regions_for(&Damage::Full, size) } else { regions_for(damage, size) };
        let err = |e: drift_video::encode::EncodeError| FrameError(e.to_string());
        let bgra = buffer.read_bgra().map_err(err)?;
        self.pending.push_back(regions);
        let pictures =
            self.encoder.encode_bgra(&bgra, size.width as usize * 4, Instant::now()).map_err(err)?;
        let mut out = Vec::with_capacity(pictures.len());
        for picture in pictures {
            let regions = self.pending.pop_front().unwrap_or_default();
            let keyframe = picture.main.is_keyframe();
            let regions = if keyframe { regions_for(&Damage::Full, size) } else { regions };
            out.push(LiveFrame {
                au: picture.main.to_annex_b().map_err(err)?,
                regions,
                keyframe,
                chroma: Some(picture.aux.to_annex_b().map_err(err)?),
                hevc: false,
            });
        }
        Ok(out)
    }
}

impl<R: RawFrames> LiveSource for LiveAvc444Encoder<R> {
    fn size(&self) -> Size<u32> {
        self.raw.size()
    }

    fn request_keyframe(&mut self) {
        self.keyframe_due = true;
    }

    fn next(&mut self, timeout: Duration, backpressure: bool) -> Result<Vec<LiveFrame>, FrameError> {
        if backpressure {
            std::thread::sleep(timeout.min(Duration::from_millis(4)));
            return Ok(Vec::new());
        }
        if !self.keyframe_due && !paced(&self.pacer, timeout) {
            return Ok(Vec::new());
        }
        match self.raw.next(timeout) {
            Some(RawFrame { buffer, damage }) => {
                self.pacer.taken(Instant::now());
                let out = match decide(false, damage != Damage::None, self.keyframe_due) {
                    Step::Encode { keyframe } => self.encode(&buffer, &damage, keyframe)?,
                    Step::Wait => Vec::new(),
                };
                self.last = Some(buffer);
                Ok(out)
            }
            None => match (self.keyframe_due, self.last.clone()) {
                (true, Some(last)) => self.encode(&last, &Damage::Full, true),
                _ => Ok(Vec::new()),
            },
        }
    }

    fn pointer(&mut self) -> Vec<drift_capture::cursor::CursorEvent> {
        self.raw.pointer()
    }

    fn avc444(&self) -> bool {
        true
    }

    fn set_quality(&mut self, level: crate::quality::QualityLevel) {
        let q = crate::quality::settings_for(level, realtime_bitrate(self.raw.size(), self.fps), self.fps);
        if let Err(e) = self.encoder.set_average_bitrate(q.bitrate) {
            tracing::warn!(error = %e, "could not change the bit rate");
        }
        self.pacer.set_fps(q.fps);
    }
}

/// [`RawFrames`] through the real-time HEVC encoder (EXT-5, ADR `EXT-5-hevc-hdr`), for a paired
/// Drift client that negotiated it. The frames carry the same AVC420 regions as H.264's; 4:4:4
/// and 10-bit profiles are converted by VideoToolbox from whatever the source delivers (BGRA for
/// 4:4:4, so chroma is not lost before the encoder).
pub struct LiveHevcEncoder<R> {
    raw: R,
    encoder: HevcEncoder,
    selection: HevcSelection,
    pending: VecDeque<Vec<Avc420Region>>,
    keyframe_due: bool,
    last: Option<PixelBuffer>,
}

impl<R> std::fmt::Debug for LiveHevcEncoder<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveHevcEncoder").field("selection", &self.selection).finish_non_exhaustive()
    }
}

impl<R: RawFrames> LiveHevcEncoder<R> {
    /// A real-time HEVC encoder for `raw` at up to `fps` frames a second.
    pub fn new(raw: R, fps: u32, selection: HevcSelection) -> Result<Self, FrameError> {
        let config = HevcConfig::realtime(raw.size(), fps, selection.profile, selection.transfer);
        let encoder = HevcEncoder::new(config).map_err(|e| FrameError(e.to_string()))?;
        Ok(Self { raw, encoder, selection, pending: VecDeque::new(), keyframe_due: true, last: None })
    }

    fn encode(
        &mut self,
        buffer: &PixelBuffer,
        damage: &Damage,
        keyframe: bool,
    ) -> Result<Vec<LiveFrame>, FrameError> {
        let size = buffer.size();
        if keyframe {
            self.encoder.request_keyframe();
            self.keyframe_due = false;
        }
        let regions = if keyframe { regions_for(&Damage::Full, size) } else { regions_for(damage, size) };
        self.pending.push_back(regions);
        let encoded = self.encoder.encode(buffer, Instant::now()).map_err(|e| FrameError(e.to_string()))?;
        let mut out = Vec::with_capacity(encoded.len());
        for frame in encoded {
            let regions = self.pending.pop_front().unwrap_or_default();
            let keyframe = frame.is_keyframe();
            let regions = if keyframe { regions_for(&Damage::Full, size) } else { regions };
            out.push(LiveFrame { au: frame.into_annex_b(), regions, keyframe, chroma: None, hevc: true });
        }
        Ok(out)
    }
}

impl<R: RawFrames> LiveSource for LiveHevcEncoder<R> {
    fn size(&self) -> Size<u32> {
        self.raw.size()
    }

    fn request_keyframe(&mut self) {
        self.keyframe_due = true;
    }

    fn next(&mut self, timeout: Duration, backpressure: bool) -> Result<Vec<LiveFrame>, FrameError> {
        if backpressure {
            std::thread::sleep(timeout.min(Duration::from_millis(4)));
            return Ok(Vec::new());
        }
        match self.raw.next(timeout) {
            Some(RawFrame { buffer, damage }) => {
                let out = match decide(false, damage != Damage::None, self.keyframe_due) {
                    Step::Encode { keyframe } => self.encode(&buffer, &damage, keyframe)?,
                    Step::Wait => Vec::new(),
                };
                self.last = Some(buffer);
                Ok(out)
            }
            None => match (self.keyframe_due, self.last.clone()) {
                (true, Some(last)) => self.encode(&last, &Damage::Full, true),
                _ => Ok(Vec::new()),
            },
        }
    }

    fn pointer(&mut self) -> Vec<drift_capture::cursor::CursorEvent> {
        self.raw.pointer()
    }

    fn hevc(&self) -> Option<HevcSelection> {
        Some(self.selection)
    }
}

/// A synthetic screen: a square moving over a dark background at a fixed rate, with exact
/// damage. Stands in for capture in tests (no Screen Recording grant needed).
#[derive(Debug)]
pub struct MovingSquare {
    size: Size<u32>,
    interval: Duration,
    next_at: Instant,
    frame: u32,
    position: Option<(u32, u32)>,
    bgra: bool,
}

const SQUARE: u32 = 64;

impl MovingSquare {
    /// A `size` screen changing `fps` times a second.
    pub fn new(size: Size<u32>, fps: u32) -> Self {
        let interval = Duration::from_secs(1) / fps.max(1);
        Self { size, interval, next_at: Instant::now(), frame: 0, position: None, bgra: false }
    }

    /// The same screen as BGRA frames (the format AVC444 is encoded from).
    pub fn bgra(size: Size<u32>, fps: u32) -> Self {
        Self { bgra: true, ..Self::new(size, fps) }
    }

    fn square_at(&self, frame: u32) -> (u32, u32) {
        let span_x = self.size.width.saturating_sub(SQUARE).max(1);
        let span_y = self.size.height.saturating_sub(SQUARE).max(1);
        (((frame * 8) % span_x) & !1, ((frame * 6) % span_y) & !1)
    }

    fn render(&self, (sx, sy): (u32, u32)) -> Option<PixelBuffer> {
        if self.bgra {
            let buffer = PixelBuffer::new_bgra(self.size).ok()?;
            let inside = |x: u32, y: u32| (sx..sx + SQUARE).contains(&x) && (sy..sy + SQUARE).contains(&y);
            // A red square on dark grey: sharp chroma edges, what AVC444 is for.
            buffer
                .fill_bgra(|x, y| if inside(x, y) { [40, 40, 220, 255] } else { [40, 40, 40, 255] })
                .ok()?;
            return Some(buffer);
        }
        let (w, h) = (self.size.width as usize, self.size.height as usize);
        let mut y = vec![40u8; w * h];
        let mut uv = vec![128u8; w * h.div_ceil(2)];
        for row in sy..(sy + SQUARE).min(self.size.height) {
            for col in sx..(sx + SQUARE).min(self.size.width) {
                y[row as usize * w + col as usize] = 200;
            }
        }
        for row in (sy / 2)..((sy + SQUARE) / 2).min(self.size.height.div_ceil(2)) {
            for col in (sx / 2)..((sx + SQUARE) / 2).min(self.size.width / 2) {
                let i = row as usize * w + col as usize * 2;
                uv[i] = 90;
                uv[i + 1] = 200;
            }
        }
        let planes = Nv12Planes::new(self.size, y, uv)?;
        let buffer = PixelBuffer::new_nv12(self.size).ok()?;
        buffer.write_nv12(&planes).ok()?;
        Some(buffer)
    }
}

impl RawFrames for MovingSquare {
    fn size(&self) -> Size<u32> {
        self.size
    }

    fn next(&mut self, timeout: Duration) -> Option<RawFrame> {
        let now = Instant::now();
        if self.next_at > now {
            let wait = self.next_at - now;
            if wait > timeout {
                std::thread::sleep(timeout);
                return None;
            }
            std::thread::sleep(wait);
        }
        self.next_at += self.interval;
        let at = self.square_at(self.frame);
        self.frame += 1;
        let buffer = self.render(at)?;
        let rect =
            |(x, y): (u32, u32)| drift_capture::rect::PixelRect { x, y, width: SQUARE, height: SQUARE };
        let damage = match self.position.replace(at) {
            None => Damage::Full,
            Some(old) => drift_capture::rect::coalesce(&[rect(old), rect(at)], self.size, 16),
        };
        Some(RawFrame { buffer, damage })
    }
}
