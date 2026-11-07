//! WIZ-4 live check (ignored by default): the wizard's service runs the real
//! `host/drift-host-setup.sh` in `--check` mode (changes nothing) on a real machine over the
//! in-app SSH client.
//!
//! ```text
//! DRIFT_SETUP_LIVE=alice@100.64.0.83 DRIFT_SETUP_LIVE_PORT=3390 \
//! DRIFT_SSH_LIVE_STORE=/path/to/store \
//!     cargo nextest run -p drift-app --test setup_live --run-ignored only --no-capture
//! ```
//!
//! The store's device key must be in the account's `authorized_keys`, and the account needs
//! passwordless sudo (or the check stops at the sudo prompt).

use drift_app::setup_wizard::{SETUP_SCRIPT, SetupService, SignIn};
use drift_core::setup::{Outcome, RunMode, SetupLine};
use drift_ssh::{Target, Trust};

#[tokio::test]
#[ignore = "needs DRIFT_SETUP_LIVE=user@host, DRIFT_SETUP_LIVE_PORT and an authorized DRIFT_SSH_LIVE_STORE"]
async fn the_real_setup_script_checks_a_real_machine() {
    let spec = std::env::var("DRIFT_SETUP_LIVE").expect("DRIFT_SETUP_LIVE=user@host");
    let (user, host) = spec.split_once('@').expect("user@host");
    let store = std::env::var("DRIFT_SSH_LIVE_STORE").expect("DRIFT_SSH_LIVE_STORE");
    let service = SetupService::new(store, "drift@live", SETUP_SCRIPT).expect("service");
    let target = Target { host: host.into(), port: 22, user: user.into() };
    let signed = service.connect(target, Trust::TrustNew).await;
    assert!(matches!(signed, SignIn::Ready { .. }), "{signed:?}");

    let mut options = service.defaults(host, user);
    // Keep the machine's RDP credentials and its port: a check must not plan to change them.
    options.rdp_password = None;
    if let Ok(port) = std::env::var("DRIFT_SETUP_LIVE_PORT") {
        options.port = port.parse().expect("port");
    }
    let mut lines = Vec::new();
    let result = service
        .run(&options, RunMode::Check, None, &mut |l| {
            eprintln!("{l:?}");
            lines.push(l);
        })
        .await
        .expect("check");
    assert_ne!(result.outcome, Outcome::Failed, "{:?}", result.errors);
    assert_eq!(lines.first(), Some(&SetupLine::Mode { mode: RunMode::Check }));
    assert!(
        lines.iter().any(|l| matches!(l, SetupLine::Fingerprint { owner, .. } if owner == user)),
        "{lines:?}"
    );
    service.disconnect().await;
}
