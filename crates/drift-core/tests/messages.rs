//! M1-6 / M9-4 Red: disconnect explanations with mode-specific next steps.

use drift_core::messages::{LOCAL_NETWORK_SETTINGS_PATH, LOCAL_NETWORK_SETTINGS_URL};
use drift_core::{ConnectMode, DisconnectReason, ErrorAction, explain_disconnect};

fn all_reasons() -> Vec<DisconnectReason> {
    vec![
        DisconnectReason::Network,
        DisconnectReason::TlsEof,
        DisconnectReason::ServerShutdown,
        DisconnectReason::Timeout,
        DisconnectReason::AuthFailed,
        DisconnectReason::RdstlsFailed(0x52E),
        DisconnectReason::CertMismatch,
        DisconnectReason::ProtocolError("bad pdu".into()),
        DisconnectReason::RedirectLoop,
        DisconnectReason::UserClosed,
        DisconnectReason::LoggedOffRemotely,
        DisconnectReason::LocalNetworkDenied,
    ]
}

fn steps(reason: DisconnectReason, mode: ConnectMode) -> String {
    explain_disconnect(&reason, mode).next_steps.join("\n")
}

#[test]
fn every_reason_and_mode_has_title_message_and_an_action() {
    for mode in ConnectMode::ALL {
        for reason in all_reasons() {
            let e = explain_disconnect(&reason, mode);
            assert!(!e.title.is_empty() && !e.message.is_empty(), "{reason:?}/{mode:?}");
            assert!(!e.actions.is_empty(), "{reason:?}/{mode:?}");
            assert!(!e.title.ends_with('.'), "titles are headlines: {e:?}");
        }
    }
}

#[test]
fn mode_specific_next_steps_table() {
    #[rustfmt::skip]
    let table: &[(DisconnectReason, ConnectMode, &str)] = &[
        (DisconnectReason::AuthFailed, ConnectMode::DesktopSharing,
            "Desktop Sharing credentials must be set in GNOME Settings on the host."),
        (DisconnectReason::AuthFailed, ConnectMode::RemoteLogin,
            "sudo grdctl --system status --show-credentials"),
        (DisconnectReason::AuthFailed, ConnectMode::Headless,
            "grdctl --headless rdp set-credentials"),
        (DisconnectReason::Network, ConnectMode::Headless,
            "Headless session not running — see host/drift-host-setup.sh."),
        (DisconnectReason::Timeout, ConnectMode::Headless,
            "Headless session not running — see host/drift-host-setup.sh."),
        (DisconnectReason::Network, ConnectMode::RemoteLogin, "sudo grdctl --system status"),
        (DisconnectReason::Network, ConnectMode::DesktopSharing, "Desktop Sharing is on in GNOME Settings"),
        (DisconnectReason::CertMismatch, ConnectMode::RemoteLogin, "sudo grdctl --system status"),
        (DisconnectReason::CertMismatch, ConnectMode::Headless, "grdctl --headless status"),
        (DisconnectReason::LocalNetworkDenied, ConnectMode::Headless,
            "Open System Settings › Privacy & Security › Local Network and turn on Drift."),
    ];
    for (reason, mode, needle) in table {
        let s = steps(reason.clone(), *mode);
        assert!(s.contains(needle), "{reason:?}/{mode:?}: {s:?} lacks {needle:?}");
    }
}

#[test]
fn actions_table() {
    use ErrorAction::*;
    let m = ConnectMode::Headless;
    assert_eq!(
        explain_disconnect(&DisconnectReason::LocalNetworkDenied, m).actions,
        [OpenLocalNetworkSettings, Reconnect]
    );
    assert_eq!(explain_disconnect(&DisconnectReason::AuthFailed, m).actions[0], EditProfile);
    assert_eq!(explain_disconnect(&DisconnectReason::CertMismatch, m).actions, [EditProfile, Close]);
    assert_eq!(explain_disconnect(&DisconnectReason::Network, m).actions[0], Reconnect);
    for reason in all_reasons() {
        let has_settings = explain_disconnect(&reason, m).actions.contains(&OpenLocalNetworkSettings);
        assert_eq!(has_settings, reason == DisconnectReason::LocalNetworkDenied, "{reason:?}");
    }
}

#[test]
fn details_are_included() {
    let e = explain_disconnect(&DisconnectReason::RdstlsFailed(0x52E), ConnectMode::RemoteLogin);
    assert!(e.message.contains("0x52E"), "{e:?}");
    let e =
        explain_disconnect(&DisconnectReason::ProtocolError("unknown codec 9".into()), ConnectMode::Headless);
    assert!(e.message.contains("unknown codec 9"), "{e:?}");
}

#[test]
fn local_network_settings_constants() {
    assert_eq!(LOCAL_NETWORK_SETTINGS_PATH, "System Settings › Privacy & Security › Local Network");
    assert!(LOCAL_NETWORK_SETTINGS_URL.starts_with("x-apple.systempreferences:"));
    assert!(LOCAL_NETWORK_SETTINGS_URL.ends_with("Privacy_LocalNetwork"));
}
