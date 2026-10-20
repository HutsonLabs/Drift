//! M1-1 Red tests: connect over loopback against `FakeServer` (plan §6 M1-1).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use common::{Harness, PASS, USER, is_connected, profile};
use drift_core::{CertFingerprint, ConnectMode, ConnectStage, DesktopSize, DisconnectReason, SessionState};
use drift_rdp::connect::{
    self, CertDecision, CertExpectation, CertVerdict, ConnectObserver, Dialer, LegAuth, LegRequest,
    TokioDialer,
};
use drift_rdp::{CertificateRole, SessionCommand, SessionEvent};
use drift_testkit::{FakeServer, LegScript, ManualClock, TestCert};
use ironrdp_connector::rdstls::RdstlsResultCode;
use ironrdp_connector::{ConnectorError, ConnectorErrorKind};
use ironrdp_pdu::rdp::capability_sets::MajorPlatformType;
use zeroize::Zeroizing;

const WAIT: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------- loopback: success

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_connects_with_pinned_cert() {
    let cert = TestCert::generate("127.0.0.1");
    let server = FakeServer::start(vec![LegScript::nla(cert.clone(), USER, PASS)]).await.unwrap();
    let mut h = Harness::start(profile(ConnectMode::Headless, server.port(), Some(cert.fingerprint())), PASS);

    let ev = h.wait_for("Connected", WAIT, is_connected).await;
    assert_eq!(
        ev,
        SessionEvent::State(SessionState::Connected {
            desktop: DesktopSize { width: 1280, height: 800 },
            scale: 100
        })
    );
    let states = h.states();
    assert_eq!(
        states.first(),
        Some(&SessionState::Connecting { leg: 1, stage: ConnectStage::Tcp }),
        "{states:?}"
    );
    assert!(states.contains(&SessionState::Connecting { leg: 1, stage: ConnectStage::Tls }), "{states:?}");
    assert!(states.contains(&SessionState::Connecting { leg: 1, stage: ConnectStage::Nla }), "{states:?}");

    let log = server.log();
    let leg = &log.legs[0];
    assert!(leg.activated, "{leg:?}");
    assert!(leg.gfx_protocol_advertised, "RNS_UD_CS_SUPPORT_DYNVC_GFX_PROTOCOL must be set");
    assert_eq!(leg.platform.as_deref(), Some(format!("{:?}", MajorPlatformType::MACINTOSH).as_str()));
    assert_eq!(leg.client_name.as_deref(), Some(connect::local_client_name().as_str()));
    // SSL | HYBRID (| HYBRID_EX): NLA requested, no RDSTLS on leg 1.
    assert_eq!(leg.requested_protocols & 0x3, 0x3, "requested {:#x}", leg.requested_protocols);
    assert_eq!(leg.requested_protocols & 0x4, 0, "no RDSTLS on an NLA leg");
    h.close().await;
}

