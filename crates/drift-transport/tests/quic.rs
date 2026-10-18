//! HOST-12 Red: RDP over QUIC between Drift ends. The RDP connection rides one reliable
//! bidirectional stream; each GFX frame rides its own unidirectional stream, so a lost packet
//! delays only its own frame. The client pins the host certificate (ADR IOS-0 decision 7).
//!
//! Runs through `LossyUdpProxy` at 5 % loss: everything still arrives intact.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::time::Duration;

use drift_testkit::TestCert;
use drift_transport::lossy::{LinkConfig, LossyUdpProxy};
use drift_transport::quic::{QuicError, QuicHost, connect};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn host(cert: &TestCert) -> QuicHost {
    QuicHost::bind("127.0.0.1:0".parse().unwrap(), cert.der().to_vec(), cert.key_pkcs8().to_vec()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn main_stream_and_frames_survive_a_lossy_link() {
    let cert = TestCert::generate("drift-host.local");
    let host = host(&cert);
    let link = LinkConfig {
        loss_percent: 5.0,
        one_way_delay: Duration::from_millis(5),
        jitter: Duration::ZERO,
        seed: 3,
    };
    let proxy = LossyUdpProxy::start(host.local_addr(), link).await.unwrap();

    let server = tokio::spawn(async move {
        let mut session = host.accept().await.unwrap();
        // Echo the RDP byte stream once, then send 50 frames.
        let mut buf = vec![0u8; 64 * 1024];
        session.main_recv().read_exact(&mut buf).await.unwrap();
        session.main_send().write_all(&buf).await.unwrap();
        for id in 0..50u32 {
            let frame = vec![u8::try_from(id % 251).unwrap(); 20_000];
            session.send_frame(id, &frame).await.unwrap();
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        drop(host);
    });

    let mut client = connect(proxy.addr(), "drift-host.local", cert.fingerprint()).await.unwrap();
    let sent: Vec<u8> = (0..64 * 1024).map(|i| u8::try_from(i % 256).unwrap()).collect();
    client.main_send().write_all(&sent).await.unwrap();
    let mut echoed = vec![0u8; sent.len()];
    client.main_recv().read_exact(&mut echoed).await.unwrap();
    assert_eq!(echoed, sent, "the RDP byte stream is reliable and ordered");

    let mut frames = BTreeMap::new();
    while frames.len() < 50 {
        let (id, data) = tokio::time::timeout(Duration::from_secs(10), client.recv_frame())
            .await
            .expect("frames keep arriving")
            .expect("the session is open");
        assert_eq!(data.len(), 20_000);
        assert!(data.iter().all(|b| *b == u8::try_from(id % 251).unwrap()), "frame {id} intact");
        frames.insert(id, ());
    }
    assert!(proxy.stats().dropped > 0, "the link really dropped packets: {:?}", proxy.stats());
    server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_with_another_certificate_is_refused() {
    let cert = TestCert::generate("drift-host.local");
    let other = TestCert::generate("drift-host.local");
    let host = host(&cert);
    let addr = host.local_addr();
    tokio::spawn(async move {
        let _ = host.accept().await;
    });
    let err = connect(addr, "drift-host.local", other.fingerprint()).await.err().expect("pin mismatch");
    assert!(matches!(err, QuicError::PinMismatch { .. }), "{err:?}");
}
