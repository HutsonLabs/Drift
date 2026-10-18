//! Passwords from the environment for unattended smoke runs (the macOS `smoke` example, and the
//! iOS simulator build with the `smoke-env` feature; IOS-5).
//!
//! The Keychain cannot be seeded from a script (see `examples/smoke.rs`), so a smoke run hands
//! the app `DRIFT_SMOKE_SECRET="<profile uuid>:<role>:<password>"` (also `_2`, `_3`, …) and the
//! passwords live only in a [`MemorySecretStore`]. The production app never reads them: the
//! macOS binary has no such path, and the iOS app only with the `smoke-env` feature, which no
//! release build enables.

use std::sync::Arc;

use drift_core::SecretRole;
use uuid::Uuid;

use crate::secrets::{MemorySecretStore, SecretStore};

/// `<profile uuid>:<role>:<password>`; the password may contain `:`.
pub fn parse_secret(spec: &str) -> Result<(Uuid, SecretRole, String), String> {
    let mut parts = spec.splitn(3, ':');
    let id = parts.next().ok_or("missing profile id")?;
    let role = parts.next().ok_or("missing role")?;
    let password = parts.next().ok_or("missing password")?;
    let id: Uuid = id.parse().map_err(|_| format!("{id} is not a UUID"))?;
    let role = match role {
        "rdp-system" => SecretRole::RdpSystem,
        "rdp-user" => SecretRole::RdpUser,
        "linux-login" => SecretRole::LinuxLogin,
        other => return Err(format!("unknown role {other}")),
    };
    Ok((id, role, password.to_owned()))
}

/// A memory store with every `DRIFT_SMOKE_SECRET*` in `vars`. Logs what was loaded, never the
/// values.
pub fn secrets_from(vars: impl IntoIterator<Item = (String, String)>) -> Arc<MemorySecretStore> {
    let store = Arc::new(MemorySecretStore::new());
    let mut loaded = 0_usize;
    for (name, value) in vars {
        if !name.starts_with("DRIFT_SMOKE_SECRET") || value.trim().is_empty() {
            continue;
        }
        match parse_secret(&value) {
            Ok((id, role, password)) => match store.set(id, role, &password) {
                Ok(()) => {
                    loaded += 1;
                    tracing::info!(profile = %id, role = role.as_str(), "smoke: loaded a password");
                }
                Err(e) => tracing::warn!(%name, error = %e, "smoke: could not store a password"),
            },
            Err(e) => tracing::warn!(%name, error = %e, "smoke: malformed secret"),
        }
    }
    tracing::info!(loaded, "smoke: passwords in memory; the Keychain is not used");
    store
}
