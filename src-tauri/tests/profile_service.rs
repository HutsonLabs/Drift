//! M1-6 / M3-2 Red: profile store on disk and the credential rules per mode.
#![allow(clippy::unwrap_used)] // fixture helpers outside #[test] fns

use std::sync::Arc;

use drift_app::profiles::{CommandError, LinuxPasswordUpdate, ProfileFile, ProfileService, SecretsUpdate};
use drift_app::secrets::{MemorySecretStore, SecretStore};
use drift_core::{CertFingerprint, ConnectMode, ConnectionProfile, ProfileField, SecretRole};

struct Fixture {
    _dir: tempfile::TempDir,
    file: ProfileFile,
    secrets: Arc<MemorySecretStore>,
    service: ProfileService,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let file = ProfileFile::in_dir(&dir.path().join("config"));
    let secrets = Arc::new(MemorySecretStore::new());
    let service = ProfileService::open(file.clone(), secrets.clone()).unwrap();
    Fixture { _dir: dir, file, secrets, service }
}

fn profile(mode: ConnectMode) -> ConnectionProfile {
    let mut p = ConnectionProfile::new("Homelab", "gnome.local", mode);
    p.rdp_username = "rdp-user".into();
    p
}

fn update(rdp: Option<&str>, linux: LinuxPasswordUpdate) -> SecretsUpdate {
    SecretsUpdate { rdp_password: rdp.map(Into::into), linux_password: linux }
}

fn secret(f: &Fixture, p: &ConnectionProfile, role: SecretRole) -> Option<String> {
    f.secrets.get(p.id, role).unwrap().map(|s| s.to_string())
}

#[test]
fn missing_file_is_empty_and_save_persists_to_disk() {
    let f = fixture();
    assert!(f.service.list().unwrap().is_empty());
    let p = profile(ConnectMode::Headless);
    let entry = f.service.save(p.clone(), update(Some("pw-Fake1"), LinuxPasswordUpdate::Keep)).unwrap();
    assert_eq!(entry.profile, p);
    assert!(entry.has_rdp_password);
    assert!(!entry.has_linux_password);

    let text = std::fs::read_to_string(f.file.path()).unwrap();
    assert!(text.contains("name = \"Homelab\""), "{text}");
    assert!(!text.contains("pw-Fake1"), "passwords never go to profiles.toml");

    let reopened = ProfileService::open(f.file.clone(), f.secrets.clone()).unwrap();
    assert_eq!(reopened.list().unwrap()[0].profile, p);
}

#[test]
fn invalid_profile_is_rejected_with_issues_and_not_written() {
    let f = fixture();
    let mut p = profile(ConnectMode::Headless);
    p.host = "gnome.local:3389".into();
    match f.service.save(p, update(Some("x"), LinuxPasswordUpdate::Keep)) {
        Err(CommandError::Invalid { issues }) => assert_eq!(issues[0].field, ProfileField::Host),
        other => panic!("{other:?}"),
    }
    assert!(!f.file.path().exists());
    assert!(f.service.list().unwrap().is_empty());
}

#[test]
fn remote_login_stores_system_rdp_credentials_and_opt_in_linux_password() {
    let f = fixture();
    let mut p = profile(ConnectMode::RemoteLogin);
    p.linux_username = Some("drifttest".into());

    // Without opt-in: only the system RDP password.
    let e = f.service.save(p.clone(), update(Some("sys-Fake1"), LinuxPasswordUpdate::Keep)).unwrap();
    assert!(e.has_rdp_password && !e.has_linux_password);
    assert_eq!(secret(&f, &p, SecretRole::RdpSystem).as_deref(), Some("sys-Fake1"));
    assert_eq!(secret(&f, &p, SecretRole::RdpUser), None);
    assert_eq!(secret(&f, &p, SecretRole::LinuxLogin), None);

    // Opt in.
    let e = f.service.save(p.clone(), update(None, LinuxPasswordUpdate::Store("lx-Fake1".into()))).unwrap();
    assert!(e.has_rdp_password && e.has_linux_password);
    assert_eq!(
        secret(&f, &p, SecretRole::RdpSystem).as_deref(),
        Some("sys-Fake1"),
        "None keeps the RDP password"
    );
    assert_eq!(secret(&f, &p, SecretRole::LinuxLogin).as_deref(), Some("lx-Fake1"));

    // Empty strings mean "unchanged".
    let e = f.service.save(p.clone(), update(Some(""), LinuxPasswordUpdate::Store(String::new()))).unwrap();
    assert!(e.has_rdp_password && e.has_linux_password);

    // Opt out.
    let e = f.service.save(p.clone(), update(None, LinuxPasswordUpdate::Forget)).unwrap();
    assert!(!e.has_linux_password);
    assert_eq!(secret(&f, &p, SecretRole::LinuxLogin), None);
}

