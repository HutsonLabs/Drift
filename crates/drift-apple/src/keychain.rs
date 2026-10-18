//! Keychain password adapter (task **M3-2**, plan §2 decision 7).
//!
//! Passwords are generic-password items. The **service** is Drift's bundle identifier
//! ([`Keychain::SERVICE`]); the **account** is `SecretRole::account(profile_id)` =
//! `<uuid>/<role>` (`rdp-system`, `rdp-user`, `linux-login`), so every (profile, role) pair has
//! exactly one item.
//!
//! Production items live in the user's default (login) keychain. Tests never touch it: they use
//! a throw-away keychain file in a temporary directory ([`Keychain::create_file`]) and a unique
//! test-only service name. (This also keeps the tests working where the login keychain is
//! locked, e.g. over SSH or in CI, which report `errSecInteractionNotAllowed`.)
//!
//! On iOS there are no keychain files: items live in the app's data-protection keychain, and
//! the file constructors do not exist (IOS-3).
//!
//! One-time Server Redirection credentials never come here: they only live in memory.

#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

use drift_core::SecretRole;
use security_framework::item::{ItemClass, ItemSearchOptions};
#[cfg(target_os = "macos")]
use security_framework::os::macos::keychain::{CreateOptions, SecKeychain};
#[cfg(target_os = "macos")]
use security_framework::os::macos::passwords::find_generic_password;
#[cfg(not(target_os = "macos"))]
use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};
use uuid::Uuid;
use zeroize::Zeroizing;

/// `errSecItemNotFound`.
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// A Keychain operation failed (the message never contains the secret).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Keychain error {code}: {message}")]
pub struct KeychainError {
    /// The `OSStatus` (0 for a stored value that is not UTF-8).
    pub code: i32,
    /// Human-readable description from Security.framework.
    pub message: String,
}

impl From<security_framework::base::Error> for KeychainError {
    fn from(e: security_framework::base::Error) -> Self {
        Self { code: e.code(), message: e.to_string() }
    }
}

/// Which keychain holds the items.
#[derive(Clone, PartialEq, Eq)]
enum Location {
    /// The user's default keychain (login); the app's data-protection keychain on iOS.
    Default,
    /// An explicit keychain file, unlocked with `password` on every use (tests; macOS only).
    #[cfg(target_os = "macos")]
    File { path: PathBuf, password: Zeroizing<String> },
}

/// Generic-password store keyed by profile id and role.
#[derive(Clone, PartialEq, Eq)]
pub struct Keychain {
    service: String,
    location: Location,
}

impl std::fmt::Debug for Keychain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let location = match &self.location {
            Location::Default => "default".to_owned(),
            #[cfg(target_os = "macos")]
            Location::File { path, .. } => path.display().to_string(),
        };
        f.debug_struct("Keychain").field("service", &self.service).field("keychain", &location).finish()
    }
}

impl Default for Keychain {
    fn default() -> Self {
        Self::new()
    }
}

impl Keychain {
    /// The service name of Drift's items (the app's bundle identifier).
    pub const SERVICE: &'static str = "com.hutsonlabs.drift";

    /// The production store: Drift's service in the default keychain.
    pub fn new() -> Self {
        Self::with_service(Self::SERVICE)
    }

    /// Another service name in the default keychain.
    pub fn with_service(service: impl Into<String>) -> Self {
        Self { service: service.into(), location: Location::Default }
    }

    /// Creates a new keychain file at `path` protected by `password` (tests and tooling) and
    /// returns a store for `service` in it. The file is not added to the search list.
    #[cfg(target_os = "macos")]
    pub fn create_file(
        path: &Path,
        password: &str,
        service: impl Into<String>,
    ) -> Result<Self, KeychainError> {
        CreateOptions::new().password(password).prompt_user(false).create(path)?;
        Ok(Self::open_file(path, password, service))
    }

