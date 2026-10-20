//! M1-6 Red: the `profiles.toml` document model.

use drift_core::{CertFingerprint, ConnectMode, ConnectionProfile, ProfileStore, StoreError};
use uuid::Uuid;

fn profile(n: u128, name: &str, mode: ConnectMode) -> ConnectionProfile {
    let mut p = ConnectionProfile::new(name, "gnome.local", mode);
    p.id = Uuid::from_u128(n);
    p.rdp_username = "user".into();
    p
}

#[test]
fn empty_input_is_an_empty_store() {
    assert_eq!(ProfileStore::from_toml("").unwrap(), ProfileStore::new());
    assert_eq!(ProfileStore::from_toml("  \n").unwrap().profiles().len(), 0);
    assert_eq!(ProfileStore::from_toml("version = 1\n").unwrap().profiles().len(), 0);
}

#[test]
fn round_trips_several_profiles_in_order() {
    let mut s = ProfileStore::new();
    s.upsert(profile(2, "zeta", ConnectMode::Headless)).unwrap();
    s.upsert(profile(1, "Alpha", ConnectMode::RemoteLogin)).unwrap();
    s.upsert(profile(3, "beta", ConnectMode::DesktopSharing)).unwrap();
    let text = s.to_toml().unwrap();
    assert!(text.starts_with("version = 1\n"), "{text}");
    assert!(text.contains("[[profiles]]"), "{text}");
    let back = ProfileStore::from_toml(&text).unwrap();
    assert_eq!(back, s);
    let names: Vec<_> = back.profiles().iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["zeta", "Alpha", "beta"]);
    let sorted: Vec<_> = back.sorted().into_iter().map(|p| p.name).collect();
    assert_eq!(sorted, ["Alpha", "beta", "zeta"]);
}

#[test]
fn upsert_replaces_in_place_and_validates() {
    let mut s = ProfileStore::new();
    assert!(!s.upsert(profile(1, "A", ConnectMode::Headless)).unwrap());
    assert!(!s.upsert(profile(2, "B", ConnectMode::Headless)).unwrap());
    let mut edited = profile(1, "A2", ConnectMode::RemoteLogin);
    edited.linux_username = Some("drifttest".into());
    assert!(s.upsert(edited.clone()).unwrap());
    assert_eq!(s.profiles()[0], edited);
    assert_eq!(s.profiles().len(), 2);

    let mut bad = profile(3, "", ConnectMode::Headless);
    bad.port = 0;
    match s.upsert(bad) {
        Err(StoreError::Invalid(issues)) => assert_eq!(issues.len(), 2),
        other => panic!("{other:?}"),
    }
    assert_eq!(s.profiles().len(), 2);
}

#[test]
fn remove_get_and_pin() {
    let mut s = ProfileStore::new();
    s.upsert(profile(1, "A", ConnectMode::Headless)).unwrap();
    s.upsert(profile(2, "B", ConnectMode::Headless)).unwrap();
    let fp = CertFingerprint::from_bytes([7; 32]);
    s.set_pin(Uuid::from_u128(2), Some(fp)).unwrap();
    assert_eq!(s.get(Uuid::from_u128(2)).unwrap().cert_pin, Some(fp));
    s.set_pin(Uuid::from_u128(2), None).unwrap();
    assert_eq!(s.get(Uuid::from_u128(2)).unwrap().cert_pin, None);
    assert_eq!(s.set_pin(Uuid::from_u128(9), None), Err(StoreError::NotFound(Uuid::from_u128(9))));
    assert_eq!(s.remove(Uuid::from_u128(1)).unwrap().name, "A");
    assert_eq!(s.remove(Uuid::from_u128(1)), Err(StoreError::NotFound(Uuid::from_u128(1))));
    assert!(s.get(Uuid::from_u128(1)).is_none());
}

#[test]
fn structural_errors() {
    assert!(matches!(ProfileStore::from_toml("version = \"x\""), Err(StoreError::Parse(_))));
    assert!(matches!(ProfileStore::from_toml("[[profiles]]\nname = 1"), Err(StoreError::Parse(_))));
    assert_eq!(ProfileStore::from_toml("version = 2\n"), Err(StoreError::UnsupportedVersion { found: 2 }));
    let mut s = ProfileStore::new();
    s.upsert(profile(1, "A", ConnectMode::Headless)).unwrap();
    let one = s.to_toml().unwrap();
    let body = one.trim_start_matches("version = 1\n");
    let dup = format!("{one}{body}");
    assert_eq!(ProfileStore::from_toml(&dup), Err(StoreError::DuplicateId(Uuid::from_u128(1))));
}

#[test]
fn invalid_profiles_on_disk_still_load() {
    // A hand-edited file with a blank name must not lock the user out of their profiles.
    let text = r#"
version = 1

[[profiles]]
id = "00000000-0000-0000-0000-000000000001"
name = ""
host = "gnome.local"
port = 3389
mode = "headless"
rdp_username = "u"
"#;
    let s = ProfileStore::from_toml(text).unwrap();
    assert_eq!(s.profiles().len(), 1);
}

/// GNOME-only: Drift 26.10's profiles of Macs paired with Drift Host are dropped on load, and
/// the next save leaves them out.
#[test]
fn drift_host_profiles_from_26_10_are_dropped() {
    let mut s = ProfileStore::new();
    s.upsert(profile(1, "Linux box", ConnectMode::Headless)).unwrap();
    let gnome = s.to_toml().unwrap();
    let mac = "\n[[profiles]]\nid = \"00000000-0000-0000-0000-000000000002\"\nname = \"Studio\"\n\
               host = \"studio.local\"\nport = 3389\nmode = \"headless\"\nrdp_username = \"me\"\n\
               paired_host = \"00000000-0000-0000-0000-0000000000aa\"\n";
    let back = ProfileStore::from_toml(&format!("{gnome}{mac}")).unwrap();
    assert_eq!(back, s);
    assert!(!back.to_toml().unwrap().contains("paired_host"));
}