#[test]
fn headless_and_sharing_store_one_rdp_credential_and_never_a_linux_password() {
    for mode in [ConnectMode::Headless, ConnectMode::DesktopSharing] {
        let f = fixture();
        let p = profile(mode);
        let e = f
            .service
            .save(p.clone(), update(Some("d-Fake1"), LinuxPasswordUpdate::Store("lx".into())))
            .unwrap();
        assert!(e.has_rdp_password && !e.has_linux_password, "{mode:?}");
        assert_eq!(secret(&f, &p, SecretRole::RdpUser).as_deref(), Some("d-Fake1"));
        assert_eq!(secret(&f, &p, SecretRole::RdpSystem), None);
        assert_eq!(secret(&f, &p, SecretRole::LinuxLogin), None, "{mode:?}");
    }
}

#[test]
fn switching_mode_moves_the_rdp_password_and_drops_the_linux_password() {
    let f = fixture();
    let mut p = profile(ConnectMode::RemoteLogin);
    f.service
        .save(p.clone(), update(Some("sys-Fake1"), LinuxPasswordUpdate::Store("lx-Fake1".into())))
        .unwrap();

    p.mode = ConnectMode::Headless;
    let e = f.service.save(p.clone(), update(None, LinuxPasswordUpdate::Keep)).unwrap();
    assert!(e.has_rdp_password && !e.has_linux_password);
    assert_eq!(secret(&f, &p, SecretRole::RdpUser).as_deref(), Some("sys-Fake1"));
    assert_eq!(secret(&f, &p, SecretRole::RdpSystem), None);
    assert_eq!(secret(&f, &p, SecretRole::LinuxLogin), None);

    // Switching with a new password stores the new one under the new role.
    p.mode = ConnectMode::RemoteLogin;
    f.service.save(p.clone(), update(Some("new-Fake2"), LinuxPasswordUpdate::Keep)).unwrap();
    assert_eq!(secret(&f, &p, SecretRole::RdpSystem).as_deref(), Some("new-Fake2"));
    assert_eq!(secret(&f, &p, SecretRole::RdpUser), None);
}

#[test]
fn delete_removes_profile_and_all_passwords() {
    let f = fixture();
    let p = profile(ConnectMode::RemoteLogin);
    f.service.save(p.clone(), update(Some("a"), LinuxPasswordUpdate::Store("b".into()))).unwrap();
    f.service.delete(p.id).unwrap();
    assert!(f.service.list().unwrap().is_empty());
    for role in [SecretRole::RdpSystem, SecretRole::RdpUser, SecretRole::LinuxLogin] {
        assert_eq!(secret(&f, &p, role), None);
    }
    assert_eq!(f.service.delete(p.id), Err(CommandError::NotFound));
    let reopened = ProfileService::open(f.file.clone(), f.secrets.clone()).unwrap();
    assert!(reopened.list().unwrap().is_empty());
}

/// UI-windows contract: `duplicate_profile` copies a profile and its stored passwords under a
/// new id, named "<name> copy", without the certificate pin.
#[test]
fn duplicate_copies_the_profile_and_its_passwords_but_not_the_pin() {
    let f = fixture();
    let mut p = profile(ConnectMode::RemoteLogin);
    p.linux_username = Some("drifttest".into());
    f.service
        .save(p.clone(), update(Some("sys-Fake1"), LinuxPasswordUpdate::Store("lx-Fake1".into())))
        .unwrap();
    f.service.set_pin(p.id, Some(CertFingerprint::from_bytes([7; 32]))).unwrap();

    let copy = f.service.duplicate(p.id).unwrap();
    assert_ne!(copy.profile.id, p.id);
    assert_eq!(copy.profile.name, "Homelab copy");
    assert_eq!(copy.profile.cert_pin, None, "trust is decided again for the copy");
    assert_eq!((copy.profile.host.as_str(), copy.profile.mode), ("gnome.local", ConnectMode::RemoteLogin));
    assert_eq!(copy.profile.linux_username.as_deref(), Some("drifttest"));
    assert!(copy.has_rdp_password && copy.has_linux_password);
    assert_eq!(secret(&f, &copy.profile, SecretRole::RdpSystem).as_deref(), Some("sys-Fake1"));
    assert_eq!(secret(&f, &copy.profile, SecretRole::LinuxLogin).as_deref(), Some("lx-Fake1"));
    assert_eq!(f.service.list().unwrap().len(), 2);
    let reopened = ProfileService::open(f.file.clone(), f.secrets.clone()).unwrap();
    assert_eq!(reopened.get(copy.profile.id).unwrap().profile, copy.profile, "saved to disk");
    assert_eq!(f.service.get(p.id).unwrap().profile.cert_pin, Some(CertFingerprint::from_bytes([7; 32])));
    assert_eq!(f.service.duplicate(uuid::Uuid::nil()), Err(CommandError::NotFound));
}

