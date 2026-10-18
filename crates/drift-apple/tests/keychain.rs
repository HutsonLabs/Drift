//! M3-2: Keychain adapter against a real (throw-away) keychain file, under a unique test-only
//! service name (never Drift's real service, never the user's login keychain). Every test
//! cleans up after itself.
//!
//! Keychain *files* are a macOS feature; iOS only has the data-protection keychain, which an
//! unsigned test binary cannot reach, so these tests run on macOS.
#![cfg(target_os = "macos")]
#![allow(missing_docs, clippy::unwrap_used)]

use std::path::PathBuf;

use drift_apple::keychain::Keychain;
use drift_core::SecretRole;
use uuid::Uuid;

/// Password of the temporary test keychain file (not a credential).
const KEYCHAIN_PASSWORD: &str = "drift-test-keychain";

/// A fresh keychain file and a unique service per test.
struct TestKeychain(Keychain, Vec<Uuid>, tempfile::TempDir);

impl TestKeychain {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let service = format!("com.hutsonlabs.drift.tests.{}", Uuid::new_v4());
        let kc =
            Keychain::create_file(&dir.path().join("test.keychain-db"), KEYCHAIN_PASSWORD, service).unwrap();
        Self(kc, Vec::new(), dir)
    }

    fn path(&self) -> PathBuf {
        self.2.path().join("test.keychain-db")
    }

    fn profile(&mut self) -> Uuid {
        let id = Uuid::new_v4();
        self.1.push(id);
        id
    }
}

impl Drop for TestKeychain {
    fn drop(&mut self) {
        for id in &self.1 {
            let _ = self.0.delete_profile(*id);
        }
    }
}

// Obviously fake test values (never real credentials).
const FAKE: &str = "fake-password-🔑-ü";
const FAKE2: &str = "another fake value";

#[test]
fn test_service_is_not_drifts_real_service() {
    let kc = TestKeychain::new();
    assert_ne!(kc.0.service(), Keychain::SERVICE);
    assert_eq!(Keychain::new().service(), "com.hutsonlabs.drift");
}

#[test]
fn set_get_round_trip() {
    let mut kc = TestKeychain::new();
    let p = kc.profile();
    kc.0.set(p, SecretRole::RdpUser, FAKE).unwrap();
    assert_eq!(kc.0.get(p, SecretRole::RdpUser).unwrap().as_deref().map(String::as_str), Some(FAKE));
}

#[test]
fn missing_item_is_none() {
    let mut kc = TestKeychain::new();
    let p = kc.profile();
    assert_eq!(kc.0.get(p, SecretRole::RdpSystem).unwrap(), None);
}

#[test]
fn set_replaces_an_existing_item() {
    let mut kc = TestKeychain::new();
    let p = kc.profile();
    kc.0.set(p, SecretRole::RdpSystem, FAKE).unwrap();
    kc.0.set(p, SecretRole::RdpSystem, FAKE2).unwrap();
    assert_eq!(kc.0.get(p, SecretRole::RdpSystem).unwrap().as_deref().map(String::as_str), Some(FAKE2));
}

#[test]
fn roles_and_profiles_are_separate_items() {
    let mut kc = TestKeychain::new();
    let (a, b) = (kc.profile(), kc.profile());
    kc.0.set(a, SecretRole::RdpSystem, "a-system").unwrap();
    kc.0.set(a, SecretRole::LinuxLogin, "a-linux").unwrap();
    kc.0.set(b, SecretRole::RdpSystem, "b-system").unwrap();
    assert_eq!(kc.0.get(a, SecretRole::RdpSystem).unwrap().as_deref().map(String::as_str), Some("a-system"));
    assert_eq!(kc.0.get(a, SecretRole::LinuxLogin).unwrap().as_deref().map(String::as_str), Some("a-linux"));
    assert_eq!(kc.0.get(b, SecretRole::RdpSystem).unwrap().as_deref().map(String::as_str), Some("b-system"));
    assert_eq!(kc.0.get(b, SecretRole::LinuxLogin).unwrap(), None);
}

