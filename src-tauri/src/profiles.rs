//! Saved profiles: `profiles.toml` in the app config directory plus passwords in a
//! [`SecretStore`] (M1-6 profile store, M3-2 credentials UX).
//!
//! Credential rules (plan M3-2):
//! - **Remote Login** stores the *system* RDP credentials (`rdp-system`). The Linux password is
//!   normally typed by the user at the GDM greeter; only when the user explicitly opts in
//!   (per profile) is it stored (`linux-login`) so Drift can type it into the greeter.
//!   Storing it *is* the opt-in; forgetting it opts out.
//! - **Headless / Desktop Sharing** store one RDP credential set (`rdp-user`) and never a Linux
//!   password.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use drift_core::{ConnectMode, ConnectionProfile, ProfileIssue, ProfileStore, SecretRole, StoreError};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::secrets::SecretStore;

/// File name of the profile document inside the app config directory.
pub const PROFILES_FILE: &str = "profiles.toml";

/// Errors returned to the webview by profile commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, thiserror::Error)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CommandError {
    /// The profile failed validation.
    #[error("the profile is not valid")]
    Invalid {
        /// Per-field problems.
        issues: Vec<ProfileIssue>,
    },
    /// No profile with the given id.
    #[error("profile not found")]
    NotFound,
    /// Reading or writing profiles or passwords failed.
    #[error("{message}")]
    Storage {
        /// Human-readable cause (never contains secrets).
        message: String,
    },
    /// A macOS facility (e.g. opening System Settings) failed.
    #[error("{message}")]
    Platform {
        /// Human-readable cause.
        message: String,
    },
    /// The window has no session (or it has already ended).
    #[error("this window has no active session")]
    NoSession,
    /// The command exists for the UI but its backend is not wired yet.
    #[error("{what} is not available yet")]
    NotImplemented {
        /// What is missing.
        what: String,
    },
}

impl CommandError {
    fn storage(e: impl std::fmt::Display) -> Self {
        Self::Storage { message: e.to_string() }
    }
}

impl From<StoreError> for CommandError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Invalid(issues) => Self::Invalid { issues },
            StoreError::NotFound(_) => Self::NotFound,
            other => Self::storage(other),
        }
    }
}

/// A profile plus which passwords are stored for it (never the passwords themselves).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProfileEntry {
    /// The profile.
    pub profile: ConnectionProfile,
    /// An RDP password is stored for the profile's mode.
    pub has_rdp_password: bool,
    /// Remote Login: a Linux password is stored (the user opted in to greeter typing).
    pub has_linux_password: bool,
}

/// What to do with the Linux greeter password on save.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "action", content = "password", rename_all = "kebab-case")]
pub enum LinuxPasswordUpdate {
    /// Leave as is.
    Keep,
    /// Opt in: store this password and type it into the greeter.
    Store(String),
    /// Opt out: delete the stored password.
    Forget,
}

/// Password changes submitted with a profile. Empty strings mean "unchanged".
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SecretsUpdate {
    /// New RDP password for the profile's mode (`None` = keep the stored one).
    pub rdp_password: Option<String>,
    /// Linux password opt-in (Remote Login only; ignored and forgotten for other modes).
    pub linux_password: LinuxPasswordUpdate,
}

impl std::fmt::Debug for LinuxPasswordUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Keep => "Keep",
            Self::Store(_) => "Store(<redacted>)",
            Self::Forget => "Forget",
        })
    }
}

impl std::fmt::Debug for SecretsUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretsUpdate")
            .field("rdp_password", &self.rdp_password.as_ref().map(|_| "<redacted>"))
            .field("linux_password", &self.linux_password)
            .finish()
    }
}

/// `profiles.toml` on disk.
#[derive(Debug, Clone)]
pub struct ProfileFile {
    path: PathBuf,
}

impl ProfileFile {
    /// The profile file inside `config_dir`.
    pub fn in_dir(config_dir: &Path) -> Self {
        Self { path: config_dir.join(PROFILES_FILE) }
    }

