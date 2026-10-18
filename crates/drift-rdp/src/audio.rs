//! Remote audio (task **HOST-11**, ADR `HOST-11-clipboard-and-audio`): IronRDP's `Rdpsnd` client
//! hands 16-bit PCM waves to an [`AudioSink`] the app provides (an `AVAudioEngine` player in
//! `drift-apple`). Without a sink the client does not offer the sound channel at all.

use std::borrow::Cow;
use std::sync::Arc;

use drift_core::audio::PcmFormat;
use ironrdp_rdpsnd::client::{Rdpsnd, RdpsndClientHandler};
use ironrdp_rdpsnd::pdu::{AudioFormat, PitchPdu, VolumePdu, WaveFormat};

/// Where the session's sound goes. Called from the session actor's thread; implementations
/// should only queue.
pub trait AudioSink: Send + Sync {
    /// A stream in `format` begins (also when the format changes).
    fn start(&self, format: PcmFormat);
    /// Interleaved little-endian PCM in the current format.
    fn play(&self, pcm: &[u8]);
    /// The server closed the stream.
    fn stop(&self);
}

/// An [`AudioSink`] for [`crate::SessionOptions::audio`] (compared by identity).
#[derive(Clone)]
pub struct AudioOutput(Arc<dyn AudioSink>);

impl AudioOutput {
    /// Wraps a sink.
    pub fn new(sink: impl AudioSink + 'static) -> Self {
        Self(Arc::new(sink))
    }

    /// Wraps a shared sink.
    pub fn from_arc(sink: Arc<dyn AudioSink>) -> Self {
        Self(sink)
    }

    pub(crate) fn client(&self) -> Rdpsnd {
        Rdpsnd::new(Box::new(Handler {
            sink: Arc::clone(&self.0),
            formats: client_audio_formats(),
            current: None,
        }))
    }
}

impl std::fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AudioOutput(..)")
    }
}

impl PartialEq for AudioOutput {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for AudioOutput {}

/// The PCM formats Drift's client accepts, most preferred first (the host offers the same).
pub fn client_formats() -> Vec<PcmFormat> {
    vec![PcmFormat::STEREO_48K_S16, PcmFormat::STEREO_44K1_S16]
}

/// `format` as an MS-RDPEA `WAVE_FORMAT_PCM` entry.
pub fn to_audio_format(format: PcmFormat) -> AudioFormat {
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

/// The PCM format of an MS-RDPEA entry (`None` for anything compressed).
pub fn pcm_of(format: &AudioFormat) -> Option<PcmFormat> {
    (format.format == WaveFormat::PCM).then_some(PcmFormat {
        sample_rate: format.n_samples_per_sec,
        channels: format.n_channels,
        bits_per_sample: format.bits_per_sample,
    })
}

fn client_audio_formats() -> Vec<AudioFormat> {
    client_formats().into_iter().map(to_audio_format).collect()
}

struct Handler {
    sink: Arc<dyn AudioSink>,
    formats: Vec<AudioFormat>,
    current: Option<PcmFormat>,
}

impl std::fmt::Debug for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handler").field("current", &self.current).finish_non_exhaustive()
    }
}

impl RdpsndClientHandler for Handler {
    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn wave(&mut self, format: &AudioFormat, _ts: u32, data: Cow<'_, [u8]>) {
        let Some(pcm) = pcm_of(format) else { return };
        if self.current != Some(pcm) {
            self.current = Some(pcm);
            self.sink.start(pcm);
        }
        self.sink.play(&data);
    }

    fn set_volume(&mut self, _volume: VolumePdu) {}

    fn set_pitch(&mut self, _pitch: PitchPdu) {}

    fn close(&mut self) {
        if self.current.take().is_some() {
            self.sink.stop();
        }
    }
}