#[test]
fn pins_are_persisted_and_cleared() {
    let f = fixture();
    let p = profile(ConnectMode::Headless);
    f.service.save(p.clone(), update(Some("a"), LinuxPasswordUpdate::Keep)).unwrap();
    let fp = CertFingerprint::from_bytes([0xab; 32]);
    f.service.set_pin(p.id, Some(fp)).unwrap();
    let reopened = ProfileService::open(f.file.clone(), f.secrets.clone()).unwrap();
    assert_eq!(reopened.get(p.id).unwrap().profile.cert_pin, Some(fp));
    f.service.set_pin(p.id, None).unwrap();
    assert_eq!(f.service.get(p.id).unwrap().profile.cert_pin, None);
    assert_eq!(f.service.set_pin(uuid::Uuid::nil(), None), Err(CommandError::NotFound));
}

#[test]
fn corrupt_file_is_a_storage_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("profiles.toml"), "version = [").unwrap();
    let r = ProfileService::open(ProfileFile::in_dir(dir.path()), Arc::new(MemorySecretStore::new()));
    assert!(matches!(r, Err(CommandError::Storage { .. })), "{r:?}");
}

#[test]
fn secrets_never_appear_in_debug_output() {
    let u = update(Some("hunter2-Fake9"), LinuxPasswordUpdate::Store("other-Fake9".into()));
    let d = format!("{u:?}");
    assert!(!d.contains("hunter2") && !d.contains("other-Fake9"), "{d}");
    let s = MemorySecretStore::new();
    s.set(uuid::Uuid::nil(), SecretRole::RdpUser, "hunter2-Fake9").unwrap();
    assert!(!format!("{s:?}").contains("hunter2"));
}

#[test]
fn session_secrets_follow_the_mode() {
    // M1-6 / M3-2 wiring: the SessionManager's host hands these to `spawn_session`.
    let f = fixture();
    let rl = profile(ConnectMode::RemoteLogin);
    let linux = LinuxPasswordUpdate::Store("lin-Fake2".into());
    f.service.save(rl.clone(), update(Some("sys-Fake1"), linux)).unwrap();
    let s = f.service.session_secrets(rl.id).unwrap();
    assert_eq!(s.rdp_password.as_str(), "sys-Fake1");
    assert_eq!(s.linux_password.as_ref().map(|p| p.as_str()), Some("lin-Fake2"));

    let hl = profile(ConnectMode::Headless);
    f.service.save(hl.clone(), update(Some("hl-Fake3"), LinuxPasswordUpdate::Keep)).unwrap();
    let s = f.service.session_secrets(hl.id).unwrap();
    assert_eq!(s.rdp_password.as_str(), "hl-Fake3");
    assert!(s.linux_password.is_none());

    let none = profile(ConnectMode::DesktopSharing);
    f.service.save(none.clone(), update(None, LinuxPasswordUpdate::Keep)).unwrap();
    assert_eq!(f.service.session_secrets(none.id).unwrap().rdp_password.as_str(), "", "none stored");
    assert_eq!(f.service.session_secrets(uuid::Uuid::new_v4()).unwrap_err(), CommandError::NotFound);
}

/// A store that records every call and refuses to hand out secret *values*.
///
/// On macOS, reading a generic password's data is what makes Security.framework evaluate the
/// item's ACL and, when it does not match the running binary, put up the "Drift wants to use
/// your confidential information" panel. `ProfileService::list` only needs to know *whether* a
/// password exists, so it must never ask for the value.
#[derive(Default)]
struct TrapStore {
    inner: MemorySecretStore,
    gets: std::sync::atomic::AtomicUsize,
}

impl TrapStore {
    fn gets(&self) -> usize {
        self.gets.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl SecretStore for TrapStore {
    fn set(
        &self,
        profile: uuid::Uuid,
        role: SecretRole,
        secret: &str,
    ) -> Result<(), drift_app::secrets::SecretError> {
        self.inner.set(profile, role, secret)
    }

    fn get(
        &self,
        profile: uuid::Uuid,
        role: SecretRole,
    ) -> Result<Option<zeroize::Zeroizing<String>>, drift_app::secrets::SecretError> {
        self.gets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.get(profile, role)
    }

    fn delete(&self, profile: uuid::Uuid, role: SecretRole) -> Result<(), drift_app::secrets::SecretError> {
        self.inner.delete(profile, role)
    }

    fn has(&self, profile: uuid::Uuid, role: SecretRole) -> bool {
        self.inner.has(profile, role)
    }
}

#[test]
fn listing_profiles_never_reads_secret_values() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(TrapStore::default());
    let service = ProfileService::open(ProfileFile::in_dir(dir.path()), store.clone()).unwrap();
    let mut remote = profile(ConnectMode::RemoteLogin);
    remote.linux_username = Some("drifttest".into());
    service.save(remote, update(Some("pw-Fake1"), LinuxPasswordUpdate::Store("pw-Fake2".into()))).unwrap();
    service
        .save(profile(ConnectMode::Headless), update(Some("pw-Fake3"), LinuxPasswordUpdate::Keep))
        .unwrap();
    let saved_gets = store.gets();

    let entries = service.list().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|e| e.has_rdp_password));
    assert!(entries.iter().any(|e| e.has_linux_password));
    assert_eq!(
        store.gets(),
        saved_gets,
        "listing profiles must test for the item, not decrypt it (a Keychain ACL prompt \
         blocks the main thread and the app launches without a window)"
    );
}
