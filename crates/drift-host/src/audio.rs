//! The host's sound: the rdpsnd server (task **HOST-11**, ADR `HOST-11-clipboard-and-audio`).
//!
//! The host offers 16-bit stereo PCM at 48 and 44.1 kHz ([`host_formats`]) and streams what an
//! [`AudioSource`] delivers: the Mac's system audio from ScreenCaptureKit
//! ([`SystemAudio`], macOS), or a synthetic [`ToneSource`] in tests. Each chunk goes to the
//! session as a Wave PDU through `ServerEvent::Rdpsnd`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use drift_core::audio::PcmFormat;
use ironrdp_rdpsnd::pdu::{AudioFormat, WaveFormat};
use ironrdp_rdpsnd::server::{NegotiatedFormat, RdpsndError};
use ironrdp_server::{
    RdpsndServerHandler, RdpsndServerMessage, ServerEvent, ServerEventSender, SoundServerFactory,
};
use tokio::sync::mpsc::UnboundedSender;

/// Delivers PCM chunks to the stream.
pub type Deliver = Box<dyn FnMut(Vec<u8>) + Send>;

/// Where the host's sound comes from.
pub trait AudioSource: Send {
    /// The PCM format the source produces.
    fn format(&self) -> PcmFormat;
    /// Starts delivering chunks (on a thread of the source's choosing).
    fn start(&mut self, deliver: Deliver) -> Result<(), String>;
    /// Stops delivering.
    fn stop(&mut self);
}

/// Makes the audio source for a connection.
pub type AudioSourceFactory = Box<dyn Fn() -> Box<dyn AudioSource> + Send>;

/// The formats the host offers, most preferred first (Drift's client offers the same).
pub fn host_formats() -> Vec<PcmFormat> {
    vec![PcmFormat::STEREO_48K_S16, PcmFormat::STEREO_44K1_S16]
}

/// Which of the formats both ends share to stream from a source at `source_rate`: the one with
/// that rate, else the first shared one (`None` when nothing is shared).
pub fn choose_format(common: &[PcmFormat], source_rate: u32) -> Option<usize> {
    common.iter().position(|f| f.sample_rate == source_rate).or_else(|| (!common.is_empty()).then_some(0))
}

fn to_audio_format(format: PcmFormat) -> AudioFormat {
    AudioFormat {
        format: WaveFormat::PCM,
        n_channels: format.channels,
        n_samples_per_sec: format.sample_rate,
        n_avg_bytes_per_sec: format.bytes_per_second(),
        n_block_align: u16::try_from(format.bytes_per_frame()).unwrap_or(u16::MAX),
        bits_per_sample: format.bits_per_sample,
        data: None,
    }
}

fn pcm_of(format: &AudioFormat) -> Option<PcmFormat> {
    (format.format == WaveFormat::PCM).then_some(PcmFormat {
        sample_rate: format.n_samples_per_sec,
        channels: format.n_channels,
        bits_per_sample: format.bits_per_sample,
    })
}

fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The rdpsnd server factory handed to `ironrdp-server`.
pub struct HostSound {
    factory: AudioSourceFactory,
    sender: Arc<Mutex<Option<UnboundedSender<ServerEvent>>>>,
    waves: Arc<AtomicU32>,
}

impl HostSound {
    /// Streams what `factory`'s sources deliver.
    pub fn new(factory: AudioSourceFactory) -> Self {
        Self { factory, sender: Arc::default(), waves: Arc::default() }
    }

    /// Waves sent so far (for stats and tests).
    pub fn waves(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.waves)
    }
}

impl ServerEventSender for HostSound {
    fn set_sender(&mut self, sender: UnboundedSender<ServerEvent>) {
        *lock(&self.sender) = Some(sender);
    }
}

impl SoundServerFactory for HostSound {
    fn build_backend(&self) -> Box<dyn RdpsndServerHandler> {
        Box::new(SoundHandler {
            formats: host_formats().into_iter().map(to_audio_format).collect(),
            source: (self.factory)(),
            sender: Arc::clone(&self.sender),
            waves: Arc::clone(&self.waves),
        })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the audio source did not start: {0}")]
struct SourceError(String);

struct SoundHandler {
    formats: Vec<AudioFormat>,
    source: Box<dyn AudioSource>,
    sender: Arc<Mutex<Option<UnboundedSender<ServerEvent>>>>,
    waves: Arc<AtomicU32>,
}

impl std::fmt::Debug for SoundHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoundHandler").finish_non_exhaustive()
    }
}

