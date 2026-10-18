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

use drift_app::{RunOptions, secrets, smoke};

fn main() {
    let store = smoke::secrets_from(std::env::vars());
    let mut options = RunOptions::from_env();
    options.secrets = Some(store as std::sync::Arc<dyn secrets::SecretStore>);
    drift_app::run_with(options);
}
