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

/// One step of a `DRIFT_SMOKE_SCRIPT` (iOS simulator smoke runs, IOS-11): the simulator cannot
/// be tapped from the command line, so a script drives the same paths a tap would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmokeStep {
    /// `wait:<seconds>`.
    Wait(std::time::Duration),
    /// `connect:<profile name>`: what tapping a card in the gallery does.
    Connect(String),
    /// `pick:<window label>`: what choosing an entry in the switcher menu does.
    Pick(String),
}

/// Parses `step;step;…` (see [`SmokeStep`]).
pub fn parse_script(script: &str) -> Result<Vec<SmokeStep>, String> {
    script
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|step| {
            let (verb, arg) = step.split_once(':').ok_or_else(|| format!("`{step}` has no `:`"))?;
            match verb.trim() {
                "wait" => arg
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|s| s.is_finite() && *s >= 0.0)
                    .map(|s| SmokeStep::Wait(std::time::Duration::from_secs_f64(s)))
                    .ok_or_else(|| format!("`{arg}` is not a number of seconds")),
                "connect" => Ok(SmokeStep::Connect(arg.trim().to_owned())),
                "pick" => Ok(SmokeStep::Pick(arg.trim().to_owned())),
                other => Err(format!("unknown step `{other}`")),
            }
        })
        .collect()
}

/// Runs `steps` on a worker thread once the app is up (iOS only).
#[cfg(target_os = "ios")]
pub fn run_script(app: tauri::AppHandle, steps: Vec<SmokeStep>) {
    std::thread::spawn(move || {
        for step in steps {
            tracing::info!(?step, "smoke script");
            match step {
                SmokeStep::Wait(d) => std::thread::sleep(d),
                SmokeStep::Connect(name) => {
                    let state = tauri::Manager::state::<crate::commands::AppState>(&app);
                    let id =
                        state.profiles.profiles().unwrap_or_default().into_iter().find(|p| p.name == name);
                    match id {
                        Some(p) => {
                            if let Err(e) = crate::windows::connect_profile(&app, p.id) {
                                tracing::warn!(error = %e, "smoke script: connect failed");
                            }
                        }
                        None => tracing::warn!(%name, "smoke script: no such profile"),
                    }
                }
                SmokeStep::Pick(label) => {
                    let app2 = app.clone();
                    let _ = crate::windows::on_main(&app, move |_| crate::windows::pick(&app2, &label));
                }
            }
        }
    });
}
