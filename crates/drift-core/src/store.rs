//! The saved-profiles document (`profiles.toml` in the app config directory).
//!
//! [`ProfileStore`] is the pure model: parse, edit, serialize. File access (atomic write into
//! `~/Library/Application Support/<bundle id>/profiles.toml`) lives in `drift-app`. Profiles hold
//! no secrets (plan §2 decision 7), so the file is safe to back up.
//!
//! Format:
//! ```toml
//! version = 1
//!
//! [[profiles]]
//! id = "…"
//! name = "Homelab"
//! # … every ConnectionProfile field
//! ```

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::profile::{self, CertFingerprint, ConnectionProfile, ProfileIssue};

/// Current `profiles.toml` format version.
pub const STORE_VERSION: u32 = 1;

/// Errors from loading or editing the profile store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The document is not valid TOML or does not match the schema.
    #[error("profiles file is not valid: {0}")]
    Parse(String),
    /// The document was written by a newer Drift.
    #[error("profiles file version {found} is newer than supported version {STORE_VERSION}")]
    UnsupportedVersion {
        /// Version found in the file.
        found: u32,
    },
    /// Two profiles share an id.
    #[error("profiles file contains the id {0} more than once")]
    DuplicateId(Uuid),
    /// The profile is paired with a Drift Host: its pin only changes by pairing again.
    #[error("profile {0} is a paired Mac; pair it again to trust a new certificate")]
    PairedHost(Uuid),
    /// Serialization failed.
    #[error("could not serialize profiles: {0}")]
    Serialize(String),
    /// The profile failed validation.
    #[error("profile is not valid ({} issue(s))", .0.len())]
    Invalid(Vec<ProfileIssue>),
    /// No profile has this id.
    #[error("no profile with id {0}")]
    NotFound(Uuid),
}

#[derive(Serialize, Deserialize)]
struct Document {
    version: u32,
    #[serde(default)]
    profiles: Vec<ConnectionProfile>,
}

/// All saved connection profiles, in insertion order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileStore {
    profiles: Vec<ConnectionProfile>,
}

impl ProfileStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses `profiles.toml`. Empty or whitespace-only input is an empty store.
    ///
    /// Profiles that fail [`profile::validate`] are still loaded (the user can fix them in the
    /// form); structural problems (bad TOML, duplicate ids, newer version) are errors.
    pub fn from_toml(s: &str) -> Result<Self, StoreError> {
        if s.trim().is_empty() {
            return Ok(Self::new());
        }
        let doc: Document = toml::from_str(s).map_err(|e| StoreError::Parse(e.to_string()))?;
        if doc.version > STORE_VERSION {
            return Err(StoreError::UnsupportedVersion { found: doc.version });
        }
        for (i, p) in doc.profiles.iter().enumerate() {
            if doc.profiles[..i].iter().any(|q| q.id == p.id) {
                return Err(StoreError::DuplicateId(p.id));
            }
        }
        Ok(Self { profiles: doc.profiles })
    }

    /// Serializes to `profiles.toml` text.
    pub fn to_toml(&self) -> Result<String, StoreError> {
        let doc = Document { version: STORE_VERSION, profiles: self.profiles.clone() };
        toml::to_string(&doc).map_err(|e| StoreError::Serialize(e.to_string()))
    }

    /// Profiles in insertion order.
    pub fn profiles(&self) -> &[ConnectionProfile] {
        &self.profiles
    }

    /// Profiles sorted for display: by name (case-insensitive), then id.
    pub fn sorted(&self) -> Vec<ConnectionProfile> {
        let mut v = self.profiles.clone();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
        v
    }

    /// The profile with `id`.
    pub fn get(&self, id: Uuid) -> Option<&ConnectionProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Validates `profile` and inserts it, replacing any profile with the same id in place.
    ///
    /// Returns `true` if an existing profile was replaced.
    pub fn upsert(&mut self, profile: ConnectionProfile) -> Result<bool, StoreError> {
        profile::validate(&profile).map_err(StoreError::Invalid)?;
        if let Some(slot) = self.profiles.iter_mut().find(|p| p.id == profile.id) {
            *slot = profile;
            Ok(true)
        } else {
            self.profiles.push(profile);
            Ok(false)
        }
    }

    /// Removes and returns the profile with `id`.
    pub fn remove(&mut self, id: Uuid) -> Result<ConnectionProfile, StoreError> {
        let pos = self.profiles.iter().position(|p| p.id == id).ok_or(StoreError::NotFound(id))?;
        Ok(self.profiles.remove(pos))
    }

    /// Stores (or clears) the TOFU certificate pin of profile `id`.
    pub fn set_pin(&mut self, id: Uuid, pin: Option<CertFingerprint>) -> Result<(), StoreError> {
        let p = self.profiles.iter_mut().find(|p| p.id == id).ok_or(StoreError::NotFound(id))?;
        // A paired host's pin came from the pairing (IOS-9): no TOFU prompt or "forget" may
        // replace it.
        if p.paired_host.is_some() && p.cert_pin != pin {
            return Err(StoreError::PairedHost(id));
        }
        p.cert_pin = pin;
        Ok(())
    }
}
