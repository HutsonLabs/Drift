//! IOS-3: on iOS the reconnect "wake" trigger is the app returning to the foreground
//! (`UIApplicationWillEnterForegroundNotification`); there is no `NSWorkspace` wake.
//!
//! Runs in the iOS simulator only (`xcrun simctl spawn <device> <test binary>`); the merge gate
//! links it for `aarch64-apple-ios-sim`.
#![cfg(target_os = "ios")]
#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use drift_ios::lifecycle::{ForegroundObserver, post_will_enter_foreground_for_tests};

#[test]
fn foreground_observer_fires_until_dropped() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let observer = ForegroundObserver::start(move || {
        c.fetch_add(1, Ordering::SeqCst);
    });
    post_will_enter_foreground_for_tests();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    drop(observer);
    post_will_enter_foreground_for_tests();
    assert_eq!(count.load(Ordering::SeqCst), 1, "no callbacks after drop");
}

#[test]
fn app_settings_url_is_uikits_constant() {
    assert_eq!(drift_ios::settings::app_settings_url(), "app-settings:");
}
