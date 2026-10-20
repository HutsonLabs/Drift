//! M1-6 / M3-2 / M7-3 Red: the per-window view model decides which screen the webview shows.

use std::time::Duration;

use drift_app::view::{CertificateSubject, Screen, SessionView};
use drift_core::{
    CertFingerprint, ConnectMode, ConnectStage, ConnectionProfile, DesktopSize, DisconnectReason,
    ErrorAction, SessionState,
};
use drift_rdp::{CertificateRole, SessionEvent, SessionStats};

fn view(mode: ConnectMode) -> SessionView {
    let mut p = ConnectionProfile::new("Homelab", "10.1.2.40", mode);
    if mode == ConnectMode::RemoteLogin {
        p.linux_username = Some("drifttest".into());
    }
    SessionView::new(&p)
}

fn state(s: SessionState) -> SessionEvent {
    SessionEvent::State(s)
}

fn connecting(leg: u8) -> SessionEvent {
    state(SessionState::Connecting { leg, stage: ConnectStage::Tls })
}

fn connected() -> SessionEvent {
    state(SessionState::Connected { desktop: DesktopSize::new(1280, 800), scale: 100 })
}

fn prompt(role: CertificateRole) -> SessionEvent {
    SessionEvent::CertificatePrompt {
        host: "10.1.2.40".into(),
        port: 3389,
        fingerprint: CertFingerprint::from_bytes([0xf3; 32]),
        role,
    }
}

#[test]
fn initial_view_shows_profiles() {
    let v = view(ConnectMode::Headless);
    assert_eq!(v.screen, Screen::Profiles);
    assert_eq!(v.profile_name, "Homelab");
    assert_eq!(v.max_attempts, Some(20));
    assert!(!v.resuming && v.certificate.is_none() && v.explanation.is_none());
}

#[test]
fn screen_table() {
    let retry = SessionState::Reconnecting {
        attempt: 2,
        next_in: Duration::from_secs(3),
        reason: DisconnectReason::Network,
    };
    #[rustfmt::skip]
    let table: &[(SessionEvent, Screen)] = &[
        (connecting(1), Screen::Connecting),
        (state(SessionState::AwaitingGreeterLogin), Screen::GreeterHint),
        (connected(), Screen::Live),
        (state(retry), Screen::Reconnecting),
        (state(SessionState::Failed { reason: DisconnectReason::AuthFailed }), Screen::Error),
        (state(SessionState::Disconnected { reason: DisconnectReason::UserClosed }), Screen::Error),
        (state(SessionState::Idle), Screen::Profiles),
    ];
    for (event, screen) in table {
        let mut v = view(ConnectMode::RemoteLogin);
        v.apply(&state(SessionState::Connecting { leg: 2, stage: ConnectStage::Activation }));
        assert!(v.apply(event), "{event:?} changes the view");
        assert_eq!(v.screen, *screen, "{event:?}");
    }
}

#[test]
fn certificate_prompt_overrides_the_state_until_the_next_state() {
    let mut v = view(ConnectMode::RemoteLogin);
    v.apply(&connecting(1));
    assert!(v.apply(&prompt(CertificateRole::Server)));
    assert_eq!(v.screen, Screen::Certificate);
    let c = v.certificate.clone().unwrap();
    assert_eq!(c.fingerprint.to_string(), ["f3"; 32].join(":"));
    assert_eq!((c.host.as_str(), c.port, c.subject), ("10.1.2.40", 3389, CertificateSubject::Server));
    assert_eq!(c.grdctl_command, "sudo grdctl --system status");

    v.apply(&state(SessionState::Connecting { leg: 1, stage: ConnectStage::Nla }));
    assert_eq!(v.screen, Screen::Connecting);
    assert!(v.certificate.is_none());

    v.apply(&prompt(CertificateRole::RedirectTarget));
    assert_eq!(v.certificate.as_ref().unwrap().subject, CertificateSubject::RedirectTarget);
    v.clear_certificate();
    assert_eq!(v.screen, Screen::Connecting);
}

#[test]
fn grdctl_command_depends_on_mode() {
    for (mode, cmd) in [
        (ConnectMode::RemoteLogin, "sudo grdctl --system status"),
        (ConnectMode::Headless, "grdctl --headless status"),
        (ConnectMode::DesktopSharing, "grdctl status"),
    ] {
        let mut v = view(mode);
        v.apply(&prompt(CertificateRole::Server));
        assert_eq!(v.certificate.unwrap().grdctl_command, cmd);
    }
}

