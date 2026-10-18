//! HOST-7 Red: link hygiene — the interactive-video service class on the session socket, an idle
//! heartbeat that keeps a phone's Wi-Fi radio awake, and a backlog signal for EXT-4.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::io::Write as _;
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd as _;
use std::time::{Duration, Instant};

use drift_host::link::{
    Backlog, BacklogMeter, BacklogSample, IdleHeartbeat, NET_SERVICE_TYPE_VI, SocketProbe, service_class,
    set_service_class,
};

fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    (server, client)
}

#[test]
fn the_session_socket_gets_the_interactive_video_service_class() {
    let (server, _client) = pair();
    set_service_class(server.as_raw_fd(), NET_SERVICE_TYPE_VI).unwrap();
    assert_eq!(service_class(server.as_raw_fd()).unwrap(), NET_SERVICE_TYPE_VI);
}

#[test]
fn the_probe_sees_bytes_the_peer_has_not_taken_yet() {
    let (mut server, client) = pair();
    let probe = SocketProbe::new(server.as_raw_fd()).unwrap();
    assert_eq!(probe.unsent_bytes().unwrap(), 0);
    // The client never reads: once its receive buffer is full, data waits in ours.
    server.set_nonblocking(true).unwrap();
    let chunk = vec![0u8; 64 * 1024];
    for _ in 0..256 {
        if server.write(&chunk).is_err() {
            break;
        }
    }
    assert!(probe.unsent_bytes().unwrap() > 0);
    drop(client);
}

#[test]
fn the_heartbeat_is_due_only_after_an_idle_interval_and_repeats() {
    let start = Instant::now();
    let mut hb = IdleHeartbeat::new(Duration::from_millis(100));
    hb.sent(start);
    assert!(!hb.due(start + Duration::from_millis(99)));
    assert!(hb.due(start + Duration::from_millis(100)));
    hb.sent(start + Duration::from_millis(100));
    assert!(!hb.due(start + Duration::from_millis(150)), "any output restarts the clock");
    assert!(hb.due(start + Duration::from_millis(200)), "and an idle link gets one every interval");
}

fn sample(in_flight: u32, unsent: u64) -> BacklogSample {
    BacklogSample { frames_in_flight: in_flight, unsent_bytes: unsent }
}

#[test]
fn backlog_rises_after_sustained_pressure_and_clears_with_hysteresis() {
    let mut meter = BacklogMeter::default();
    assert_eq!(meter.update(sample(0, 0)), Backlog::Clear);
    // One bad sample is noise.
    assert_eq!(meter.update(sample(6, 1 << 20)), Backlog::Clear);
    assert_eq!(meter.update(sample(0, 0)), Backlog::Clear);
    // Sustained pressure: building, then backlogged.
    for _ in 0..3 {
        meter.update(sample(3, 100_000));
    }
    assert_eq!(meter.level(), Backlog::Building);
    for _ in 0..3 {
        meter.update(sample(6, 1 << 20));
    }
    assert_eq!(meter.level(), Backlog::Backlogged);
    // It only clears after several quiet samples in a row.
    meter.update(sample(0, 0));
    assert_eq!(meter.level(), Backlog::Backlogged);
    for _ in 0..5 {
        meter.update(sample(0, 0));
    }
    assert_eq!(meter.level(), Backlog::Clear);
    assert_eq!(meter.transitions(), 3, "clear → building → backlogged → clear");
}
