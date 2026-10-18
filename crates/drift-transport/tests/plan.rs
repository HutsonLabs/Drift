//! HOST-12 Red: the client tries QUIC first with a paired Drift Host and falls back to TCP;
//! every other host gets TCP only (ADR IOS-0 decision 6).
#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use drift_transport::plan::{Attempt, Transport, connect_with_fallback, transports_for};

#[test]
fn a_paired_drift_host_gets_quic_then_tcp_and_everything_else_tcp() {
    assert_eq!(transports_for(true), vec![Transport::Quic, Transport::Tcp]);
    assert_eq!(transports_for(false), vec![Transport::Tcp]);
}

#[tokio::test]
async fn a_failed_quic_attempt_falls_back_to_tcp() {
    let tried = Arc::new(Mutex::new(Vec::new()));
    let t = tried.clone();
    let got = connect_with_fallback(&transports_for(true), Duration::from_secs(1), move |transport| {
        let t = t.clone();
        async move {
            t.lock().unwrap().push(transport);
            match transport {
                Transport::Quic => Err("udp blocked".to_owned()),
                Transport::Tcp => Ok("tcp session"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        got,
        Attempt {
            transport: Transport::Tcp,
            value: "tcp session",
            failures: vec![(Transport::Quic, "udp blocked".to_owned())]
        }
    );
    assert_eq!(*tried.lock().unwrap(), vec![Transport::Quic, Transport::Tcp]);
}

#[tokio::test]
async fn a_silent_quic_attempt_times_out_and_falls_back() {
    let got =
        connect_with_fallback(&transports_for(true), Duration::from_millis(100), |transport| async move {
            match transport {
                Transport::Quic => {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok("never")
                }
                Transport::Tcp => Ok("tcp"),
            }
        })
        .await
        .unwrap();
    assert_eq!(got.transport, Transport::Tcp);
    assert_eq!(got.failures.len(), 1);
}

#[tokio::test]
async fn when_every_transport_fails_the_last_error_is_returned() {
    let err = connect_with_fallback(&transports_for(true), Duration::from_secs(1), |t| async move {
        Err::<(), _>(format!("{t:?} down"))
    })
    .await
    .unwrap_err();
    assert_eq!(err, vec![(Transport::Quic, "Quic down".to_owned()), (Transport::Tcp, "Tcp down".to_owned())]);
}
