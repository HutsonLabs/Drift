//! IOS-3: the pure path-status table and the merged trigger feed moved to drift-apple, so the
//! macOS and iOS platform layers share them.
#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use drift_apple::network::{PathMonitor, PathStatus, TriggerFeed};
use drift_core::{Trigger, TriggerAction};
use drift_testkit::ManualClock;

#[test]
fn raw_path_status_values_and_triggers() {
    assert_eq!(PathStatus::from_raw(1), PathStatus::Satisfied);
    assert_eq!(PathStatus::from_raw(2), PathStatus::Unsatisfied);
    assert_eq!(PathStatus::from_raw(3), PathStatus::Satisfiable);
    assert_eq!(PathStatus::from_raw(9), PathStatus::Invalid);
    assert_eq!(PathStatus::Satisfiable.trigger(), Some(Trigger::NetworkOnline));
    assert_eq!(PathStatus::Unsatisfied.trigger(), Some(Trigger::NetworkOffline));
    assert_eq!(PathStatus::Invalid.trigger(), None);
}

#[test]
fn feed_forwards_merged_actions() {
    let clock = Arc::new(ManualClock::new());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let feed = TriggerFeed::new(true, clock.clone(), move |a: TriggerAction| s.lock().unwrap().push(a));
    feed.path_status(PathStatus::Unsatisfied);
    assert!(!feed.is_online());
    clock.advance(Duration::from_secs(10));
    feed.path_status(PathStatus::Satisfied);
    assert!(feed.is_online());
    assert_eq!(*seen.lock().unwrap(), vec![TriggerAction::PauseReconnect, TriggerAction::RetryNow]);
}

#[test]
fn path_monitor_reports_an_initial_status() {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let monitor = PathMonitor::start(move |status| {
        let _ = tx.lock().unwrap().send(status);
    })
    .expect("Network.framework creates a monitor");
    let status = rx.recv_timeout(Duration::from_secs(5)).expect("initial status");
    assert_ne!(status, PathStatus::Invalid);
    drop(monitor);
}
