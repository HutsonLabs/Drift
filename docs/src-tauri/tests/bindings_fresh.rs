//! M1-6 Red: the committed `ui/src/bindings.ts` matches the tauri-specta builder.
//!
//! `cargo xtask ci` runs the same check through `cargo xtask bindings --check`; this test makes
//! staleness visible in a plain `cargo test -p drift-app` too. Fix with `cargo xtask bindings`.

use std::path::Path;

#[test]
fn committed_bindings_are_fresh() {
    let committed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/src/bindings.ts");
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("bindings.ts");
    drift_app::export_bindings(&fresh).unwrap();
    let fresh = std::fs::read_to_string(fresh).unwrap();
    let committed = std::fs::read_to_string(committed).unwrap_or_default();
    assert!(
        fresh == committed,
        "ui/src/bindings.ts is stale; run `cargo xtask bindings` and commit the result"
    );
}

#[test]
fn bindings_expose_the_ui_intents_and_view_event() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bindings.ts");
    drift_app::export_bindings(&out).unwrap();
    let ts = std::fs::read_to_string(out).unwrap();
    for needle in [
        "listProfiles:",
        "newProfile:",
        "validateProfile:",
        "saveProfile:",
        "deleteProfile:",
        "forgetCertificate:",
        "openLocalNetworkSettings:",
        "connect:",
        "acceptCertificate:",
        "rejectCertificate:",
        "reconnectNow:",
        "cancelReconnect:",
        "closeSession:",
        "duplicateProfile:",
        "connections:",
        "showWindow:",
        "disconnectProfile:",
        "showConnections:",
        "newConnection:",
        "windowIdentity:",
        "toggleStats:",
        "focusContent:",
        "connectionsChanged:",
        "thumbnailUpdated:",
        "connectionsIntentRequested:",
        "windowIdentityChanged:",
        "\"no-session\"",
        "sessionViewChanged:",
        "export type SessionView =",
        "export type Screen =",
    ] {
        assert!(ts.contains(needle), "bindings lack {needle}");
    }
    // UI-windows: no tab commands or events, and no "back to the connect form" disconnect.
    for gone in ["Tab", "disconnect:"] {
        assert!(!ts.contains(gone), "bindings still have {gone}");
    }
}
