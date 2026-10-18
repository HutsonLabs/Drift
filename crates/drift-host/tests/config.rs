//! HOST-2 Red: the host's configuration, identity, session gate and layout policy (pure parts).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use drift_core::Size;
use drift_host::config::HostConfig;
use drift_host::gate::SessionGate;
use drift_host::identity::HostIdentity;
use drift_host::layout::{MAX_SIDE, MIN_SIDE, clamp_layout};

#[test]
fn the_default_listener_is_loopback_only_on_the_rdp_port() {
    // Until pairing (HOST-8) exists the host must not be reachable from the network by default.
    let c = HostConfig::default();
    assert!(c.listen.ip().is_loopback(), "{}", c.listen);
    assert_eq!(c.listen.port(), 3389);
    assert!(!c.inject_input, "input injection stays off in the skeleton");
    assert_eq!(c.desktop, Size::new(1920, 1080));
}

#[test]
fn config_round_trips_through_toml_and_a_missing_file_is_the_default() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(HostConfig::load(dir.path()).unwrap(), HostConfig::default());
    let mut c = HostConfig::default();
    c.listen = "0.0.0.0:3390".parse().unwrap();
    "someone".clone_into(&mut c.username);
    c.save(dir.path()).unwrap();
    assert_eq!(HostConfig::load(dir.path()).unwrap(), c);
    assert!(c.listens_beyond_loopback());
    assert!(!HostConfig::default().listens_beyond_loopback());
}

#[test]
fn a_malformed_config_is_an_error_not_a_default() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(drift_host::config::CONFIG_FILE), "listen = 12").unwrap();
    assert!(HostConfig::load(dir.path()).is_err());
}

#[test]
fn the_identity_is_created_once_and_reused() {
    let dir = tempfile::tempdir().unwrap();
    let a = HostIdentity::load_or_create(dir.path()).unwrap();
    let b = HostIdentity::load_or_create(dir.path()).unwrap();
    assert_eq!(a.fingerprint(), b.fingerprint(), "the pinned certificate never changes by itself");
    assert_eq!(a.public_key(), b.public_key());
    assert!(!a.public_key().is_empty());
    use std::os::unix::fs::PermissionsExt as _;
    let mode =
        std::fs::metadata(dir.path().join(drift_host::identity::KEY_FILE)).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "the private key is readable by the owner only");
    let other = tempfile::tempdir().unwrap();
    assert_ne!(HostIdentity::load_or_create(other.path()).unwrap().fingerprint(), a.fingerprint());
}

#[test]
fn one_session_at_a_time() {
    let gate = SessionGate::new();
    let first = gate.try_begin().expect("first session");
    assert!(gate.try_begin().is_none(), "a second session is refused while one runs");
    assert!(gate.busy());
    drop(first);
    assert!(!gate.busy());
    assert!(gate.try_begin().is_some(), "free again once the first ends");
}

#[test]
fn client_layouts_are_clamped_to_even_sizes_within_limits() {
    assert_eq!(clamp_layout(Size::new(1000, 700)), Size::new(1000, 700));
    assert_eq!(clamp_layout(Size::new(1001, 701)), Size::new(1000, 700), "even, as H.264 4:2:0 wants");
    assert_eq!(clamp_layout(Size::new(10, 10)), Size::new(MIN_SIDE, MIN_SIDE));
    assert_eq!(clamp_layout(Size::new(100_000, 3)), Size::new(MAX_SIDE, MIN_SIDE));
}
