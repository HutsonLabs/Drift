//! M3-2: the app's SecretStore backed by drift-macos's Keychain adapter (throw-away keychain
//! file and test-only service; never the login keychain).
#![allow(missing_docs, clippy::unwrap_used)]

use drift_app::secrets::{KeychainSecretStore, SecretStore};
use drift_core::SecretRole;
use drift_macos::Keychain;
use uuid::Uuid;

#[test]
fn keychain_secret_store_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let service = format!("com.hutsonlabs.drift.tests.{}", Uuid::new_v4());
    let kc =
        Keychain::create_file(&dir.path().join("t.keychain-db"), "drift-test-keychain", service).unwrap();
    let store = KeychainSecretStore::new(kc);
    let p = Uuid::new_v4();
    assert!(!store.has(p, SecretRole::RdpUser));
    store.set(p, SecretRole::RdpUser, "fake-value").unwrap();
    assert!(store.has(p, SecretRole::RdpUser));
    assert_eq!(store.get(p, SecretRole::RdpUser).unwrap().as_deref().map(String::as_str), Some("fake-value"));
    store.delete(p, SecretRole::RdpUser).unwrap();
    assert_eq!(store.get(p, SecretRole::RdpUser).unwrap(), None);
    store.delete(p, SecretRole::RdpUser).unwrap();
}

#[test]
fn keychain_errors_become_secret_errors_without_the_secret() {
    let dir = tempfile::tempdir().unwrap();
    // A keychain file that does not exist: every operation fails cleanly.
    let kc = Keychain::open_file(&dir.path().join("missing.keychain-db"), "x", "com.hutsonlabs.drift.tests");
    let store = KeychainSecretStore::new(kc);
    let err = store.set(Uuid::new_v4(), SecretRole::RdpSystem, "fake-secret-value").unwrap_err();
    assert!(!err.to_string().contains("fake-secret-value"), "{err}");
}
