//! IOS-9 Red: the device key and the pinned host keys live in the Keychain (ADR IOS-9), next
//! to the passwords but never clashing with them. Throw-away keychain files, as in
//! `keychain.rs`; macOS only for the same reason.
#![cfg(target_os = "macos")]
#![allow(missing_docs, clippy::unwrap_used)]

use drift_apple::keychain::Keychain;
use drift_apple::pairing::{DEVICE_KEY_ACCOUNT, PairingKeychain, host_pin_account};
use drift_core::{CertFingerprint, SecretRole};
use uuid::Uuid;

const KEYCHAIN_PASSWORD: &str = "drift-test-keychain";

fn keychain() -> (Keychain, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let service = format!("com.hutsonlabs.drift.tests.{}", Uuid::new_v4());
    let kc = Keychain::create_file(&dir.path().join("test.keychain-db"), KEYCHAIN_PASSWORD, service).unwrap();
    (kc, dir)
}

#[test]
fn raw_items_round_trip_and_delete() {
    let (kc, _dir) = keychain();
    assert_eq!(kc.get_item("pairing/test").unwrap(), None);
    kc.set_item("pairing/test", &[0, 1, 2, 255]).unwrap();
    assert_eq!(kc.get_item("pairing/test").unwrap().as_deref().map(Vec::as_slice), Some(&[0, 1, 2, 255][..]));
    kc.set_item("pairing/test", &[9]).unwrap();
    assert_eq!(kc.get_item("pairing/test").unwrap().as_deref().map(Vec::as_slice), Some(&[9][..]));
    kc.delete_item("pairing/test").unwrap();
    kc.delete_item("pairing/test").unwrap();
    assert_eq!(kc.get_item("pairing/test").unwrap(), None);
}

#[test]
fn the_device_key_is_created_once_and_kept() {
    let (kc, _dir) = keychain();
    let store = PairingKeychain::new(kc.clone());
    let first = store.device_key().unwrap();
    assert_eq!(first.public_key().len(), 65, "uncompressed P-256 point");
    assert_eq!(first.public_key()[0], 0x04);
    let again = PairingKeychain::new(kc.clone()).device_key().unwrap();
    assert_eq!(again.public_key(), first.public_key());
    assert!(kc.get_item(DEVICE_KEY_ACCOUNT).unwrap().is_some());
}

#[test]
fn the_device_key_signs_what_its_public_key_verifies() {
    let (kc, _dir) = keychain();
    let key = PairingKeychain::new(kc).device_key().unwrap();
    let signature = key.sign(b"pairing transcript").unwrap();
    assert!(drift_apple::pairing::verify(key.public_key(), b"pairing transcript", &signature));
    assert!(!drift_apple::pairing::verify(key.public_key(), b"something else", &signature));
}

#[test]
fn host_pins_round_trip_and_are_forgotten() {
    let (kc, _dir) = keychain();
    let store = PairingKeychain::new(kc);
    let host = Uuid::new_v4();
    let fp = CertFingerprint::from_bytes([7; 32]);
    assert_eq!(store.host_pin(host).unwrap(), None);
    store.pin_host(host, fp).unwrap();
    assert_eq!(store.host_pin(host).unwrap(), Some(fp));
    store.forget_host(host).unwrap();
    assert_eq!(store.host_pin(host).unwrap(), None);
}

#[test]
fn pairing_accounts_never_collide_with_password_accounts() {
    let id = Uuid::new_v4();
    for role in [SecretRole::RdpSystem, SecretRole::RdpUser, SecretRole::LinuxLogin] {
        assert_ne!(role.account(id), host_pin_account(id));
        assert_ne!(role.account(id), DEVICE_KEY_ACCOUNT);
    }
    assert!(host_pin_account(id).starts_with("pairing/"));
}
