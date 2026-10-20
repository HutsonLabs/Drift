//! GNOME-only: `drift-connect:` links prefill a new GNOME connection.

use drift_core::link::{self, LinkError};
use drift_core::{CertFingerprint, ConnectMode, ConnectionProfile};

const PIN: &str =
    "f3:e7:a2:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:01:02:03:04:05:06:07:08:09:0a:0b:0c:0d";

#[test]
fn a_setup_link_fills_every_field() {
    let p = link::parse(&format!(
        "drift-connect:?name=Work%20box&host=box.local&port=3390&mode=headless&user=me&pin={PIN}"
    ))
    .unwrap();
    assert_eq!(p.name, "Work box");
    assert_eq!(p.host, "box.local");
    assert_eq!(p.port, 3390);
    assert_eq!(p.mode, ConnectMode::Headless);
    assert_eq!(p.rdp_username, "me");
    assert_eq!(p.cert_pin, Some(PIN.parse::<CertFingerprint>().unwrap()));
    assert_eq!(p.linux_username, None);
}

#[test]
fn defaults_come_from_the_host_and_mode() {
    let p = link::parse("DRIFT-CONNECT:?host=10.0.0.5&mode=remote-login&linux_user=ann&extra=1").unwrap();
    assert_eq!(p.name, "10.0.0.5");
    assert_eq!(p.mode, ConnectMode::RemoteLogin);
    assert_eq!(p.port, ConnectMode::RemoteLogin.default_port());
    assert_eq!(p.linux_username.as_deref(), Some("ann"));
    assert_eq!(p.cert_pin, None);
    assert_eq!(link::parse("drift-connect:?host=h").unwrap().mode, ConnectMode::Headless);
}

#[test]
fn bad_links_are_refused() {
    assert_eq!(link::parse("drift-pair:abc"), Err(LinkError::NotALink));
    assert_eq!(link::parse("drift-connect:?name=x"), Err(LinkError::NoHost));
    assert_eq!(link::parse("drift-connect:?host=h&port=0"), Err(LinkError::Invalid("port")));
    assert_eq!(link::parse("drift-connect:?host=h&port=99999"), Err(LinkError::Invalid("port")));
    assert_eq!(link::parse("drift-connect:?host=h&mode=vnc"), Err(LinkError::Invalid("mode")));
    assert_eq!(link::parse("drift-connect:?host=h&pin=zz"), Err(LinkError::Invalid("pin")));
    assert_eq!(link::parse("drift-connect:?host=h%G1"), Err(LinkError::Invalid("host")));
    assert!(!link::is_connect_link("https://example.com"));
}

#[test]
fn format_round_trips() {
    let mut p = ConnectionProfile::new("Ann's PC & co", "pc.example.org", ConnectMode::RemoteLogin);
    p.rdp_username = "rdp user".into();
    p.linux_username = Some("ann".into());
    p.cert_pin = Some(PIN.parse().unwrap());
    let back = link::parse(&link::format(&p)).unwrap();
    assert_eq!(
        (back.name, back.host, back.port, back.mode),
        (p.name.clone(), p.host.clone(), p.port, p.mode)
    );
    assert_eq!(
        (back.rdp_username, back.linux_username, back.cert_pin),
        (p.rdp_username, p.linux_username, p.cert_pin)
    );
}

/// The exact link `host/drift-gnome-setup --print-link` prints (xtask/tests/host_script.rs).
#[test]
fn the_setup_scripts_link_parses() {
    let p = link::parse(&format!(
        "drift-connect:?name=Work%20box&host=box.local&port=3391&mode=headless&user=me&pin={PIN}"
    ))
    .unwrap();
    assert_eq!(
        (p.name.as_str(), p.host.as_str(), p.port, p.rdp_username.as_str()),
        ("Work box", "box.local", 3391, "me")
    );
    assert!(p.cert_pin.is_some());
}
