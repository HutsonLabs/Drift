//! Remote sound on the Mac, iPhone and iPad (task **HOST-11**, ADR `HOST-11-clipboard-and-audio`).
//!
//! A humble object around `AVAudioEngine`: one `AVAudioPlayerNode` into the main mixer, fed
//! 16-bit stereo PCM converted to the engine's float format. The pure
//! [`PlayoutBudget`](drift_core::audio::PlayoutBudget) decides what to drop, so the sound never
//! falls more than 200 ms behind the picture after a network stall.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use block2::RcBlock;
use drift_core::audio::{Admit, PcmFormat, PlayoutBudget};
use objc2::AllocAnyThread as _;
use objc2::rc::Retained;
use objc2_avf_audio::{AVAudioEngine, AVAudioFormat, AVAudioPCMBuffer, AVAudioPlayerNode};

struct Engine {
    engine: Retained<AVAudioEngine>,
    node: Retained<AVAudioPlayerNode>,
    format: Retained<AVAudioFormat>,
    pcm: PcmFormat,
}

/// Plays a session's PCM; safe to call from any thread.
pub struct Player {
    engine: Mutex<Option<Engine>>,
    /// Frames scheduled on the player node and not yet played.
    queued: Arc<AtomicU64>,
    dropped: AtomicU64,
    budget: PlayoutBudget,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player").field("queued", &self.queued).field("dropped", &self.dropped).finish()
    }
}

// SAFETY: `AVAudioEngine` and `AVAudioPlayerNode` may be driven from any thread (Apple documents
// scheduling buffers from arbitrary threads); every access goes through the mutex.
unsafe impl Send for Player {}
// SAFETY: as above.
unsafe impl Sync for Player {}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Player {
    /// A stopped player.
    pub fn new() -> Self {
        Self {
            engine: Mutex::new(None),
            queued: Arc::default(),
            dropped: AtomicU64::new(0),
            budget: PlayoutBudget::default(),
        }
    }

    /// Starts (or restarts) the output for `pcm`.
    pub fn start(&self, pcm: PcmFormat) -> Result<(), String> {
        self.stop();
        if pcm.bits_per_sample != 16 || !(1..=2).contains(&pcm.channels) {
            return Err(format!("unsupported PCM format {pcm:?}"));
        }
        activate_session();
        // SAFETY: plain constructors and graph setup on fresh objects; the node is attached
        // before it is connected, as AVAudioEngine requires.
        let engine = unsafe {
            let engine = AVAudioEngine::new();
            let node = AVAudioPlayerNode::new();
            let format = AVAudioFormat::initStandardFormatWithSampleRate_channels(
                AVAudioFormat::alloc(),
                f64::from(pcm.sample_rate),
                u32::from(pcm.channels),
            )
            .ok_or("no float format for this rate")?;
            engine.attachNode(&node);
            engine.connect_to_format(&node, &engine.mainMixerNode(), Some(&format));
            engine.startAndReturnError().map_err(|e| e.localizedDescription().to_string())?;
            node.play();
            Engine { engine, node, format, pcm }
        };
        *lock(&self.engine) = Some(engine);
        Ok(())
    }

    /// Queues interleaved 16-bit PCM in the started format; drops it when the output is already
    /// [`PlayoutBudget::max_backlog`] behind.
    pub fn play(&self, bytes: &[u8]) {
        let guard = lock(&self.engine);
        let Some(engine) = guard.as_ref() else { return };
        let channels = usize::from(engine.pcm.channels);
        let frames = bytes.len() / (2 * channels);
        if frames == 0 {
            return;
        }
        let rate = u64::from(engine.pcm.sample_rate.max(1));
        let queued = Duration::from_nanos(self.queued.load(Ordering::Relaxed) * 1_000_000_000 / rate);
        let chunk = Duration::from_nanos(frames as u64 * 1_000_000_000 / rate);
        if self.budget.admit(queued, chunk) == Admit::Drop {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let Ok(capacity) = u32::try_from(frames) else { return };
        // SAFETY: a buffer of `capacity` frames in the engine's standard (deinterleaved float)
        // format; `floatChannelData` points at `channels` planes of at least `capacity` floats.
        unsafe {
            let Some(buffer) = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                &engine.format,
                capacity,
            ) else {
                return;
            };
            let planes = buffer.floatChannelData();
            if planes.is_null() {
                return;
            }
            for channel in 0..channels {
                let plane = (*planes.add(channel)).as_ptr();
                for frame in 0..frames {
                    let at = (frame * channels + channel) * 2;
                    let sample = i16::from_le_bytes([bytes[at], bytes[at + 1]]);
                    *plane.add(frame) = f32::from(sample) / 32768.0;
                }
            }
            buffer.setFrameLength(capacity);
            self.queued.fetch_add(frames as u64, Ordering::Relaxed);
            let queued = Arc::clone(&self.queued);
            let done = RcBlock::new(move || {
                let _ = queued.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |q| {
                    Some(q.saturating_sub(frames as u64))
                });
            });
            engine.node.scheduleBuffer_completionHandler(&buffer, RcBlock::as_ptr(&done));
        }
    }

    /// Stops the output and forgets queued sound.
    pub fn stop(&self) {
        if let Some(engine) = lock(&self.engine).take() {
            // SAFETY: stopping a running node and engine.
            unsafe {
                engine.node.stop();
                engine.engine.stop();
            }
        }
        self.queued.store(0, Ordering::Relaxed);
    }

    /// Chunks dropped by the latency cap so far.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

/// iOS: play alongside other apps' sound, and keep playing with the silent switch on (a remote
/// desktop's sound is the user's choice, like a video call's).
#[cfg(target_os = "ios")]
fn activate_session() {
    use objc2_avf_audio::{AVAudioSession, AVAudioSessionCategoryOptions, AVAudioSessionCategoryPlayback};
    // SAFETY: the shared session; a constant category; errors are logged, not fatal.
    unsafe {
        let session = AVAudioSession::sharedInstance();
        if let Some(category) = AVAudioSessionCategoryPlayback
            && let Err(e) =
                session.setCategory_withOptions_error(category, AVAudioSessionCategoryOptions::MixWithOthers)
        {
            tracing::warn!(error = %e.localizedDescription(), "could not set the audio session category");
        }
        if let Err(e) = session.setActive_error(true) {
            tracing::warn!(error = %e.localizedDescription(), "could not activate the audio session");
        }
    }
}

#[cfg(not(target_os = "ios"))]
fn activate_session() {}