// ---------------------------------------------------------------- loopback: failures

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_password_fails_with_auth_failed() {
    let cert = TestCert::generate("127.0.0.1");
    let server = FakeServer::start(vec![LegScript::nla(cert.clone(), USER, PASS)]).await.unwrap();
    let mut h = Harness::start(
        profile(ConnectMode::Headless, server.port(), Some(cert.fingerprint())),
        "Wrong9-pass",
    );

    let end = h.wait_terminal(WAIT).await;
    assert_eq!(end, SessionState::Failed { reason: DisconnectReason::AuthFailed });
    assert!(!h.states().iter().any(|s| matches!(s, SessionState::Connected { .. })));
    assert_eq!(server.log().legs.len(), 1, "AuthFailed is not retried");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pin_mismatch_fails_before_credentials_are_sent() {
    let cert = TestCert::generate("127.0.0.1");
    let other = TestCert::generate("127.0.0.1");
    let server = FakeServer::start(vec![LegScript::nla(cert, USER, PASS)]).await.unwrap();
    let mut h =
        Harness::start(profile(ConnectMode::Headless, server.port(), Some(other.fingerprint())), PASS);

    let end = h.wait_terminal(WAIT).await;
    assert_eq!(end, SessionState::Failed { reason: DisconnectReason::CertMismatch });
    assert!(
        !h.seen.iter().any(|e| matches!(e, SessionEvent::CertificatePrompt { .. })),
        "a pin never prompts"
    );
    assert!(!h.states().contains(&SessionState::Connecting { leg: 1, stage: ConnectStage::Nla }));
    assert!(!server.log().legs[0].activated);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_cert_prompts_then_waits_for_the_user() {
    let cert = TestCert::generate("127.0.0.1");
    let server = FakeServer::start(vec![LegScript::nla(cert.clone(), USER, PASS)]).await.unwrap();
    let mut h = Harness::start(profile(ConnectMode::Headless, server.port(), None), PASS);

    let prompt =
        h.wait_for("CertificatePrompt", WAIT, |e| matches!(e, SessionEvent::CertificatePrompt { .. })).await;
    let SessionEvent::CertificatePrompt { host, port, fingerprint, role } = prompt else { unreachable!() };
    assert_eq!((host.as_str(), port, role), ("127.0.0.1", server.port(), CertificateRole::Server));
    assert_eq!(fingerprint, cert.fingerprint());

    // The actor waits: nothing happens without a decision.
    let idle = h.drain_for(Duration::from_millis(400)).await;
    assert!(idle.is_empty(), "actor must wait for the user, got {idle:?}");
    assert!(!server.log().legs[0].activated);

    h.handle.send(SessionCommand::AcceptCertificate { fingerprint, pin: true }).unwrap();
    h.wait_for("CertificatePinned", WAIT, |e| *e == SessionEvent::CertificatePinned(cert.fingerprint()))
        .await;
    h.wait_for("Connected", WAIT, is_connected).await;
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejecting_an_unknown_cert_fails_with_cert_mismatch() {
    let cert = TestCert::generate("127.0.0.1");
    let server = FakeServer::start(vec![LegScript::nla(cert, USER, PASS)]).await.unwrap();
    let mut h = Harness::start(profile(ConnectMode::Headless, server.port(), None), PASS);
    h.wait_for("CertificatePrompt", WAIT, |e| matches!(e, SessionEvent::CertificatePrompt { .. })).await;
    h.handle.send(SessionCommand::RejectCertificate).unwrap();
    assert_eq!(h.wait_terminal(WAIT).await, SessionState::Failed { reason: DisconnectReason::CertMismatch });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_server_times_out_on_the_manual_clock() {
    let server = FakeServer::start(vec![LegScript::stall(TestCert::generate("127.0.0.1"))]).await.unwrap();
    let clock = ManualClock::new();
    let mut h = Harness::start_with_clock(
        profile(ConnectMode::Headless, server.port(), None),
        PASS,
        Arc::new(clock.clone()),
    );

    h.wait_for("Connecting", WAIT, |e| matches!(e, SessionEvent::State(SessionState::Connecting { .. })))
        .await;
    // Real time passes, the manual clock does not: no timeout.
    let quiet = h.drain_for(Duration::from_millis(500)).await;
    assert!(
        !quiet.iter().any(|e| matches!(
            e,
            SessionEvent::State(SessionState::Disconnected { .. } | SessionState::Failed { .. })
        )),
        "timed out without the clock moving: {quiet:?}"
    );
    clock.advance(Duration::from_secs(16));
    assert_eq!(h.wait_terminal(WAIT).await, SessionState::Disconnected { reason: DisconnectReason::Timeout });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_while_prompting_ends_user_closed() {
    let server =
        FakeServer::start(vec![LegScript::nla(TestCert::generate("127.0.0.1"), USER, PASS)]).await.unwrap();
    let mut h = Harness::start(profile(ConnectMode::Headless, server.port(), None), PASS);
    h.wait_for("CertificatePrompt", WAIT, |e| matches!(e, SessionEvent::CertificatePrompt { .. })).await;
    h.handle.send(SessionCommand::Close).unwrap();
    assert_eq!(
        h.wait_terminal(WAIT).await,
        SessionState::Disconnected { reason: DisconnectReason::UserClosed }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_when_connected_ends_user_closed() {
    let cert = TestCert::generate("127.0.0.1");
    let server = FakeServer::start(vec![LegScript::nla(cert.clone(), USER, PASS)]).await.unwrap();
    let mut h = Harness::start(profile(ConnectMode::Headless, server.port(), Some(cert.fingerprint())), PASS);
    h.wait_for("Connected", WAIT, is_connected).await;
    h.handle.send(SessionCommand::Close).unwrap();
    assert_eq!(
        h.wait_terminal(WAIT).await,
        SessionState::Disconnected { reason: DisconnectReason::UserClosed }
    );
    assert!(server.log().legs[0].shutdown_requested, "Close sends a graceful Shutdown Request");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_close_ends_the_session_as_retryable() {
    let cert = TestCert::generate("127.0.0.1");
    let server = FakeServer::start(vec![
        LegScript::nla(cert.clone(), USER, PASS).then(drift_testkit::ServerAction::Close),
    ])
    .await
    .unwrap();
    let mut h = Harness::start(profile(ConnectMode::Headless, server.port(), Some(cert.fingerprint())), PASS);
    // Since M7-3 an established session that drops is reconnected instead of ending.
    let state = h
        .wait_for("Reconnecting", WAIT, |e| {
            matches!(e, SessionEvent::State(SessionState::Reconnecting { .. }))
        })
        .await;
    let SessionEvent::State(SessionState::Reconnecting { reason, attempt, .. }) = &state else {
        panic!("{state:?}")
    };
    assert_eq!(*attempt, 1);
    assert!(reason.is_retryable(), "a dropped transport is retryable: {reason:?}");
    let reason = reason.clone();
    h.handle.send(SessionCommand::Cancel).unwrap();
    assert_eq!(h.wait_terminal(WAIT).await, SessionState::Disconnected { reason });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_is_forwarded_as_fast_path() {
    let cert = TestCert::generate("127.0.0.1");
    let server = FakeServer::start(vec![LegScript::nla(cert.clone(), USER, PASS)]).await.unwrap();
    let mut h = Harness::start(profile(ConnectMode::Headless, server.port(), Some(cert.fingerprint())), PASS);
    h.wait_for("Connected", WAIT, is_connected).await;
    for ev in [
        drift_core::InputEvent::Key { scancode: 0x1C, extended: false, down: true },
        drift_core::InputEvent::Key { scancode: 0x1C, extended: false, down: false },
        drift_core::InputEvent::MouseMove { x: 10, y: 10 },
    ] {
        h.handle.send(SessionCommand::Input(ev)).unwrap();
    }
    for _ in 0..100 {
        if server.log().legs[0].fast_path_inputs >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(server.log().legs[0].fast_path_inputs >= 1, "{:?}", server.log());
    h.close().await;
}

// ---------------------------------------------------------------- injected EHOSTUNREACH

struct Unreachable;

impl Dialer for Unreachable {
    async fn dial(&self, _host: &str, _port: u16) -> io::Result<drift_rdp::connect::Dialed> {
        Err(io::Error::from_raw_os_error(libc_ehostunreach()))
    }
}

fn libc_ehostunreach() -> i32 {
    65 // EHOSTUNREACH on macOS (plan §1.8)
}

struct NoPrompt(Vec<ConnectStage>);

impl ConnectObserver for NoPrompt {
    fn stage(&mut self, stage: ConnectStage) {
        self.0.push(stage);
    }
    async fn decide_certificate(
        &mut self,
        _: &str,
        _: u16,
        _: CertFingerprint,
        _: CertificateRole,
    ) -> CertDecision {
        CertDecision::Reject
    }
}

fn nla_request(port: u16, expectation: CertExpectation) -> LegRequest {
    LegRequest {
        leg: 1,
        mode: ConnectMode::Headless,
        host: "127.0.0.1".into(),
        port,
        tls_server_name: "127.0.0.1".into(),
        auth: LegAuth::Nla { username: USER.into(), password: Zeroizing::new(PASS.into()) },
        expectation,
        role: CertificateRole::Server,
        routing_token: None,
        client_name: "drift-test".into(),
        desktop: connect::DEFAULT_DESKTOP,
        scale: 100,
    }
}

#[tokio::test]
async fn injected_ehostunreach_maps_to_local_network_denied() {
    let clock = ManualClock::new();
    let mut obs = NoPrompt(Vec::new());
    let res = connect::connect_leg(
        nla_request(3389, CertExpectation::TrustOnFirstUse),
        &Unreachable,
        &clock,
        Duration::from_secs(15),
        &mut obs,
        |c| c,
    )
    .await;
    assert_eq!(res.err(), Some(DisconnectReason::LocalNetworkDenied));
    assert_eq!(obs.0, vec![ConnectStage::Tcp]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_leg_reports_stages_and_leaf() {
    let cert = TestCert::generate("127.0.0.1");
    let server = FakeServer::start(vec![LegScript::nla(cert.clone(), USER, PASS)]).await.unwrap();
    let clock = ManualClock::new();
    let mut obs = NoPrompt(Vec::new());
    let leg = connect::connect_leg(
        nla_request(server.port(), CertExpectation::Pinned(cert.fingerprint())),
        &TokioDialer,
        &clock,
        Duration::from_secs(15),
        &mut obs,
        |c| c,
    )
    .await
    .expect("connects");
    assert_eq!(leg.leaf_der, cert.der());
    assert_eq!((leg.result.desktop_size.width, leg.result.desktop_size.height), (1280, 800));
    assert_eq!(obs.0, vec![ConnectStage::Tcp, ConnectStage::Tls, ConnectStage::Nla]);
    assert_eq!(server.log().legs[0].client_name.as_deref(), Some("drift-test"));
}

// ---------------------------------------------------------------- pure decisions

#[test]
fn certificate_check_table() {
    let a = TestCert::generate("a.example");
    let b = TestCert::generate("b.example");
    let cases = [
        (CertExpectation::Pinned(a.fingerprint()), a.der(), CertVerdict::Trusted),
        (CertExpectation::Pinned(a.fingerprint()), b.der(), CertVerdict::Mismatch),
        (CertExpectation::TrustOnFirstUse, a.der(), CertVerdict::Unknown(a.fingerprint())),
        (CertExpectation::Exact(a.der().to_vec()), a.der(), CertVerdict::Trusted),
        (CertExpectation::Exact(a.der().to_vec()), b.der(), CertVerdict::Mismatch),
    ];
    for (exp, leaf, want) in cases {
        assert_eq!(connect::check_certificate(&exp, leaf), want, "{exp:?}");
    }
}

#[test]
fn client_name_table() {
    let cases = [
        ("Hutsons-MacBook-Pro.local", "Hutsons-MacBook"),
        ("mini", "mini"),
        ("my mac.lan", "mymac"),
        ("", "drift"),
        (".local", "drift"),
        ("Ünïcode-Mac", "ncode-Mac"),
    ];
    for (host, want) in cases {
        assert_eq!(connect::client_name_from_hostname(host), want, "{host:?}");
    }
    let local = connect::local_client_name();
    assert!(!local.is_empty() && local.chars().count() <= connect::MAX_CLIENT_NAME_CHARS, "{local:?}");
}

#[test]
fn config_for_nla_leg() {
    let cfg = connect::build_config(&nla_request(3389, CertExpectation::TrustOnFirstUse));
    assert!(cfg.enable_tls && cfg.enable_credssp && !cfg.enable_standard_rdp_security);
    assert!(cfg.support_dyn_vc_gfx_protocol);
    assert_eq!(cfg.platform, MajorPlatformType::MACINTOSH);
    assert_eq!(cfg.client_name, "drift-test");
    assert!(!cfg.autologon);
    assert_eq!((cfg.desktop_size.width, cfg.desktop_size.height), (1280, 800));
    assert_eq!(cfg.desktop_scale_factor, 100);
}

#[test]
fn io_error_classification_table() {
    use ConnectStage::*;
    let e = |raw: i32| io::Error::from_raw_os_error(raw);
    let k = |kind: io::ErrorKind| io::Error::new(kind, "x");
    let cases: Vec<(io::Error, ConnectStage, DisconnectReason)> = vec![
        (e(65), Tcp, DisconnectReason::LocalNetworkDenied),
        (e(61), Tcp, DisconnectReason::Network), // ECONNREFUSED
        (e(60), Tcp, DisconnectReason::Timeout), // ETIMEDOUT
        (k(io::ErrorKind::TimedOut), Tls, DisconnectReason::Timeout),
        (k(io::ErrorKind::UnexpectedEof), Tls, DisconnectReason::TlsEof),
        (k(io::ErrorKind::UnexpectedEof), Activation, DisconnectReason::TlsEof),
        (k(io::ErrorKind::ConnectionReset), Activation, DisconnectReason::Network),
        (k(io::ErrorKind::UnexpectedEof), Nla, DisconnectReason::AuthFailed),
        (k(io::ErrorKind::ConnectionReset), Nla, DisconnectReason::AuthFailed),
    ];
    for (err, stage, want) in cases {
        assert_eq!(connect::classify_io_error(&err, stage), want, "{err:?} at {stage:?}");
    }
}

#[test]
fn connector_error_classification_table() {
    let rdstls =
        ConnectorError::new("RDSTLS", ConnectorErrorKind::RdstlsAuthFailed(RdstlsResultCode::LOGON_FAILURE));
    assert_eq!(
        connect::classify_connector_error(&rdstls, ConnectStage::Rdstls),
        DisconnectReason::RdstlsFailed(0x52E)
    );
    let denied = ConnectorError::new("x", ConnectorErrorKind::AccessDenied);
    assert_eq!(
        connect::classify_connector_error(&denied, ConnectStage::Activation),
        DisconnectReason::AuthFailed
    );
    let io_eof = ConnectorError::new("read frame by hint", ConnectorErrorKind::Custom)
        .with_source(io::Error::new(io::ErrorKind::UnexpectedEof, "eof"));
    assert_eq!(connect::classify_connector_error(&io_eof, ConnectStage::Nla), DisconnectReason::AuthFailed);
    assert_eq!(
        connect::classify_connector_error(&io_eof, ConnectStage::Activation),
        DisconnectReason::TlsEof
    );
    let unreachable = ConnectorError::new("connect", ConnectorErrorKind::Custom)
        .with_source(io::Error::from_raw_os_error(65));
    assert_eq!(
        connect::classify_connector_error(&unreachable, ConnectStage::Tcp),
        DisconnectReason::LocalNetworkDenied
    );
    let general = ConnectorError::new("weird", ConnectorErrorKind::General);
    assert!(matches!(
        connect::classify_connector_error(&general, ConnectStage::Activation),
        DisconnectReason::ProtocolError(_)
    ));
}
