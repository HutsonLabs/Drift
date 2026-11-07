//! M3-1 Red tests: the Server Redirection loop (plan §6 M3-1).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::{Harness, PASS, USER, is_connected, profile};
use drift_core::{ConnectMode, ConnectStage, DesktopSize, DisconnectReason, SessionState};
use drift_rdp::SessionEvent;
use drift_rdp::rdstls::OneTimeCredentials;
use drift_rdp::redirect::{self, RedirectLoop};
use drift_testkit::{FakeServer, LegScript, ServerAction, TestCert, redirection_pdu};
use ironrdp_pdu::rdp::server_redirection::ServerRedirectionPdu;

const WAIT: Duration = Duration::from_secs(20);
const DESK: DesktopSize = DesktopSize { width: 1280, height: 800 };

fn redirect_after(ms: u64, pdu: ServerRedirectionPdu) -> [ServerAction; 2] {
    [ServerAction::Wait(Duration::from_millis(ms)), ServerAction::Redirect(Box::new(pdu))]
}

// ---------------------------------------------------------------- FSM (pure)

#[test]
fn remote_login_leg_sequencing() {
    let cert = TestCert::generate("127.0.0.1");
    let mut rl = RedirectLoop::new(ConnectMode::RemoteLogin, "10.0.2.40", 3389);
    assert_eq!(rl.leg(), 1);
    assert_eq!(rl.on_activated(DESK, 100), None, "leg 1 of Remote Login waits for the redirect");

    let mut pdu = redirection_pdu(1_234_567, cert.der(), 1);
    let expected_user = pdu.username.clone().unwrap();
    let next = rl.on_redirect(&mut pdu).expect("first redirect");
    assert_eq!((next.leg, next.host.as_str(), next.port), (2, "10.0.2.40", 3389));
    assert_eq!(next.routing_token.as_deref(), Some("Cookie: msts=1234567"));
    assert_eq!(next.target_certificate.as_deref(), Some(cert.der()));
    assert_eq!(next.credentials.username(), expected_user);
    assert_eq!(pdu.password, None, "one-time password is cleared from the PDU");
    assert_eq!(pdu.username, None);
    assert_eq!(pdu.redirection_guid, None);
    assert_eq!(rl.leg(), 2);
    assert_eq!(rl.on_activated(DESK, 100), Some(SessionState::AwaitingGreeterLogin));

    let next = rl.on_redirect(&mut redirection_pdu(1_234_567, cert.der(), 2)).expect("second redirect");
    assert_eq!(next.leg, 3);
    assert_eq!(rl.on_activated(DESK, 100), Some(SessionState::Connected { desktop: DESK, scale: 100 }));
}

#[test]
fn headless_is_connected_on_every_leg() {
    let cert = TestCert::generate("127.0.0.1");
    let mut rl = RedirectLoop::new(ConnectMode::Headless, "h", 3392);
    assert_eq!(rl.on_activated(DESK, 200), Some(SessionState::Connected { desktop: DESK, scale: 200 }));
    rl.on_redirect(&mut redirection_pdu(1, cert.der(), 1)).unwrap();
    assert_eq!(rl.on_activated(DESK, 100), Some(SessionState::Connected { desktop: DESK, scale: 100 }));
}

#[test]
fn fifth_redirect_is_a_loop() {
    let cert = TestCert::generate("127.0.0.1");
    let mut rl = RedirectLoop::new(ConnectMode::RemoteLogin, "h", 3389);
    for i in 0..4u8 {
        let next = rl.on_redirect(&mut redirection_pdu(7, cert.der(), i)).expect("within the cap");
        assert_eq!(next.leg, i + 2);
    }
    assert_eq!(
        rl.on_redirect(&mut redirection_pdu(7, cert.der(), 9)).err(),
        Some(DisconnectReason::RedirectLoop)
    );
}

#[test]
fn target_net_address_overrides_host() {
    let cert = TestCert::generate("127.0.0.1");
    let mut pdu = redirection_pdu(7, cert.der(), 1);
    pdu.target_net_address = Some("10.9.8.7".into());
    pdu.redirection_flags |= ironrdp_pdu::rdp::server_redirection::ServerRedirectionFlags::TARGET_NET_ADDRESS;
    let next = RedirectLoop::new(ConnectMode::RemoteLogin, "h", 3389).on_redirect(&mut pdu).unwrap();
    assert_eq!((next.host.as_str(), next.port), ("10.9.8.7", 3389));
}

#[test]
fn redirect_without_credentials_is_a_protocol_error() {
    let cert = TestCert::generate("127.0.0.1");
    let mut pdu = redirection_pdu(7, cert.der(), 1);
    pdu.password = None;
    let err = RedirectLoop::new(ConnectMode::RemoteLogin, "h", 3389).on_redirect(&mut pdu).err();
    assert!(matches!(err, Some(DisconnectReason::ProtocolError(_))), "{err:?}");
}

