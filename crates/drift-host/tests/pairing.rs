//! HOST-8 Red: the host's pairing window and its list of paired devices (ADR `HOST-8-pairing`).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use drift_core::pairing::ShortCode;
use drift_ext::pairing::PairStatus;
use drift_host::pairing::{CODE_LIFETIME, DeviceStore, LOCKOUT, MAX_WRONG_CODES, Pairing};
use drift_testkit::ManualClock;

fn code(body: u32) -> ShortCode {
    ShortCode::from_body(body).unwrap()
}

fn key(n: u8) -> Vec<u8> {
    let mut k = vec![4_u8];
    k.extend([n; 64]);
    k
}

#[test]
fn a_window_accepts_its_code_once_and_stores_the_device() {
    let clock = Arc::new(ManualClock::new());
    let pairing = Pairing::new(DeviceStore::memory(), clock.clone());
    assert_eq!(pairing.try_pair("12345674", "iPad", &key(1)), PairStatus::NotPairing, "no window open");

    let shown = pairing.open_window_with(code(1_234_567));
    assert_eq!(pairing.code(), Some(shown.clone()));
    assert_eq!(pairing.try_pair(shown.as_str(), "iPad", &key(1)), PairStatus::Accepted);
    assert_eq!(pairing.code(), None, "a code pairs one device");
    let devices = pairing.devices();
    assert_eq!(devices.len(), 1);
    assert_eq!((devices[0].name.as_str(), devices[0].public_key()), ("iPad", key(1)));
    assert!(pairing.find_device(&key(1)).is_some());
    assert!(pairing.find_device(&key(2)).is_none());
}

#[test]
fn codes_expire_and_wrong_codes_lock_pairing_for_a_while() {
    let clock = Arc::new(ManualClock::new());
    let pairing = Pairing::new(DeviceStore::memory(), clock.clone());
    let shown = pairing.open_window_with(code(7_654_321));
    clock.advance(CODE_LIFETIME + Duration::from_secs(1));
    assert_eq!(pairing.try_pair(shown.as_str(), "iPad", &key(1)), PairStatus::Expired);

    let shown = pairing.open_window_with(code(7_654_321));
    let wrong = code(1_111_111);
    for _ in 1..MAX_WRONG_CODES {
        assert_eq!(pairing.try_pair(wrong.as_str(), "x", &key(3)), PairStatus::WrongCode);
    }
    assert_eq!(pairing.try_pair(wrong.as_str(), "x", &key(3)), PairStatus::TooManyAttempts);
    // Even the right code is refused during the lockout, and the window is gone after it.
    assert_eq!(pairing.try_pair(shown.as_str(), "x", &key(3)), PairStatus::TooManyAttempts);
    clock.advance(LOCKOUT + Duration::from_secs(1));
    assert_eq!(pairing.try_pair(shown.as_str(), "x", &key(3)), PairStatus::NotPairing);
    assert!(pairing.devices().is_empty());
    assert_eq!(pairing.wrong_codes(), u64::from(MAX_WRONG_CODES));
}

#[test]
fn opened_codes_are_valid_and_differ() {
    let pairing = Pairing::new(DeviceStore::memory(), Arc::new(ManualClock::new()));
    let a = pairing.open_window();
    let b = pairing.open_window();
    assert!(ShortCode::parse(a.as_str()).is_ok() && ShortCode::parse(b.as_str()).is_ok());
    assert_ne!(a, b, "two random 7-digit bodies collide with probability 1e-7");
    assert_eq!(pairing.code(), Some(b), "a new window replaces the old code");
}

#[test]
fn the_device_list_persists_and_devices_can_be_removed() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new());
    let pairing = Pairing::new(DeviceStore::load(dir.path()).unwrap(), clock.clone());
    let shown = pairing.open_window_with(code(2_222_222));
    assert_eq!(pairing.try_pair(shown.as_str(), "Andrew’s iPhone", &key(5)), PairStatus::Accepted);

    let reopened = Pairing::new(DeviceStore::load(dir.path()).unwrap(), clock);
    let devices = reopened.devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Andrew’s iPhone");
    assert!(reopened.remove_device(devices[0].id));
    assert!(reopened.devices().is_empty());
    assert!(DeviceStore::load(dir.path()).unwrap().devices().is_empty(), "removal is saved");
}
