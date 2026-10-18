//! EXT-4 Red: the host steps its quality down when the link backs up and back up when it clears.
#![allow(missing_docs, clippy::unwrap_used)]

use std::time::{Duration, Instant};

use drift_ext::status::QualityLevel;
use drift_host::link::Backlog;
use drift_host::quality::{BUILDING_FOR, CLEAR_FOR, DOWN_HOLD, QualityPolicy, settings_for};

#[test]
fn backlogged_steps_down_at_once_but_not_twice_within_the_hold() {
    let t0 = Instant::now();
    let mut p = QualityPolicy::new(QualityLevel::Best);
    assert_eq!(p.update(Backlog::Backlogged, t0), Some(QualityLevel::High));
    assert_eq!(p.update(Backlog::Backlogged, t0 + DOWN_HOLD / 2), None, "give the lower rate a moment");
    assert_eq!(p.update(Backlog::Backlogged, t0 + DOWN_HOLD), Some(QualityLevel::Medium));
    assert_eq!(p.level(), QualityLevel::Medium);
}

#[test]
fn building_for_a_while_steps_down_and_clear_for_long_steps_up() {
    let t0 = Instant::now();
    let mut p = QualityPolicy::new(QualityLevel::Best);
    assert_eq!(p.update(Backlog::Building, t0), None);
    assert_eq!(p.update(Backlog::Building, t0 + BUILDING_FOR), Some(QualityLevel::High));
    let t1 = t0 + BUILDING_FOR;
    assert_eq!(p.update(Backlog::Clear, t1), None);
    assert_eq!(p.update(Backlog::Clear, t1 + CLEAR_FOR / 2), None);
    assert_eq!(p.update(Backlog::Clear, t1 + CLEAR_FOR), Some(QualityLevel::Best));
    assert_eq!(p.update(Backlog::Clear, t1 + CLEAR_FOR * 3), None, "never above the best");
}

#[test]
fn the_clients_cap_applies_at_once_and_bounds_stepping_up() {
    let t0 = Instant::now();
    let mut p = QualityPolicy::new(QualityLevel::Best);
    assert_eq!(p.set_max(QualityLevel::Medium), Some(QualityLevel::Medium));
    assert_eq!(p.update(Backlog::Clear, t0 + CLEAR_FOR * 2), None, "the cap holds");
    assert_eq!(p.update(Backlog::Backlogged, t0 + CLEAR_FOR * 2), Some(QualityLevel::Low));
    assert_eq!(p.update(Backlog::Clear, t0 + CLEAR_FOR * 2 + Duration::from_millis(1)), None);
    assert_eq!(p.update(Backlog::Clear, t0 + CLEAR_FOR * 4), Some(QualityLevel::Medium));
}

#[test]
fn each_level_lowers_the_bit_rate_and_the_lower_ones_halve_the_frame_rate() {
    let base = 20_000_000;
    let best = settings_for(QualityLevel::Best, base, 60);
    let high = settings_for(QualityLevel::High, base, 60);
    let medium = settings_for(QualityLevel::Medium, base, 60);
    let low = settings_for(QualityLevel::Low, base, 60);
    assert_eq!((best.bitrate, best.fps), (base, 60));
    assert!(best.bitrate > high.bitrate && high.bitrate > medium.bitrate && medium.bitrate > low.bitrate);
    assert_eq!((high.fps, medium.fps, low.fps), (60, 30, 30));
    assert!(low.bitrate >= 1_000_000, "never starved below 1 Mbit/s");
}
