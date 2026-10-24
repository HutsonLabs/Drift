//! WIZ-3 Red: `drift-ssh` against an in-process SSH server (`drift_testkit::ssh_server`): device key, strict host
//! keys with an explicit trust, password-once key install, and commands with stdin, streamed
//! output and exit status (ADR `GNOME-WIZARD-headless-setup`).

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use drift_ssh::{Diagnosis, KeyStore, Target, Trust, connect, install_device_key};
use drift_testkit::ssh_server::{ServerOptions, TestServer};

const COMMENT: &str = "drift@test";

fn target(server: &TestServer) -> Target {
    Target { host: "127.0.0.1".into(), port: server.port, user: "tester".into() }
}

fn store(dir: &Path) -> KeyStore {
    KeyStore::open(dir.join("ssh")).unwrap_or_else(|e| panic!("key store: {e}"))
}

/// `home/.ssh/authorized_keys` with the store's device key (what a set-up account has).
fn authorize(keys: &KeyStore, home: &Path) {
    let line = keys.public_key(COMMENT).unwrap_or_else(|e| panic!("{e}"));
    std::fs::create_dir_all(home.join(".ssh")).expect("~/.ssh");
    std::fs::write(home.join(".ssh/authorized_keys"), format!("{line}\n")).expect("authorized_keys");
}

struct Machine {
    _dir: tempfile::TempDir,
    keys: KeyStore,
    home: std::path::PathBuf,
}

fn machine() -> Machine {
    let dir = tempfile::tempdir().expect("tempdir");
    let keys = store(dir.path());
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    Machine { _dir: dir, keys, home }
}

async fn run(conn: &drift_ssh::Connection, command: &str, stdin: &str) -> (Vec<String>, Option<u32>) {
    let mut lines = Vec::new();
    let status = conn
        .run(command, stdin.as_bytes(), &mut |l| lines.push(l.to_owned()))
        .await
        .unwrap_or_else(|e| panic!("{command}: {e}"));
    (lines, status)
}

#[test]
fn the_device_key_is_made_once_and_kept_private() {
    let dir = tempfile::tempdir().expect("tempdir");
    let keys = store(dir.path());
    let first = keys.public_key(COMMENT).unwrap_or_else(|e| panic!("{e}"));
    assert!(first.starts_with("ssh-ed25519 AAAA"), "{first}");
    assert!(first.ends_with(" drift@test"), "{first}");
    assert_eq!(keys.public_key("other comment").ok(), Some(first.clone()), "made once");
    assert_eq!(store(dir.path()).public_key(COMMENT).ok(), Some(first), "kept across opens");

    let mode = |p: &Path| std::fs::metadata(p).map(|m| m.permissions().mode() & 0o777).ok();
    assert_eq!(mode(&dir.path().join("ssh")), Some(0o700));
    assert_eq!(mode(&dir.path().join("ssh/id_ed25519")), Some(0o600));
}

#[tokio::test]
async fn an_unknown_host_key_stops_until_it_is_trusted() {
    let m = machine();
    authorize(&m.keys, &m.home);
    let server = TestServer::start(ServerOptions::new(&m.home)).await;

    let refused = connect(&m.keys, &target(&server), Trust::KnownOnly, COMMENT).await.err();
    assert_eq!(refused, Some(Diagnosis::HostKeyUnknown { fingerprint: server.fingerprint.clone() }));
    assert!(server.fingerprint.starts_with("SHA256:"));

    let conn = connect(&m.keys, &target(&server), Trust::TrustNew, COMMENT).await;
    assert!(conn.is_ok(), "{:?}", conn.err());
    conn.unwrap_or_else(|e| panic!("{e}")).close().await;
    // Recorded: the strict policy lets it in now.
    let again = connect(&m.keys, &target(&server), Trust::KnownOnly, COMMENT).await;
    assert!(again.is_ok(), "{:?}", again.err());
}

#[tokio::test]
async fn a_changed_host_key_is_refused_until_forgotten() {
    let m = machine();
    authorize(&m.keys, &m.home);
    let first = TestServer::start(ServerOptions::new(&m.home)).await;
    let t = target(&first);
    assert!(connect(&m.keys, &t, Trust::TrustNew, COMMENT).await.is_ok());

    // Another machine answers on a port with a recorded key: same host and port, new key.
    let mut options = ServerOptions::new(&m.home);
    options.host_key_seed = 2;
    let other = TestServer::start(options).await;
    let recorded = std::fs::read_to_string(m.keys.known_hosts()).expect("known_hosts");
    std::fs::write(m.keys.known_hosts(), recorded.replace(&first.port.to_string(), &other.port.to_string()))
        .expect("rewrite known_hosts");
    let t = target(&other);
    for trust in [Trust::KnownOnly, Trust::TrustNew] {
        assert_eq!(
            connect(&m.keys, &t, trust, COMMENT).await.err(),
            Some(Diagnosis::HostKeyChanged { fingerprint: other.fingerprint.clone() }),
            "{trust:?}"
        );
    }
    m.keys.forget_host("127.0.0.1", other.port).unwrap_or_else(|e| panic!("{e}"));
    assert!(connect(&m.keys, &t, Trust::TrustNew, COMMENT).await.is_ok());
}