#[test]
fn delete_removes_only_that_role_and_is_idempotent() {
    let mut kc = TestKeychain::new();
    let p = kc.profile();
    kc.0.set(p, SecretRole::RdpSystem, FAKE).unwrap();
    kc.0.set(p, SecretRole::LinuxLogin, FAKE2).unwrap();
    kc.0.delete(p, SecretRole::LinuxLogin).unwrap();
    kc.0.delete(p, SecretRole::LinuxLogin).unwrap();
    assert_eq!(kc.0.get(p, SecretRole::LinuxLogin).unwrap(), None);
    assert!(kc.0.get(p, SecretRole::RdpSystem).unwrap().is_some());
}

#[test]
fn delete_profile_removes_every_role() {
    let mut kc = TestKeychain::new();
    let p = kc.profile();
    for role in [SecretRole::RdpSystem, SecretRole::RdpUser, SecretRole::LinuxLogin] {
        kc.0.set(p, role, FAKE).unwrap();
    }
    kc.0.delete_profile(p).unwrap();
    for role in [SecretRole::RdpSystem, SecretRole::RdpUser, SecretRole::LinuxLogin] {
        assert_eq!(kc.0.get(p, role).unwrap(), None, "{role:?}");
    }
}

#[test]
fn services_are_isolated() {
    let mut a = TestKeychain::new();
    // A second service in the same keychain file.
    let b = Keychain::open_file(&a.path(), KEYCHAIN_PASSWORD, format!("{}.other", a.0.service()));
    let p = a.profile();
    a.0.set(p, SecretRole::RdpUser, FAKE).unwrap();
    assert_eq!(b.get(p, SecretRole::RdpUser).unwrap(), None);
    b.set(p, SecretRole::RdpUser, FAKE2).unwrap();
    assert_eq!(a.0.get(p, SecretRole::RdpUser).unwrap().as_deref().map(String::as_str), Some(FAKE));
}

#[test]
fn a_wrong_keychain_password_is_an_error_not_a_panic() {
    let a = TestKeychain::new();
    let wrong = Keychain::open_file(&a.path(), "not-the-password", a.0.service());
    assert!(wrong.get(Uuid::new_v4(), SecretRole::RdpUser).is_err());
}

#[test]
fn debug_output_never_contains_the_keychain_password() {
    let a = TestKeychain::new();
    let dbg = format!("{:?}", a.0);
    assert!(!dbg.contains(KEYCHAIN_PASSWORD), "{dbg}");
    assert!(dbg.contains(a.0.service()));
}

/// `contains` answers "is a password stored?" from the item's *attributes*.
///
/// Reading the data (what `get` does) makes Security.framework decrypt the item and evaluate
/// its ACL; when the running binary is not on that ACL the user gets the "Drift wants to use
/// your confidential information" panel, and the caller blocks until somebody clicks it.
/// `drift-app` asks this question for every saved profile while the first window is being
/// created, so it must never touch the data.
///
/// An item whose stored bytes are not UTF-8 makes the difference observable without an ACL:
/// `get` fails on it, while `contains` — which never looks at the bytes — still says yes.
#[test]
fn contains_reports_the_item_without_reading_its_data() {
    let mut kc = TestKeychain::new();
    let present = kc.profile();
    let absent = kc.profile();
    kc.0.set(present, SecretRole::RdpUser, FAKE).unwrap();

    assert!(kc.0.contains(present, SecretRole::RdpUser).unwrap());
    assert!(!kc.0.contains(present, SecretRole::LinuxLogin).unwrap());
    assert!(!kc.0.contains(absent, SecretRole::RdpUser).unwrap());

    // Store bytes that are not UTF-8 under a third account, behind the adapter's back.
    let undecodable = kc.profile();
    let account = SecretRole::RdpSystem.account(undecodable);
    let mut raw = security_framework::os::macos::keychain::SecKeychain::open(kc.path()).unwrap();
    raw.unlock(Some(KEYCHAIN_PASSWORD)).unwrap();
    raw.set_generic_password(kc.0.service(), &account, &[0xff, 0xfe, 0x00]).unwrap();

    assert!(kc.0.get(undecodable, SecretRole::RdpSystem).is_err(), "reading the data fails");
    assert!(
        kc.0.contains(undecodable, SecretRole::RdpSystem).unwrap(),
        "contains sees the item without decoding it"
    );
}
