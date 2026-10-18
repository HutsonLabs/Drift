//! Local Network Privacy: errno 65 detection and the System Settings link.
#![allow(missing_docs)]

use std::io;

use drift_apple::lnp::{EHOSTUNREACH, is_local_network_denied, local_network_settings_url};

#[test]
fn errno_65_is_local_network_denied() {
    assert_eq!(EHOSTUNREACH, 65);
    assert!(is_local_network_denied(&io::Error::from_raw_os_error(65)));
}

#[test]
fn other_connect_errors_are_not() {
    for errno in [1, 13, 50, 51, 60, 61, 64] {
        assert!(!is_local_network_denied(&io::Error::from_raw_os_error(errno)), "errno {errno}");
    }
    assert!(!is_local_network_denied(&io::Error::new(io::ErrorKind::TimedOut, "timeout")));
    assert!(!is_local_network_denied(&io::Error::other("host unreachable")));
}

#[test]
fn wrapped_errno_65_is_detected() {
    // tokio / TLS layers wrap the socket error.
    let inner = io::Error::from_raw_os_error(65);
    let wrapped = io::Error::new(io::ErrorKind::HostUnreachable, inner);
    assert!(is_local_network_denied(&wrapped));
    let twice = io::Error::other(io::Error::other(io::Error::from_raw_os_error(65)));
    assert!(is_local_network_denied(&twice));
}

#[test]
fn settings_url_opens_the_local_network_pane() {
    assert_eq!(
        local_network_settings_url(),
        "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork"
    );
}
