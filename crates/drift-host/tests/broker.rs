//! HOST-13 Red: the pure parts of the daemon/agent split — which agent gets a connection, which
//! agents the daemon accepts, and the IPC framing between them.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;

use drift_host::broker::{
    AgentPolicy, AgentRefused, AgentTable, ConsoleState, RefuseReason, Route, SessionKind, route,
};
use drift_host::ipc::{IpcError, MAX_FRAME, Message, decode, encode};

const ALICE: u32 = 501;
const BOB: u32 = 502;

fn user(uid: u32) -> ConsoleState {
    ConsoleState::User { uid, name: format!("user{uid}") }
}

#[test]
fn the_console_users_aqua_agent_gets_the_connection() {
    let mut agents = AgentTable::default();
    let a = agents.register(SessionKind::Aqua, ALICE);
    let _b = agents.register(SessionKind::Aqua, BOB);
    let _lw = agents.register(SessionKind::LoginWindow, 0);
    assert_eq!(route(&user(ALICE), &agents), Route::Agent(a));
}

#[test]
fn at_the_login_window_the_login_window_agent_gets_it() {
    let mut agents = AgentTable::default();
    let _a = agents.register(SessionKind::Aqua, ALICE);
    let lw = agents.register(SessionKind::LoginWindow, 0);
    assert_eq!(route(&ConsoleState::LoginWindow, &agents), Route::Agent(lw));
}

#[test]
fn a_missing_agent_means_wait_then_the_newest_one_wins() {
    let mut agents = AgentTable::default();
    // Fast user switching: Bob is at the console but his agent has not started yet.
    let _a = agents.register(SessionKind::Aqua, ALICE);
    assert_eq!(route(&user(BOB), &agents), Route::Wait);
    let b1 = agents.register(SessionKind::Aqua, BOB);
    let b2 = agents.register(SessionKind::Aqua, BOB);
    assert_eq!(route(&user(BOB), &agents), Route::Agent(b2), "a restarted agent replaces the old one");
    agents.remove(b2);
    assert_eq!(route(&user(BOB), &agents), Route::Agent(b1));
    assert_eq!(route(&ConsoleState::LoginWindow, &agents), Route::Wait);
}

#[test]
fn an_unknown_console_is_refused_not_guessed() {
    let mut agents = AgentTable::default();
    let _a = agents.register(SessionKind::Aqua, ALICE);
    assert_eq!(route(&ConsoleState::Unknown, &agents), Route::Refuse(RefuseReason::NoConsole));
}

#[test]
fn the_daemon_only_admits_agents_that_are_who_they_claim() {
    let policy = AgentPolicy::system();
    // An Aqua agent runs as the user it serves, never as root.
    assert_eq!(policy.admit(SessionKind::Aqua, ALICE, ALICE), Ok(()));
    assert_eq!(policy.admit(SessionKind::Aqua, BOB, ALICE), Err(AgentRefused::UidMismatch));
    assert_eq!(policy.admit(SessionKind::Aqua, 0, 0), Err(AgentRefused::RootAqua));
    // The login-window agent is started by launchd as root.
    assert_eq!(policy.admit(SessionKind::LoginWindow, 0, 0), Ok(()));
    assert_eq!(policy.admit(SessionKind::LoginWindow, ALICE, 0), Err(AgentRefused::NotRoot));
    // Tests run as one unprivileged user: a test policy names it as the "root" stand-in.
    let test = AgentPolicy::with_root_uid(ALICE);
    assert_eq!(test.admit(SessionKind::LoginWindow, ALICE, ALICE), Ok(()));
}

#[test]
fn ipc_messages_round_trip() {
    let peer: SocketAddr = "192.0.2.7:51234".parse().unwrap();
    for message in [
        Message::Hello { kind: SessionKind::Aqua, uid: ALICE },
        Message::Hello { kind: SessionKind::LoginWindow, uid: 0 },
        Message::Welcome { agent: 7 },
        Message::Connection { peer },
        Message::Connection { peer: "[2001:db8::1]:3389".parse().unwrap() },
        Message::SaveDevices { toml: "[[devices]]\nname = \"iPad\"\n".into() },
        Message::Saved,
        Message::Error { text: "no".into() },
    ] {
        let bytes = encode(&message);
        assert_eq!(decode(&bytes[4..]).unwrap(), message, "{message:?}");
        assert_eq!(u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize, bytes.len() - 4);
    }
}

#[test]
fn malformed_ipc_frames_are_errors_not_panics() {
    assert_eq!(decode(&[]), Err(IpcError::Truncated));
    assert_eq!(decode(&[0xEE]), Err(IpcError::UnknownKind(0xEE)));
    let mut hello = encode(&Message::Hello { kind: SessionKind::Aqua, uid: ALICE })[4..].to_vec();
    hello.push(0);
    assert_eq!(decode(&hello), Err(IpcError::TrailingBytes));
    let mut bad_kind = encode(&Message::Hello { kind: SessionKind::Aqua, uid: ALICE })[4..].to_vec();
    bad_kind[1] = 9;
    assert!(matches!(decode(&bad_kind), Err(IpcError::Invalid(_))));
    // Every prefix of every message is rejected cleanly.
    let save = encode(&Message::SaveDevices { toml: "x".repeat(40) });
    for n in 0..save.len() - 4 {
        assert!(decode(&save[4..4 + n]).is_err(), "prefix {n}");
    }
    assert!(MAX_FRAME >= 64 * 1024, "room for a devices file");
}
