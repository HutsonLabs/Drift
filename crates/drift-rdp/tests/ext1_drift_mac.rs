//! EXT-1 Red: the session negotiates `Drift::Mac::v1` when the host opens it, and stays plain
//! RDP when it does not (loopback FakeServer).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{Harness, PASS, USER, is_connected, options, profile};
use drift_core::{ConnectMode, SystemClock};
use drift_ext::channel::{Link, LinkState};
use drift_rdp::SessionSecrets;
use drift_testkit::{Channels, FakeServer, LegScript, TestCert};

const WAIT: Duration = Duration::from_secs(20);

async fn run(channels: Channels) -> (Link, FakeServer, Harness) {
    let cert = TestCert::generate("drift-mac.test");
    let leg = LegScript::nla(cert.clone(), USER, PASS).with_channels(channels);
    let server = FakeServer::start(vec![leg]).await.unwrap();
    let link = Link::new();
    let mut opts = options();
    opts.drift_mac = Some(link.clone());
    let mut h = Harness::start_full(
        profile(ConnectMode::Headless, server.port(), Some(cert.fingerprint())),
        SessionSecrets::new(PASS),
        Arc::new(SystemClock),
        opts,
    );
    h.wait_for("connected", WAIT, is_connected).await;
    (link, server, h)
}

async fn settle(link: &Link, until: impl Fn(&LinkState) -> bool) -> LinkState {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let s = link.state();
        if until(&s) || Instant::now() > deadline {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_drift_host_negotiates_the_channel() {
    let (link, server, h) = run(Channels::all().with_drift_mac()).await;
    let state = settle(&link, |s| matches!(s, LinkState::Negotiated(_))).await;
    assert!(matches!(state, LinkState::Negotiated(n) if n.version == 1), "{state:?}");
    let legs = server.log().legs;
    assert!(legs[0].drift_mac_negotiated.is_some(), "the host side negotiated too: {:?}", legs[0].error);
    h.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn other_hosts_leave_it_absent_and_the_session_works() {
    let (link, _server, h) = run(Channels::all()).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(link.state(), LinkState::Absent, "plain RDP: the channel was never opened");
    h.close().await;
}