#[tokio::test]
async fn an_unauthorized_key_says_whether_a_password_would_work() {
    let m = machine();
    let server = TestServer::start(ServerOptions::new(&m.home)).await;
    assert_eq!(
        connect(&m.keys, &target(&server), Trust::TrustNew, COMMENT).await.err(),
        Some(Diagnosis::KeyNotAuthorized { offers_password: false })
    );
    let mut options = ServerOptions::new(&m.home);
    options.password = Some("hunter22".into());
    let server = TestServer::start(options).await;
    assert_eq!(
        connect(&m.keys, &target(&server), Trust::TrustNew, COMMENT).await.err(),
        Some(Diagnosis::KeyNotAuthorized { offers_password: true })
    );
}

#[tokio::test]
async fn the_password_installs_the_key_once() {
    let m = machine();
    let mut options = ServerOptions::new(&m.home);
    options.password = Some("hunter22".into());
    let server = TestServer::start(options).await;
    let t = target(&server);
    assert!(connect(&m.keys, &t, Trust::TrustNew, COMMENT).await.is_err(), "not authorized yet");

    for _ in 0..2 {
        let installed = install_device_key(&m.keys, &t, "hunter22", COMMENT).await;
        assert_eq!(installed, Ok(()));
    }
    let key = m.keys.public_key(COMMENT).unwrap_or_else(|e| panic!("{e}"));
    let authorized = std::fs::read_to_string(m.home.join(".ssh/authorized_keys")).expect("authorized_keys");
    assert_eq!(authorized.lines().filter(|l| *l == key).count(), 1, "{authorized}");
    let mode =
        std::fs::metadata(m.home.join(".ssh/authorized_keys")).map(|md| md.permissions().mode() & 0o077);
    assert_eq!(mode.ok(), Some(0), "authorized_keys is private");

    let conn = connect(&m.keys, &t, Trust::KnownOnly, COMMENT).await;
    assert!(conn.is_ok(), "{:?}", conn.err());
}

#[tokio::test]
async fn password_failures_are_named() {
    let m = machine();
    let mut options = ServerOptions::new(&m.home);
    options.password = Some("hunter22".into());
    let server = TestServer::start(options).await;
    // The host key must be trusted first; installing never trusts on its own.
    assert_eq!(
        install_device_key(&m.keys, &target(&server), "hunter22", COMMENT).await,
        Err(Diagnosis::HostKeyUnknown { fingerprint: server.fingerprint.clone() })
    );
    let _ = connect(&m.keys, &target(&server), Trust::TrustNew, COMMENT).await;
    assert_eq!(
        install_device_key(&m.keys, &target(&server), "wrong", COMMENT).await,
        Err(Diagnosis::PasswordRejected)
    );

    let no_passwords = TestServer::start(ServerOptions::new(&m.home)).await;
    let _ = connect(&m.keys, &target(&no_passwords), Trust::TrustNew, COMMENT).await;
    assert_eq!(
        install_device_key(&m.keys, &target(&no_passwords), "hunter22", COMMENT).await,
        Err(Diagnosis::PasswordNotOffered)
    );
}

#[tokio::test]
async fn commands_get_stdin_and_stream_lines_and_their_exit_status() {
    let m = machine();
    authorize(&m.keys, &m.home);
    let server = TestServer::start(ServerOptions::new(&m.home)).await;
    let conn =
        connect(&m.keys, &target(&server), Trust::TrustNew, COMMENT).await.unwrap_or_else(|e| panic!("{e}"));

    let (lines, status) = run(&conn, "cat; echo oops >&2; printf tail; exit 3", "a\nb\n").await;
    assert_eq!(status, Some(3));
    for want in ["a", "b", "oops", "tail"] {
        assert!(lines.iter().any(|l| l == want), "{want} in {lines:?}");
    }
    assert_eq!(lines.len(), 4, "{lines:?}");

    // How the wizard runs drift-host-setup.sh: the script on stdin, arguments after `--`.
    let (lines, status) = run(&conn, "bash -s -- one 'two words'", "printf '%s|' \"$@\"; echo\n").await;
    assert_eq!((lines, status), (vec!["one|two words|".to_owned()], Some(0)));

    // Several commands on one connection.
    let (lines, _) = run(&conn, "echo $HOME", "").await;
    assert_eq!(lines, [m.home.display().to_string()]);
}

#[tokio::test]
async fn sudo_without_a_password_is_detected() {
    let m = machine();
    authorize(&m.keys, &m.home);
    for (exit, want) in [(0, true), (1, false)] {
        let bin = m.home.join(format!("bin{exit}"));
        std::fs::create_dir_all(&bin).expect("bin");
        let sudo = bin.join("sudo");
        std::fs::write(
            &sudo,
            format!("#!/bin/sh\n[ \"$1\" = -n ] && [ \"$2\" = true ] || exit 9\nexit {exit}\n"),
        )
        .expect("fake sudo");
        std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let mut options = ServerOptions::new(&m.home);
        options.path_prefix = Some(bin);
        let server = TestServer::start(options).await;
        let conn = connect(&m.keys, &target(&server), Trust::TrustNew, COMMENT)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(conn.sudo_without_password().await, Ok(want), "sudo -n exits {exit}");
    }
}

#[tokio::test]
async fn an_unreachable_machine_is_named() {
    let m = machine();
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("addr").port()
    };
    let t = Target { host: "127.0.0.1".into(), port, user: "tester".into() };
    let err = connect(&m.keys, &t, Trust::TrustNew, COMMENT).await.err();
    assert!(matches!(err, Some(Diagnosis::Unreachable { .. })), "{err:?}");
}