#[test]
fn malformed_target_certificate_is_a_protocol_error() {
    let cert = TestCert::generate("127.0.0.1");
    let mut pdu = redirection_pdu(7, cert.der(), 1);
    pdu.target_certificate = Some(vec![0x41, 0x00, 0x42]); // odd UTF-16 length
    let err = RedirectLoop::new(ConnectMode::RemoteLogin, "h", 3389).on_redirect(&mut pdu).err();
    assert!(matches!(err, Some(DisconnectReason::ProtocolError(_))), "{err:?}");
}

/// The sanitized g-r-d capture (plan §1.3): the target certificate inside the redirection PDU is
/// byte-identical to the TLS certificate presented on leg 2.
#[test]
fn captured_grd_redirection_yields_the_leg2_certificate() {
    use drift_testkit::fixtures::{self, names};
    use ironrdp_pdu::mcs::SendDataIndication;
    use ironrdp_pdu::rdp::headers::{ShareControlHeader, ShareControlPdu};
    use ironrdp_pdu::x224::X224;

    let frame = fixtures::read(names::SERVER_REDIRECTION_LEG1);
    let X224(sdi) = ironrdp_core::decode::<X224<SendDataIndication<'_>>>(&frame).expect("X.224 frame");
    let header = ironrdp_core::decode::<ShareControlHeader>(sdi.user_data.as_ref()).expect("share control");
    let ShareControlPdu::ServerRedirect(mut pdu) = header.share_control_pdu else {
        panic!("not a Server Redirection PDU")
    };
    let mut rl = RedirectLoop::new(ConnectMode::RemoteLogin, "10.0.2.40", 3389);
    let next = rl.on_redirect(&mut pdu).expect("captured redirect is followed");
    assert_eq!((next.leg, next.host.as_str(), next.port), (2, "10.0.2.40", 3389), "no TargetNetAddress");
    let token = next.routing_token.expect("routing token");
    assert!(token.starts_with("Cookie: msts=") && !token.ends_with('\n'), "{token:?}");
    assert_eq!(next.target_certificate.as_deref(), Some(fixtures::read(names::TLS_CERT_LEG2).as_slice()));
    assert_eq!(
        redirect::verify_target_certificate(
            &fixtures::read(names::TLS_CERT_LEG2),
            &fixtures::read(names::TLS_CERT_LEG2)
        ),
        Ok(())
    );
}

#[test]
fn routing_token_strips_crlf() {
    assert_eq!(
        redirect::routing_token(b"Cookie: msts=3640205228\r\n").as_deref(),
        Some("Cookie: msts=3640205228")
    );
    assert_eq!(redirect::routing_token(b"Cookie: msts=1\r\n\0").as_deref(), Some("Cookie: msts=1"));
    assert_eq!(redirect::routing_token(b"").as_deref(), None);
    assert_eq!(redirect::routing_token(&[0xff, 0xfe]).as_deref(), None, "non-ASCII token rejected");
}

#[test]
fn target_certificate_verification() {
    let a = TestCert::generate("a");
    let b = TestCert::generate("b");
    assert_eq!(redirect::verify_target_certificate(a.der(), a.der()), Ok(()));
    assert_eq!(redirect::verify_target_certificate(a.der(), b.der()), Err(DisconnectReason::CertMismatch));
}

#[test]
fn one_time_credentials_from_pdu_are_redacted() {
    let cert = TestCert::generate("127.0.0.1");
    let pdu = redirection_pdu(7, cert.der(), 3);
    let creds = OneTimeCredentials::from_redirection(&pdu).expect("complete PDU");
    let user = pdu.username.clone().unwrap();
    assert_eq!(creds.username(), user);
    let dbg = format!("{creds:?}");
    assert!(!dbg.contains(&user), "{dbg}");
    let conn = creds.into_connector();
    assert_eq!(Some(conn.password.clone()), pdu.password);
    assert_eq!(Some(conn.redirection_guid.clone()), pdu.redirection_guid);
    assert_eq!(conn.domain, "");
}

