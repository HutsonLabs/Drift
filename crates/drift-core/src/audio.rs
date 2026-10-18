//! PCM audio shared by the host's rdpsnd server and the client's output (task **HOST-11**, ADR
//! `HOST-11-clipboard-and-audio`).
//!
//! Drift sends uncompressed 16-bit stereo PCM: on a local network the ~1.5 Mbit/s cost is small
//! next to the picture, and it needs no codec on either end.

use std::time::Duration;

/// A PCM stream format (little-endian, interleaved, signed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PcmFormat {
    /// Frames per second.
    pub sample_rate: u32,
    /// Interleaved channels.
    pub channels: u16,
    /// Bits per sample.
    pub bits_per_sample: u16,
}

impl PcmFormat {
    /// 48 kHz stereo, 16-bit: what ScreenCaptureKit is asked for.
    pub const STEREO_48K_S16: Self = Self { sample_rate: 48_000, channels: 2, bits_per_sample: 16 };
    /// 44.1 kHz stereo, 16-bit: the classic RDP format, offered for other clients.
    pub const STEREO_44K1_S16: Self = Self { sample_rate: 44_100, channels: 2, bits_per_sample: 16 };

    /// Bytes per interleaved frame.
    pub const fn bytes_per_frame(&self) -> u32 {
        self.channels as u32 * (self.bits_per_sample as u32 / 8)
    }

    /// Bytes per second.
    pub const fn bytes_per_second(&self) -> u32 {
        self.sample_rate * self.bytes_per_frame()
    }

    /// How long `bytes` of audio plays (partial frames count as nothing).
    pub fn duration_of(&self, bytes: usize) -> Duration {
        let per_frame = self.bytes_per_frame() as usize;
        if per_frame == 0 || self.sample_rate == 0 {
            return Duration::ZERO;
        }
        let frames = (bytes / per_frame) as u64;
        Duration::from_nanos(frames * 1_000_000_000 / u64::from(self.sample_rate))
    }
}

/// Whether a chunk of audio should be played or dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// Queue it for playback.
    Play,
    /// Drop it: the output is already too far behind.
    Drop,
}

/// The client's latency cap: audio that would sit behind more than [`Self::max_backlog`] of
/// queued sound is dropped, so the sound never drifts far behind the picture after a stall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayoutBudget {
    /// The most queued audio allowed after the new chunk.
    pub max_backlog: Duration,
}

impl Default for PlayoutBudget {
    fn default() -> Self {
        Self { max_backlog: Duration::from_millis(200) }
    }
}

impl PlayoutBudget {
    /// Whether a `chunk` may be queued behind `queued` of unplayed audio. An empty queue always
    /// takes the chunk, so the output never starves on large chunks.
    pub fn admit(&self, queued: Duration, chunk: Duration) -> Admit {
        if queued.is_zero() || queued + chunk <= self.max_backlog { Admit::Play } else { Admit::Drop }
    }
}
