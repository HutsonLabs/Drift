//! HOST-13 Red: the daemon hands accepted TCP connections to agents over a Unix socket
//! (SCM_RIGHTS), checks who the agent is (`getpeereid`), and Drift's client reaches the agent's
//! session through the daemon's port. Pairing writes go through the daemon, which owns the store.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read as _, Write as _};
use std::os::fd::AsFd as _;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use drift_core::{ConnectMode, ConnectionProfile, SessionState, Size, SystemClock};
use drift_host::agent::AgentLink;
use drift_host::broker::{AgentPolicy, ConsoleState, SessionKind};
use drift_host::daemon::{Daemon, DaemonOptions};
use drift_host::fdpass::{peer_uid, recv_with_fd, send_with_fd};
use drift_host::frames::TestPattern;
use drift_host::identity::HostIdentity;
use drift_host::input::RecordingInput;
use drift_host::server::{Host, HostOptions};
use drift_rdp::{SessionEvent, SessionOptions, SessionSecrets};
use drift_testkit::{FrameSinkCall, PresentMode, RecordingFrameSink};

const USER: &str = "drift-host-test";
const PASS: &str = "fake-host13-password";

fn me() -> u32 {
    // SAFETY: getuid has no preconditions.
    unsafe { libc::getuid() }
}

#[test]
fn a_tcp_connection_crosses_a_unix_socket_and_keeps_working() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (accepted, _) = listener.accept().unwrap();
    let (daemon_end, agent_end) = UnixStream::pair().unwrap();
    send_with_fd(&daemon_end, b"conn", Some(accepted.as_fd())).unwrap();
    drop(accepted);
    let (bytes, fd) = recv_with_fd(&agent_end).unwrap();
    assert_eq!(bytes, b"conn");
    let mut received = std::net::TcpStream::from(fd.expect("a descriptor came along"));
    client.write_all(b"hello agent").unwrap();
    let mut buf = [0_u8; 11];
    received.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"hello agent");
    assert_eq!(peer_uid(&agent_end).unwrap(), me());
    // A message without a descriptor carries none.
    send_with_fd(&daemon_end, b"plain", None).unwrap();
    let (bytes, fd) = recv_with_fd(&agent_end).unwrap();
    assert_eq!((bytes.as_slice(), fd.is_none()), (&b"plain"[..], true));
}

#[test]
fn the_console_is_someone_or_the_login_window() {
    // On a dev Mac or CI runner somebody is logged in; never Unknown.
    let state = drift_host::console::console_state();
    assert!(!matches!(state, ConsoleState::Unknown), "{state:?}");
}

fn profile(port: u16, identity: &HostIdentity) -> ConnectionProfile {
    let mut p = ConnectionProfile::new("Drift Host", "127.0.0.1", ConnectMode::Headless);
    p.port = port;
    USER.clone_into(&mut p.rdp_username);
    p.cert_pin = Some(identity.fingerprint());
    p
}

async fn wait_until(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn drift_reaches_the_console_agent_through_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("broker.sock");
    let console = me();
    let daemon = Daemon::start(DaemonOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        socket: socket.clone(),
        store_dir: dir.path().join("store"),
        policy: AgentPolicy::with_root_uid(me()),
        console: Box::new(move || ConsoleState::User { uid: console, name: "tester".into() }),
        wait_for_agent: Duration::from_secs(5),
    })
    .unwrap();

    // The user's agent registers, then runs the RDP session on what the daemon hands it.
    let link = AgentLink::connect(&socket, SessionKind::Aqua).unwrap();
    let identity = HostIdentity::load_or_create(dir.path()).unwrap();
    let input = RecordingInput::default();
    let host = Host::start_with(
        HostOptions {
            listen: "127.0.0.1:0".parse().unwrap(),
            identity: identity.clone(),
            username: USER.into(),
            password: zeroize::Zeroizing::new(PASS.to_owned()),
            desktop: Size::new(1024, 640),
            frames: Box::new(|| Box::new(TestPattern::new())),
            live: None,
            input: Box::new(input.clone()),
            clipboard: None,
            audio: None,
            displays: Box::new(drift_host::display::PhysicalOnly),
            pairing: None,
            menus: None,
        },
        link.incoming(),
    )
    .unwrap();
    wait_until("the agent registered", Duration::from_secs(5), || daemon.stats().agents == 1).await;

    let (sink, frames) = RecordingFrameSink::new(PresentMode::Immediate);
    let (handle, mut events) = drift_rdp::spawn_session(
        profile(daemon.addr().port(), &identity),
        SessionSecrets::new(PASS),
        Box::new(sink),
        Arc::new(SystemClock),
        SessionOptions {
            connect_timeout: Duration::from_secs(15),
            reconnect_seed: Some(3),
            ..SessionOptions::default()
        },
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Some(SessionEvent::State(SessionState::Connected { .. }))) => break,
            Ok(Some(_)) => {}
            other => panic!("not connected through the daemon: {other:?}"),
        }
    }
    wait_until("the pattern through the daemon", Duration::from_secs(15), || {
        frames.calls().iter().any(|c| matches!(c, FrameSinkCall::BlitNv12 { .. }))
    })
    .await;
    assert_eq!(daemon.stats().handed, 1);
    assert_eq!(host.stats().sessions_served, 1);

    // Pairing writes go through the daemon, which owns devices.toml.
    let text = "[[device]]\nid = \"6f1c7f0e-1111-4222-8333-944455556666\"\nname = \"iPad\"\nkey = \"04ab\"\npaired_at = 1\n";
    link.save_devices(text).unwrap();
    let saved = std::fs::read_to_string(dir.path().join("store").join("devices.toml")).unwrap();
    assert!(saved.contains("iPad"), "{saved}");
    assert!(link.save_devices("not toml {{{").is_err(), "the daemon refuses a malformed list");

    drop(handle);
    host.stop();
    daemon.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_agent_for_the_console_the_connection_is_dropped_after_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(DaemonOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        socket: dir.path().join("broker.sock"),
        store_dir: dir.path().join("store"),
        policy: AgentPolicy::with_root_uid(me()),
        console: Box::new(|| ConsoleState::LoginWindow),
        wait_for_agent: Duration::from_millis(300),
    })
    .unwrap();
    let mut client = std::net::TcpStream::connect(daemon.addr()).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = [0_u8; 1];
    assert_eq!(client.read(&mut buf).unwrap(), 0, "closed, not left hanging");
    assert_eq!(daemon.stats().dropped_no_agent, 1);
    daemon.stop();
}

#[test]
fn an_agent_claiming_another_user_is_turned_away() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("broker.sock");
    let daemon = Daemon::start(DaemonOptions {
        listen: "127.0.0.1:0".parse().unwrap(),
        socket: socket.clone(),
        store_dir: dir.path().join("store"),
        policy: AgentPolicy::system(),
        console: Box::new(|| ConsoleState::LoginWindow),
        wait_for_agent: Duration::from_millis(100),
    })
    .unwrap();
    // As an unprivileged user, claiming the login window (root) must fail.
    assert!(AgentLink::connect(&socket, SessionKind::LoginWindow).is_err());
    assert_eq!(daemon.stats().agents, 0);
    daemon.stop();
}