    /// Path of the file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the store; a missing file is an empty store.
    pub fn load(&self) -> Result<ProfileStore, CommandError> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(ProfileStore::from_toml(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ProfileStore::new()),
            Err(e) => Err(CommandError::storage(format!("reading {}: {e}", self.path.display()))),
        }
    }

    /// Writes the store atomically (temp file in the same directory, then rename).
    pub fn save(&self, store: &ProfileStore) -> Result<(), CommandError> {
        let text = store.to_toml()?;
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir)
            .map_err(|e| CommandError::storage(format!("creating {}: {e}", dir.display())))?;
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)
            .map_err(|e| CommandError::storage(format!("writing {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| CommandError::storage(format!("replacing {}: {e}", self.path.display())))
    }
}

/// Profile CRUD shared by the IPC commands (and the SessionManager, which reads profiles and
/// passwords to start sessions and persists TOFU pins).
pub struct ProfileService {
    file: ProfileFile,
    secrets: Arc<dyn SecretStore>,
    store: Mutex<ProfileStore>,
}

impl std::fmt::Debug for ProfileService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileService").field("file", &self.file).finish_non_exhaustive()
    }
}

impl ProfileService {
    /// Loads `file` and uses `secrets` for passwords.
    pub fn open(file: ProfileFile, secrets: Arc<dyn SecretStore>) -> Result<Self, CommandError> {
        let store = file.load()?;
        Ok(Self { file, secrets, store: Mutex::new(store) })
    }

