//! Session statistics (fps, bit rate, latency percentiles) sampled about once per second. Pure.
//!
//! The latencies are the two numbers plan M9-1 budgets and `cargo xtask e2e --bench` reports:
//! **decode + present** (wire → presented frame, p95) and **input-to-wire** (an `InputEvent`
//! reaching the actor → its fast-path frame written, p99).

use std::time::{Duration, Instant};

use crate::session::SessionStats;

/// How often [`StatsMeter::sample`] produces a [`SessionStats`].
pub const STATS_PERIOD: Duration = Duration::from_secs(1);

/// A bounded reservoir of latency samples with nearest-rank percentiles.
///
/// Samples are kept in arrival order and the oldest are dropped once [`Percentiles::CAPACITY`]
/// is reached, so a long-running session cannot grow without bound. A statistics window holds
/// about one second of samples, far below the capacity.
#[derive(Debug, Clone, Default)]
pub struct Percentiles {
    samples: std::collections::VecDeque<Duration>,
}

impl Percentiles {
    /// How many samples are kept.
    pub const CAPACITY: usize = 4096;

    /// Adds one sample, dropping the oldest when the reservoir is full.
    pub fn record(&mut self, latency: Duration) {
        if self.samples.len() == Self::CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back(latency);
    }

    /// How many samples are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether nothing was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Forgets every sample.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// The `p`-th percentile in milliseconds (nearest rank, `p` clamped to `0..=100`), or
    /// `0.0` when nothing was recorded.
    #[must_use]
    pub fn percentile_ms(&self, p: u8) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut sorted: Vec<Duration> = self.samples.iter().copied().collect();
        sorted.sort_unstable();
        // Nearest rank: the ceil(p/100 * n)-th smallest sample, 1-based.
        let n = sorted.len();
        let rank = (usize::from(p.min(100)) * n).div_ceil(100).max(1);
        #[expect(clippy::cast_possible_truncation, reason = "a latency in ms is a display value")]
        let ms = sorted.get(rank - 1).map_or(0.0, |d| d.as_secs_f64() * 1000.0) as f32;
        ms
    }
}

/// Counts presented frames, received bytes and latencies between samples.
#[derive(Debug, Clone, Default)]
pub struct StatsMeter {
    since: Option<Instant>,
    frames: u64,
    bytes: u64,
    frame_latency: Percentiles,
    input_latency: Percentiles,
}

impl StatsMeter {
    /// A meter whose first window starts at `now`.
    pub fn new(now: Instant) -> Self {
        Self { since: Some(now), ..Self::default() }
    }

    /// Starts a new window at `now`, discarding counts (e.g. after a reconnect or unhide).
    pub fn restart(&mut self, now: Instant) {
        *self = Self::new(now);
    }

    /// One decode-and-present latency (wire payload → the frame sink reported it presented).
    pub fn frame_latency(&mut self, latency: Duration) {
        self.frame_latency.record(latency);
    }

    /// Many decode-and-present latencies at once (the render thread collects them).
    pub fn frame_latencies(&mut self, latencies: impl IntoIterator<Item = Duration>) {
        for l in latencies {
            self.frame_latency.record(l);
        }
    }

    /// One input-to-wire latency (an `InputEvent` arriving → its frame written to the socket).
    pub fn input_latency(&mut self, latency: Duration) {
        self.input_latency.record(latency);
    }

    /// `n` frames were presented.
    pub fn frames(&mut self, n: u64) {
        self.frames = self.frames.saturating_add(n);
    }

    /// `n` payload bytes were received.
    pub fn bytes(&mut self, n: u64) {
        self.bytes = self.bytes.saturating_add(n);
    }

    /// When the current window ends.
    ///
    /// # Panics
    /// Never: a meter always has a window (`Default` starts one lazily on the first sample).
    pub fn deadline(&self) -> Instant {
        self.since.unwrap_or_else(Instant::now) + STATS_PERIOD
    }

    /// The stats of the finished window, once [`STATS_PERIOD`] has elapsed since it started;
    /// `unacked_frames` is reported as given.
    pub fn sample(&mut self, now: Instant, unacked_frames: u32) -> Option<SessionStats> {
        let since = *self.since.get_or_insert(now);
        let elapsed = now.saturating_duration_since(since);
        if elapsed < STATS_PERIOD {
            return None;
        }
        let secs = elapsed.as_secs_f64();
        #[expect(clippy::cast_possible_truncation, reason = "fps and bit rate are display values")]
        let stats = SessionStats {
            fps: (self.frames as f64 / secs) as f32,
            bitrate_bps: (self.bytes as f64 * 8.0 / secs) as u64,
            frame_latency_p95_ms: self.frame_latency.percentile_ms(95),
            input_to_wire_p99_ms: self.input_latency.percentile_ms(99),
            unacked_frames,
        };
        self.restart(now);
        Some(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_once_per_period() {
        let t0 = Instant::now();
        let mut m = StatsMeter::new(t0);
        m.frames(60);
        m.bytes(150_000);
        assert_eq!(m.sample(t0 + Duration::from_millis(999), 0), None, "window not over");
        let stats = m.sample(t0 + STATS_PERIOD, 3).expect("a sample after one second");
        assert!((stats.fps - 60.0).abs() < 0.1, "{stats:?}");
        assert_eq!(stats.bitrate_bps, 1_200_000);
        assert_eq!(stats.unacked_frames, 3);
        assert_eq!(m.sample(t0 + STATS_PERIOD, 0), None, "the window restarted");
        assert_eq!(m.deadline(), t0 + STATS_PERIOD * 2);
    }

    #[test]
    fn restart_discards_counts() {
        let t0 = Instant::now();
        let mut m = StatsMeter::new(t0);
        m.frames(10);
        m.restart(t0 + Duration::from_millis(500));
        let stats = m.sample(t0 + Duration::from_millis(1500), 0).expect("a sample");
        assert_eq!(stats.fps, 0.0);
    }
}
