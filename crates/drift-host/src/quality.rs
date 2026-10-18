//! The host's quality ladder (task **EXT-4**; ADR `EXT-4-quality-hints`). Pure.
//!
//! HOST-7's [`BacklogMeter`](crate::link::BacklogMeter) already smooths the link into
//! [`Backlog`] levels. [`QualityPolicy`] turns them into a [`QualityLevel`]:
//!
//! * `Backlogged` steps down at once, then at most once per [`DOWN_HOLD`], so the lower rate has
//!   time to drain the queue before the next step;
//! * `Building` for [`BUILDING_FOR`] steps down once;
//! * `Clear` for [`CLEAR_FOR`] steps up one level, never above the client's cap
//!   ([`QualityPolicy::set_max`]).
//!
//! [`settings_for`] maps a level to the encoder's bit rate and the frame-rate cap.

use std::time::{Duration, Instant};

pub use drift_ext::status::QualityLevel;

use crate::link::Backlog;

/// After a step down, the next one waits this long.
pub const DOWN_HOLD: Duration = Duration::from_secs(2);
/// A building backlog this long steps down.
pub const BUILDING_FOR: Duration = Duration::from_secs(3);
/// A clear link this long steps up.
pub const CLEAR_FOR: Duration = Duration::from_secs(10);
/// The lowest bit rate any level uses.
pub const MIN_BITRATE: u32 = 1_000_000;

/// The encoder settings of a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualitySettings {
    /// Average bit rate, bit/s.
    pub bitrate: u32,
    /// Frame-rate cap.
    pub fps: u32,
}

/// The settings of `level`, from the full-quality `bitrate` and `fps`: 100 %, 70 %, 45 % and
/// 25 % of the bit rate; the two lower levels also halve the frame rate (fewer, sharper pictures
/// read better than many smeared ones on a slow link).
pub fn settings_for(level: QualityLevel, bitrate: u32, fps: u32) -> QualitySettings {
    let (percent, fps) = match level {
        QualityLevel::Best => (100, fps),
        QualityLevel::High => (70, fps),
        QualityLevel::Medium => (45, (fps / 2).max(1)),
        QualityLevel::Low => (25, (fps / 2).max(1)),
    };
    let scaled = u64::from(bitrate) * percent / 100;
    QualitySettings { bitrate: u32::try_from(scaled).unwrap_or(u32::MAX).max(MIN_BITRATE), fps }
}

/// The quality ladder for one session.
#[derive(Debug, Clone)]
pub struct QualityPolicy {
    level: QualityLevel,
    max: QualityLevel,
    last_down: Option<Instant>,
    /// When the current run of the same backlog level started.
    since: Option<(Backlog, Instant)>,
}

impl QualityPolicy {
    /// A policy starting at `level`, with no cap.
    pub fn new(level: QualityLevel) -> Self {
        Self { level, max: QualityLevel::Best, last_down: None, since: None }
    }

    /// The level in use.
    pub fn level(&self) -> QualityLevel {
        self.level
    }

    /// The client's cap.
    pub fn max(&self) -> QualityLevel {
        self.max
    }

    /// Sets the client's cap; returns the new level if it had to drop to it.
    pub fn set_max(&mut self, max: QualityLevel) -> Option<QualityLevel> {
        self.max = max;
        (self.level < max).then(|| {
            self.level = max;
            self.level
        })
    }

    /// Folds in the backlog at `now`; returns the new level when it changed.
    pub fn update(&mut self, backlog: Backlog, now: Instant) -> Option<QualityLevel> {
        let started = match self.since {
            Some((b, t)) if b == backlog => t,
            _ => {
                self.since = Some((backlog, now));
                now
            }
        };
        let held = now.saturating_duration_since(started);
        let may_step_down = self.last_down.is_none_or(|t| now.saturating_duration_since(t) >= DOWN_HOLD);
        let next = match backlog {
            Backlog::Backlogged if may_step_down => self.level.lower(),
            Backlog::Building if held >= BUILDING_FOR && may_step_down => self.level.lower(),
            Backlog::Clear if held >= CLEAR_FOR => self.level.higher().max(self.max),
            _ => self.level,
        };
        if next == self.level {
            return None;
        }
        if next > self.level {
            self.last_down = Some(now);
        }
        // A step starts a new run: the next step waits its full time again.
        self.since = Some((backlog, now));
        self.level = next;
        Some(next)
    }
}
