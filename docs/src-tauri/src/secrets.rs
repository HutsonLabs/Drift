//! Password storage seam (plan §2 decision 7).
//!
//! Profiles never contain secrets; passwords are stored per profile id + [`SecretRole`]. The
//! production implementation is [`KeychainSecretStore`] over the Keychain adapter from
//! `drift-macos` (M3-2); [`MemorySecretStore`] (process lifetime only) serves tests.

use std::collections::HashMap;
use std::sync::Mutex;

use drift_core::SecretRole;
use uuid::Uuid;
use zeroize::Zeroizing;

/// Failure of the secret backend (message is safe to show: it never contains the secret).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("password storage failed: {0}")]
pub struct SecretError(pub String);

/// Stores passwords by profile id and role.
pub trait SecretStore: Send + Sync {
    /// Stores (replaces) a secret.
    fn set(&self, profile: Uuid, role: SecretRole, secret: &str) -> Result<(), SecretError>;
    /// Reads a secret.
    fn get(&self, profile: Uuid, role: SecretRole) -> Result<Option<Zeroizing<String>>, SecretError>;
    /// Deletes a secret; deleting a missing item is not an error.
    fn delete(&self, profile: Uuid, role: SecretRole) -> Result<(), SecretError>;

    /// Whether a secret exists, **without reading its value**.
    ///
    /// There is deliberately no default implementation in terms of [`SecretStore::get`]:
    /// on the Keychain, reading a password decrypts the item and evaluates its ACL, which
    /// can put up an authorization panel and block the calling thread until somebody clicks
    /// it. `ProfileService::list` asks this for every saved profile while the first window
    /// is being built (`drift_app::run_with`), so it must stay a metadata lookup.
    fn has(&self, profile: Uuid, role: SecretRole) -> bool;
}

type SecretMap = HashMap<(Uuid, SecretRole), Zeroizing<String>>;

/// In-memory [`SecretStore`] (tests, and the app until the Keychain adapter lands).
#[derive(Default)]
pub struct MemorySecretStore {
    items: Mutex<SecretMap>,
}

impl MemorySecretStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, SecretMap>, SecretError> {
        self.items.lock().map_err(|_| SecretError("secret store lock poisoned".into()))
    }
}

impl SecretStore for MemorySecretStore {
    fn set(&self, profile: Uuid, role: SecretRole, secret: &str) -> Result<(), SecretError> {
        self.lock()?.insert((profile, role), Zeroizing::new(secret.to_owned()));
        Ok(())
    }

    fn get(&self, profile: Uuid, role: SecretRole) -> Result<Option<Zeroizing<String>>, SecretError> {
        Ok(self.lock()?.get(&(profile, role)).cloned())
    }

    fn delete(&self, profile: Uuid, role: SecretRole) -> Result<(), SecretError> {
        self.lock()?.remove(&(profile, role));
        Ok(())
    }

    fn has(&self, profile: Uuid, role: SecretRole) -> bool {
        self.lock().is_ok_and(|items| items.contains_key(&(profile, role)))
    }
}

/// The production [`SecretStore`]: generic passwords in the Keychain (drift-macos, M3-2).
#[derive(Debug, Clone, Default)]
pub struct KeychainSecretStore {
    keychain: drift_macos::Keychain,
}

impl KeychainSecretStore {
    /// Wraps a Keychain adapter (`drift_macos::Keychain::new()` in the app).
    pub fn new(keychain: drift_macos::Keychain) -> Self {
        Self { keychain }
    }
}

impl From<drift_macos::KeychainError> for SecretError {
    fn from(e: drift_macos::KeychainError) -> Self {
        SecretError(e.to_string())
    }
}

impl SecretStore for KeychainSecretStore {
    fn set(&self, profile: Uuid, role: SecretRole, secret: &str) -> Result<(), SecretError> {
        Ok(self.keychain.set(profile, role, secret)?)
    }

    fn get(&self, profile: Uuid, role: SecretRole) -> Result<Option<Zeroizing<String>>, SecretError> {
        Ok(self.keychain.get(profile, role)?)
    }

    fn delete(&self, profile: Uuid, role: SecretRole) -> Result<(), SecretError> {
        Ok(self.keychain.delete(profile, role)?)
    }

    fn has(&self, profile: Uuid, role: SecretRole) -> bool {
        match self.keychain.contains(profile, role) {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!(error = %e, "could not query the Keychain for a stored password");
                false
            }
        }
    }
}

impl std::fmt::Debug for MemorySecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.items.lock().map(|m| m.len()).unwrap_or(0);
        f.debug_struct("MemorySecretStore").field("items", &n).finish_non_exhaustive()
    }
}