#[test]
fn greeter_after_a_desktop_means_resuming() {
    let mut v = view(ConnectMode::RemoteLogin);
    v.apply(&connecting(2));
    v.apply(&state(SessionState::AwaitingGreeterLogin));
    assert!(!v.resuming, "first login: nothing to resume yet");
    assert_eq!(v.linux_username.as_deref(), Some("drifttest"));
    v.apply(&connected());
    v.apply(&state(SessionState::Reconnecting {
        attempt: 1,
        next_in: Duration::ZERO,
        reason: DisconnectReason::Network,
    }));
    v.apply(&connecting(1));
    v.apply(&connecting(2));
    v.apply(&state(SessionState::AwaitingGreeterLogin));
    assert!(v.resuming);
    assert_eq!(v.screen, Screen::GreeterHint);
    v.apply(&state(SessionState::Idle));
    assert!(!v.resuming);
}

#[test]
fn ended_states_carry_mode_specific_explanations() {
    let mut v = view(ConnectMode::DesktopSharing);
    v.apply(&state(SessionState::Failed { reason: DisconnectReason::AuthFailed }));
    let e = v.explanation.clone().unwrap();
    assert!(e.next_steps.iter().any(|s| s.contains("GNOME Settings on the host")), "{e:?}");

    let mut v = view(ConnectMode::Headless);
    v.apply(&state(SessionState::Failed { reason: DisconnectReason::LocalNetworkDenied }));
    assert_eq!(v.explanation.as_ref().unwrap().actions[0], ErrorAction::OpenLocalNetworkSettings);
    v.apply(&connecting(1));
    assert!(v.explanation.is_none());
}

#[test]
fn unrelated_events_do_not_change_the_view() {
    let mut v = view(ConnectMode::Headless);
    v.apply(&connected());
    assert!(!v.apply(&SessionEvent::Capabilities(Default::default())));
    assert!(!v.apply(&connected()), "same state again is not a change");
}

fn stats(fps: f32) -> SessionEvent {
    SessionEvent::Stats(SessionStats {
        fps,
        bitrate_bps: 1_234_567,
        frame_latency_p95_ms: 4.26,
        input_to_wire_p99_ms: 0.8,
        unacked_frames: 2,
    })
}

#[test]
fn statistics_reach_the_view_so_the_hud_can_show_them() {
    // Plan M1 "Done (manual M1)" and M9-1: the fps number must be visible in the app.
    let mut v = view(ConnectMode::Headless);
    v.apply(&connected());
    assert!(v.stats.is_none() && !v.show_stats, "no sample yet, HUD off");
    assert!(v.apply(&stats(58.93)), "a first sample changes the view");
    let s = v.stats.expect("a sample");
    // Tenths of the displayed units: "58.9 fps · 1.2 Mbit/s · 4.3 ms · 2 unacked".
    assert_eq!((s.fps_tenths, s.mbit_tenths, s.latency_p95_tenths_ms, s.unacked_frames), (589, 12, 43, 2));
    assert!(!v.apply(&stats(58.94)), "the same displayed numbers are not a change");
    assert!(v.apply(&stats(60.0)));
}

#[test]
fn statistics_are_dropped_when_the_picture_goes_away() {
    let mut v = view(ConnectMode::Headless);
    v.apply(&connected());
    v.apply(&stats(60.0));
    v.apply(&state(SessionState::Reconnecting {
        attempt: 1,
        next_in: Duration::from_secs(1),
        reason: DisconnectReason::Network,
    }));
    assert!(v.stats.is_none(), "stale numbers must not linger under the reconnect overlay");
}

#[test]
fn the_statistics_hud_is_a_per_tab_toggle_that_survives_state_changes() {
    let mut v = view(ConnectMode::Headless);
    v.show_stats = true;
    v.apply(&connected());
    v.apply(&stats(60.0));
    assert!(v.show_stats, "Session ▸ Show Statistics is not reset by session events");
    let json = serde_json::to_value(&v).unwrap();
    assert_eq!(json["show_stats"], true);
    assert_eq!(json["stats"]["unacked_frames"], 2);
    assert_eq!(json["stats"]["fps_tenths"], 600);
}

#[test]
fn view_serializes_with_screen_and_state_tags() {
    let mut v = view(ConnectMode::Headless);
    v.apply(&connecting(1));
    let json = serde_json::to_value(&v).unwrap();
    assert_eq!(json["screen"], "connecting");
    assert_eq!(json["state"]["state"], "connecting");
    assert_eq!(json["mode"], "headless");
}