    /// A store for `service` in an existing keychain file (see [`Keychain::create_file`]).
    #[cfg(target_os = "macos")]
    pub fn open_file(path: &Path, password: &str, service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            location: Location::File { path: path.to_owned(), password: Zeroizing::new(password.to_owned()) },
        }
    }

    /// The service name.
    pub fn service(&self) -> &str {
        &self.service
    }

    #[cfg(target_os = "macos")]
    fn keychain(&self) -> Result<SecKeychain, KeychainError> {
        match &self.location {
            Location::Default => Ok(SecKeychain::default()?),
            Location::File { path, password } => {
                let mut kc = SecKeychain::open(path)?;
                kc.unlock(Some(password))?;
                Ok(kc)
            }
        }
    }

    /// Stores (creates or replaces) the secret for `profile`/`role`.
    pub fn set(&self, profile: Uuid, role: SecretRole, secret: &str) -> Result<(), KeychainError> {
        self.set_item(&role.account(profile), secret.as_bytes())
    }

    /// Reads the secret for `profile`/`role`; `None` if there is no item.
    pub fn get(&self, profile: Uuid, role: SecretRole) -> Result<Option<Zeroizing<String>>, KeychainError> {
        let Some(bytes) = self.get_item(&role.account(profile))? else { return Ok(None) };
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| KeychainError { code: 0, message: "stored password is not valid UTF-8".into() })?;
        Ok(Some(Zeroizing::new(text.to_owned())))
    }

    /// Deletes the secret for `profile`/`role`; deleting a missing item succeeds.
    pub fn delete(&self, profile: Uuid, role: SecretRole) -> Result<(), KeychainError> {
        self.delete_item(&role.account(profile))
    }

    /// Stores (creates or replaces) raw bytes under `account` (pairing keys and pins, IOS-9;
    /// accounts never look like `<uuid>/<role>`).
    #[cfg(target_os = "macos")]
    pub fn set_item(&self, account: &str, bytes: &[u8]) -> Result<(), KeychainError> {
        Ok(self.keychain()?.set_generic_password(&self.service, account, bytes)?)
    }

    /// Reads the bytes under `account`; `None` if there is no item.
    #[cfg(target_os = "macos")]
    pub fn get_item(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeychainError> {
        let kc = self.keychain()?;
        match find_generic_password(Some(std::slice::from_ref(&kc)), &self.service, account) {
            Ok((password, _item)) => Ok(Some(Zeroizing::new(password.as_ref().to_vec()))),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Deletes the item under `account`; deleting a missing item succeeds.
    #[cfg(target_os = "macos")]
    pub fn delete_item(&self, account: &str) -> Result<(), KeychainError> {
        let kc = self.keychain()?;
        match find_generic_password(Some(std::slice::from_ref(&kc)), &self.service, account) {
            Ok((_password, item)) => {
                item.delete();
                Ok(())
            }
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Stores (creates or replaces) raw bytes under `account`.
    #[cfg(not(target_os = "macos"))]
    pub fn set_item(&self, account: &str, bytes: &[u8]) -> Result<(), KeychainError> {
        Ok(set_generic_password(&self.service, account, bytes)?)
    }

    /// Reads the bytes under `account`; `None` if there is no item.
    #[cfg(not(target_os = "macos"))]
    pub fn get_item(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeychainError> {
        match get_generic_password(&self.service, account) {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Deletes the item under `account`; deleting a missing item succeeds.
    #[cfg(not(target_os = "macos"))]
    pub fn delete_item(&self, account: &str) -> Result<(), KeychainError> {
        match delete_generic_password(&self.service, account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Whether a secret is stored for `profile`/`role`, **without reading it**.
    ///
    /// This is an attribute-only `SecItemCopyMatching`: Security.framework neither decrypts the
    /// item nor evaluates its ACL, so it never puts up the "Drift wants to use your confidential
    /// information" panel and never blocks. [`Keychain::get`] does both, which is why the app
    /// must not answer "is a password stored?" with it (a panel nobody clicks would freeze the
    /// thread asking, and the app launches without a window).
    #[cfg(target_os = "macos")]
    pub fn contains(&self, profile: Uuid, role: SecretRole) -> Result<bool, KeychainError> {
        let kc = self.keychain()?;
        let found = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .keychains(std::slice::from_ref(&kc))
            .service(&self.service)
            .account(&role.account(profile))
            .load_attributes(true)
            .limit(1)
            .search();
        match found {
            Ok(items) => Ok(!items.is_empty()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Whether a secret is stored for `profile`/`role`, **without reading it** (an
    /// attribute-only `SecItemCopyMatching`, as on macOS).
    #[cfg(not(target_os = "macos"))]
    pub fn contains(&self, profile: Uuid, role: SecretRole) -> Result<bool, KeychainError> {
        let found = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(&self.service)
            .account(&role.account(profile))
            .load_attributes(true)
            .limit(1)
            .search();
        match found {
            Ok(items) => Ok(!items.is_empty()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Deletes every role's secret for `profile` (profile deleted).
    pub fn delete_profile(&self, profile: Uuid) -> Result<(), KeychainError> {
        for role in [SecretRole::RdpSystem, SecretRole::RdpUser, SecretRole::LinuxLogin] {
            self.delete(profile, role)?;
        }
        Ok(())
    }
}
