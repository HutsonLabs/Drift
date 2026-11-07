//! WIZ-3 live check against a real OpenSSH server (ignored by default):
//!
//! ```text
//! DRIFT_SSH_LIVE=alice@10.0.2.5 DRIFT_SSH_LIVE_STORE=/path/to/store \
//!     cargo nextest run -p drift-ssh --test live --run-ignored only
//! ```
//!
//! The store's device key (`public_key("drift@live")`) must be in the account's
//! `authorized_keys`; the host key is trusted on first use.

use drift_ssh::{KeyStore, Target, Trust, connect};

#[tokio::test]
#[ignore = "needs DRIFT_SSH_LIVE=user@host and an authorized DRIFT_SSH_LIVE_STORE"]
async fn real_openssh_runs_a_script_from_stdin() {
    let spec = std::env::var("DRIFT_SSH_LIVE").expect("DRIFT_SSH_LIVE=user@host[:port]");
    let store = std::env::var("DRIFT_SSH_LIVE_STORE").expect("DRIFT_SSH_LIVE_STORE");
    let (user, rest) = spec.split_once('@').expect("user@host");
    let (host, port) = rest.split_once(':').map_or((rest, 22), |(h, p)| (h, p.parse().expect("port")));
    let keys = KeyStore::open(store).unwrap_or_else(|e| panic!("{e}"));
    let target = Target { host: host.into(), port, user: user.into() };
    let conn = connect(&keys, &target, Trust::TrustNew, "drift@live").await.unwrap_or_else(|e| panic!("{e}"));

    let mut lines = Vec::new();
    let status = conn
        .run("bash -s -- one 'two words'", b"uname -s\nprintf '%s|' \"$@\"; echo\nexit 4\n", &mut |l| {
            lines.push(l.to_owned());
        })
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(status, Some(4));
    assert_eq!(lines, ["Linux", "one|two words|"]);
    let sudo = conn.sudo_without_password().await.unwrap_or_else(|e| panic!("{e}"));
    eprintln!("sudo without a password: {sudo}");
    conn.close().await;
}