impl RdpsndServerHandler for SoundHandler {
    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn choose_format<'a>(&mut self, common: &'a [NegotiatedFormat]) -> Option<&'a NegotiatedFormat> {
        let pcm: Vec<PcmFormat> = common.iter().filter_map(|n| pcm_of(n.format())).collect();
        let chosen = choose_format(&pcm, self.source.format().sample_rate)?;
        let target = pcm[chosen];
        common.iter().find(|n| pcm_of(n.format()) == Some(target))
    }

    fn start(&mut self, format: &NegotiatedFormat) -> Result<(), Box<dyn RdpsndError>> {
        if pcm_of(format.format()) != Some(self.source.format()) {
            tracing::info!(?format, "the client's format differs from the source's; streaming anyway");
        }
        let Some(sender) = lock(&self.sender).clone() else {
            return Err(Box::new(SourceError("no session to send to".into())));
        };
        let waves = Arc::clone(&self.waves);
        let origin = Instant::now();
        let deliver: Deliver = Box::new(move |pcm| {
            let ts = u32::try_from(origin.elapsed().as_millis()).unwrap_or(u32::MAX);
            if sender.send(ServerEvent::Rdpsnd(RdpsndServerMessage::Wave(pcm, ts))).is_ok() {
                waves.fetch_add(1, Ordering::Relaxed);
            }
        });
        self.source.start(deliver).map_err(|e| Box::new(SourceError(e)) as Box<dyn RdpsndError>)
    }

    fn stop(&mut self) {
        self.source.stop();
    }
}

/// A deterministic test tone: 48 kHz stereo, a sawtooth on the left and `left ^ 0x5555` on the
/// right, in 10 ms chunks.
#[derive(Debug, Default)]
pub struct ToneSource {
    next_frame: u64,
    running: Option<(Arc<AtomicBool>, std::thread::JoinHandle<()>)>,
}

impl ToneSource {
    /// A tone starting at frame 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// The next `frames` frames as PCM.
    pub fn render(&mut self, frames: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(frames * 4);
        for _ in 0..frames {
            let left = i16::try_from(self.next_frame * 37 % 2000).unwrap_or(0) - 1000;
            let right = left ^ 0x5555;
            out.extend_from_slice(&left.to_le_bytes());
            out.extend_from_slice(&right.to_le_bytes());
            self.next_frame += 1;
        }
        out
    }
}

impl AudioSource for ToneSource {
    fn format(&self) -> PcmFormat {
        PcmFormat::STEREO_48K_S16
    }

    fn start(&mut self, mut deliver: Deliver) -> Result<(), String> {
        self.stop();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let mut tone = Self { next_frame: self.next_frame, running: None };
        let thread = std::thread::Builder::new()
            .name("drift-host-tone".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    deliver(tone.render(480));
                    std::thread::sleep(Duration::from_millis(10));
                }
            })
            .map_err(|e| e.to_string())?;
        self.running = Some((stop, thread));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some((stop, thread)) = self.running.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
        }
    }
}

impl Drop for ToneSource {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(target_os = "macos")]
pub use system::SystemAudio;

#[cfg(target_os = "macos")]
mod system {
    use drift_capture::audio::AudioCapture;
    use drift_core::audio::PcmFormat;

    use super::{AudioSource, Deliver};

    /// The Mac's system audio through ScreenCaptureKit (needs Screen Recording).
    #[derive(Debug, Default)]
    pub struct SystemAudio {
        capture: Option<AudioCapture>,
    }

    impl AudioSource for SystemAudio {
        fn format(&self) -> PcmFormat {
            PcmFormat::STEREO_48K_S16
        }

        fn start(&mut self, deliver: Deliver) -> Result<(), String> {
            let deliver = std::sync::Mutex::new(deliver);
            let capture = AudioCapture::start(move |chunk| {
                if let Ok(mut deliver) = deliver.lock() {
                    deliver(chunk.pcm);
                }
            })
            .map_err(|e| e.to_string())?;
            self.capture = Some(capture);
            Ok(())
        }

        fn stop(&mut self) {
            self.capture = None;
        }
    }
}