    fn lock(&self) -> Result<MutexGuard<'_, ProfileStore>, CommandError> {
        self.store.lock().map_err(|_| CommandError::storage("profile store lock poisoned"))
    }

    fn entry(&self, profile: ConnectionProfile) -> ProfileEntry {
        let has_rdp_password = self.secrets.has(profile.id, SecretRole::rdp_for(profile.mode));
        let has_linux_password =
            profile.mode == ConnectMode::RemoteLogin && self.secrets.has(profile.id, SecretRole::LinuxLogin);
        ProfileEntry { profile, has_rdp_password, has_linux_password }
    }

    /// All profiles, sorted for display.
    pub fn list(&self) -> Result<Vec<ProfileEntry>, CommandError> {
        let sorted = self.lock()?.sorted();
        Ok(sorted.into_iter().map(|p| self.entry(p)).collect())
    }

    /// All profiles, sorted for display, **without asking the secret store anything**.
    ///
    /// Used by launch-time lookups (`DRIFT_AUTOCONNECT`), which only need names and ids.
    pub fn profiles(&self) -> Result<Vec<ConnectionProfile>, CommandError> {
        Ok(self.lock()?.sorted())
    }

    /// One profile.
    pub fn get(&self, id: Uuid) -> Result<ProfileEntry, CommandError> {
        let p = self.lock()?.get(id).cloned().ok_or(CommandError::NotFound)?;
        Ok(self.entry(p))
    }

    /// Validates and saves `profile` (insert or update) and applies the password changes.
    pub fn save(
        &self,
        profile: ConnectionProfile,
        secrets: SecretsUpdate,
    ) -> Result<ProfileEntry, CommandError> {
        let mut store = self.lock()?;
        let previous = store.get(profile.id).cloned();
        let mut next = store.clone();
        next.upsert(profile.clone())?;
        self.file.save(&next)?;
        *store = next;
        drop(store);
        self.apply_secrets(&profile, previous.as_ref(), secrets)?;
        Ok(self.entry(profile))
    }

    fn apply_secrets(
        &self,
        profile: &ConnectionProfile,
        previous: Option<&ConnectionProfile>,
        update: SecretsUpdate,
    ) -> Result<(), CommandError> {
        let s = &self.secrets;
        let id = profile.id;
        let role = SecretRole::rdp_for(profile.mode);
        let old_role = previous.map(|p| SecretRole::rdp_for(p.mode));
        match update.rdp_password.filter(|p| !p.is_empty()) {
            Some(pw) => s.set(id, role, &pw).map_err(CommandError::storage)?,
            // Mode changed between RemoteLogin and Headless/Sharing: move the stored password.
            None => {
                if let Some(old) = old_role.filter(|r| *r != role)
                    && let Some(pw) = s.get(id, old).map_err(CommandError::storage)?
                {
                    s.set(id, role, &pw).map_err(CommandError::storage)?;
                }
            }
        }
        if let Some(old) = old_role.filter(|r| *r != role) {
            s.delete(id, old).map_err(CommandError::storage)?;
        }
        if profile.mode != ConnectMode::RemoteLogin {
            return s.delete(id, SecretRole::LinuxLogin).map_err(CommandError::storage);
        }
        match update.linux_password {
            LinuxPasswordUpdate::Keep => Ok(()),
            LinuxPasswordUpdate::Store(pw) if pw.is_empty() => Ok(()),
            LinuxPasswordUpdate::Store(pw) => {
                s.set(id, SecretRole::LinuxLogin, &pw).map_err(CommandError::storage)
            }
            LinuxPasswordUpdate::Forget => {
                s.delete(id, SecretRole::LinuxLogin).map_err(CommandError::storage)
            }
        }
    }

    /// Deletes a profile and all its passwords.
    pub fn delete(&self, id: Uuid) -> Result<(), CommandError> {
        let mut store = self.lock()?;
        let mut next = store.clone();
        next.remove(id)?;
        self.file.save(&next)?;
        *store = next;
        drop(store);
        for role in [SecretRole::RdpSystem, SecretRole::RdpUser, SecretRole::LinuxLogin] {
            self.secrets.delete(id, role).map_err(CommandError::storage)?;
        }
        Ok(())
    }

    /// Copies profile `id` under a new id, named "<name> copy", with its stored passwords and
    /// without its certificate pin (the copy's trust is decided again).
    pub fn duplicate(&self, id: Uuid) -> Result<ProfileEntry, CommandError> {
        let mut copy = self.lock()?.get(id).cloned().ok_or(CommandError::NotFound)?;
        copy.id = Uuid::new_v4();
        copy.name = format!("{} copy", copy.name.trim());
        copy.cert_pin = None;
        let s = &self.secrets;
        let rdp = s.get(id, SecretRole::rdp_for(copy.mode)).map_err(CommandError::storage)?;
        let linux = match copy.mode {
            ConnectMode::RemoteLogin => s.get(id, SecretRole::LinuxLogin).map_err(CommandError::storage)?,
            _ => None,
        };
        let update = SecretsUpdate {
            rdp_password: rdp.map(|pw| pw.to_string()),
            linux_password: linux
                .map_or(LinuxPasswordUpdate::Keep, |pw| LinuxPasswordUpdate::Store(pw.to_string())),
        };
        self.save(copy, update)
    }

    /// Persists (or clears) a TOFU pin (the SessionManager calls this on `CertificatePinned`).
    pub fn set_pin(&self, id: Uuid, pin: Option<drift_core::CertFingerprint>) -> Result<(), CommandError> {
        let mut store = self.lock()?;
        let mut next = store.clone();
        next.set_pin(id, pin)?;
        self.file.save(&next)?;
        *store = next;
        Ok(())
    }

    /// The long-lived secrets for a session of profile `id` (M1-6/M3-2 wiring): the RDP
    /// password of the profile's mode (empty if none is stored, so the server reports the
    /// authentication failure) and, for Remote Login with the opt-in, the Linux password.
    pub fn session_secrets(&self, id: Uuid) -> Result<drift_rdp::SessionSecrets, CommandError> {
        let profile = self.lock()?.get(id).cloned().ok_or(CommandError::NotFound)?;
        let rdp = self
            .secrets
            .get(id, SecretRole::rdp_for(profile.mode))
            .map_err(CommandError::storage)?
            .unwrap_or_default();
        let linux = if profile.mode == ConnectMode::RemoteLogin {
            self.secrets.get(id, SecretRole::LinuxLogin).map_err(CommandError::storage)?
        } else {
            None
        };
        let mut secrets = drift_rdp::SessionSecrets::new(rdp.to_string());
        secrets.linux_password = linux;
        Ok(secrets)
    }
}