// ---------------------------------------------------------------- loopback

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_login_three_legs_over_loopback() {
    let cert = TestCert::generate("127.0.0.1");
    let pdu1 = redirection_pdu(424_242, cert.der(), 1);
    let pdu2 = redirection_pdu(424_242, cert.der(), 2);
    let server = FakeServer::start(vec![
        LegScript {
            actions: redirect_after(50, pdu1.clone()).into(),
            ..LegScript::nla(cert.clone(), USER, PASS)
        },
        LegScript { actions: redirect_after(300, pdu2.clone()).into(), ..LegScript::rdstls(cert.clone(), 0) },
        LegScript::rdstls(cert.clone(), 0),
    ])
    .await
    .unwrap();
    let mut h =
        Harness::start(profile(ConnectMode::RemoteLogin, server.port(), Some(cert.fingerprint())), PASS);

    h.wait_for("AwaitingGreeterLogin", WAIT, |e| {
        *e == SessionEvent::State(SessionState::AwaitingGreeterLogin)
    })
    .await;
    h.wait_for("Connected", WAIT, is_connected).await;

    let states = h.states();
    let legs: Vec<u8> = states
        .iter()
        .filter_map(|s| match s {
            SessionState::Connecting { leg, stage: ConnectStage::Tcp } => Some(*leg),
            _ => None,
        })
        .collect();
    assert_eq!(legs, vec![1, 2, 3], "{states:?}");
    assert!(states.contains(&SessionState::Connecting { leg: 2, stage: ConnectStage::Rdstls }), "{states:?}");
    let greeter = states.iter().position(|s| *s == SessionState::AwaitingGreeterLogin).unwrap();
    let leg3 = states
        .iter()
        .position(|s| *s == SessionState::Connecting { leg: 3, stage: ConnectStage::Tcp })
        .unwrap();
    assert!(greeter < leg3);
    assert_eq!(states.last(), Some(&SessionState::Connected { desktop: DESK, scale: 100 }));
    assert!(
        !h.seen.iter().any(|e| matches!(e, SessionEvent::CertificatePrompt { .. })),
        "target cert is trusted"
    );

    let log = server.log();
    assert_eq!(log.legs.len(), 3);
    assert_eq!(log.legs[0].routing_token.as_deref(), Some(&*format!("Cookie: mstshash={USER}")));
    for (i, pdu) in [(1, &pdu1), (2, &pdu2)] {
        let leg = &log.legs[i];
        assert_eq!(leg.requested_protocols, 0x5, "SSL|RDSTLS on leg {}", i + 1);
        assert_eq!(leg.routing_token.as_deref(), Some("Cookie: msts=424242"));
        let req = leg.rdstls_request.as_ref().expect("RDSTLS request");
        assert_eq!(Some(&req.username), pdu.username.as_ref());
        assert_eq!(Some(&req.password), pdu.password.as_ref());
        assert_eq!(Some(&req.redirection_guid), pdu.redirection_guid.as_ref());
        assert_eq!(req.domain, "");
        assert!(leg.activated);
    }
    h.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_cert_mismatch_fails_with_cert_mismatch() {
    let cert = TestCert::generate("127.0.0.1");
    let imposter = TestCert::generate("127.0.0.1");
    let server = FakeServer::start(vec![
        LegScript {
            actions: redirect_after(50, redirection_pdu(9, cert.der(), 1)).into(),
            ..LegScript::nla(cert.clone(), USER, PASS)
        },
        LegScript::rdstls(imposter, 0),
    ])
    .await
    .unwrap();
    let mut h =
        Harness::start(profile(ConnectMode::RemoteLogin, server.port(), Some(cert.fingerprint())), PASS);
    assert_eq!(h.wait_terminal(WAIT).await, SessionState::Failed { reason: DisconnectReason::CertMismatch });
    let log = server.log();
    assert_eq!(log.legs.len(), 2);
    assert_eq!(log.legs[1].rdstls_request, None, "one-time credentials never reach an imposter");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rdstls_logon_failure_is_not_retried() {
    let cert = TestCert::generate("127.0.0.1");
    let server = FakeServer::start(vec![
        LegScript {
            actions: redirect_after(50, redirection_pdu(9, cert.der(), 1)).into(),
            ..LegScript::nla(cert.clone(), USER, PASS)
        },
        LegScript::rdstls(cert.clone(), 0x52E),
        LegScript::rdstls(cert.clone(), 0),
    ])
    .await
    .unwrap();
    let mut h =
        Harness::start(profile(ConnectMode::RemoteLogin, server.port(), Some(cert.fingerprint())), PASS);
    assert_eq!(
        h.wait_terminal(WAIT).await,
        SessionState::Failed { reason: DisconnectReason::RdstlsFailed(0x52E) }
    );
    let _ = h.drain_for(Duration::from_millis(500)).await;
    assert_eq!(server.log().legs.len(), 2, "no retry after RDSTLS 0x52E");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_loop_is_capped() {
    let cert = TestCert::generate("127.0.0.1");
    let mut legs = vec![LegScript {
        actions: redirect_after(20, redirection_pdu(5, cert.der(), 0)).into(),
        ..LegScript::nla(cert.clone(), USER, PASS)
    }];
    for i in 1..=5u8 {
        legs.push(LegScript {
            actions: redirect_after(20, redirection_pdu(5, cert.der(), i)).into(),
            ..LegScript::rdstls(cert.clone(), 0)
        });
    }
    let server = FakeServer::start(legs).await.unwrap();
    let mut h =
        Harness::start(profile(ConnectMode::RemoteLogin, server.port(), Some(cert.fingerprint())), PASS);
    assert_eq!(h.wait_terminal(WAIT).await, SessionState::Failed { reason: DisconnectReason::RedirectLoop });
    assert_eq!(server.log().legs.len(), 5, "legs 1..=5, the fifth redirect is refused");
}
