//! Live-app smoke harness: the real Drift app (windows, tabs, menus, RemoteView, Metal,
//! SessionManager, drift-rdp) with the passwords supplied by the environment instead of the
//! Keychain, so the whole launch → connect → live picture path can be driven unattended.
//!
//! It exists because the Keychain cannot be seeded from a script: an item written by
//! `/usr/bin/security` carries the partition list `apple-tool:`, so any other binary reading it
//! gets the "Drift wants to use your confidential information" panel, and a smoke run on a
//! machine with a locked screen (or no person in front of it) hangs on that panel forever.
//! The production binary always uses the Keychain — this example is the only way to bypass it.
//!
//! ```text
//! DRIFT_CONFIG_DIR=/tmp/drift-smoke \
//! DRIFT_AUTOCONNECT="Homelab Headless" \
//! DRIFT_SMOKE_SECRET="<profile-uuid>:rdp-user:<password>" \
//!   cargo run -p drift-app --example smoke
//! ```
//!
//! `DRIFT_SMOKE_SECRET` may be repeated as `DRIFT_SMOKE_SECRET_2`, `_3`, … Roles are the
//! `SecretRole` names: `rdp-system`, `rdp-user`, `linux-login`.

use std::sync::Arc;

use drift_app::secrets::{MemorySecretStore, SecretStore};
use drift_app::{RunOptions, secrets};
use drift_core::SecretRole;

/// `<profile uuid>:<role>:<password>`; the password may contain `:`.
fn parse_secret(spec: &str) -> Result<(uuid::Uuid, SecretRole, String), String> {
    let mut parts = spec.splitn(3, ':');
    let id = parts.next().ok_or("missing profile id")?;
    let role = parts.next().ok_or("missing role")?;
    let password = parts.next().ok_or("missing password")?;
    let id: uuid::Uuid = id.parse().map_err(|_| format!("{id} is not a UUID"))?;
    let role = match role {
        "rdp-system" => SecretRole::RdpSystem,
        "rdp-user" => SecretRole::RdpUser,
        "linux-login" => SecretRole::LinuxLogin,
        other => return Err(format!("unknown role {other}")),
    };
    Ok((id, role, password.to_owned()))
}

fn main() {
    let store = Arc::new(MemorySecretStore::new());
    let mut loaded = 0_usize;
    for (name, value) in std::env::vars() {
        if !name.starts_with("DRIFT_SMOKE_SECRET") || value.trim().is_empty() {
            continue;
        }
        match parse_secret(&value) {
            // Never print the value, only that one was accepted.
            Ok((id, role, password)) => match store.set(id, role, &password) {
                Ok(()) => {
                    loaded += 1;
                    eprintln!("smoke: loaded the {} password of {id}", role.as_str());
                }
                Err(e) => eprintln!("smoke: {name} could not be stored: {e}"),
            },
            Err(e) => eprintln!("smoke: {name} is malformed: {e}"),
        }
    }
    eprintln!("smoke: {loaded} password(s) in memory; the Keychain is not used");

    let mut options = RunOptions::from_env();
    options.secrets = Some(store as Arc<dyn secrets::SecretStore>);
    drift_app::run_with(options);
}
