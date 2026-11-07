//! M1-6 Red: `drift_core::profile::validate` table tests (+ secret-role addressing, M3-2).

use drift_core::profile::{self, ProfileField as F, ProfileProblem as P};
use drift_core::{ConnectMode, ConnectionProfile, SecretRole};
use uuid::Uuid;

fn base(mode: ConnectMode) -> ConnectionProfile {
    let mut p = ConnectionProfile::new("Homelab", "gnome.example.lan", mode);
    p.rdp_username = "rdp-user".into();
    p
}

fn problems(p: &ConnectionProfile) -> Vec<(F, P)> {
    match profile::validate(p) {
        Ok(()) => vec![],
        Err(issues) => issues.into_iter().map(|i| (i.field, i.problem)).collect(),
    }
}

type Edit = fn(&mut ConnectionProfile);
type Case<'a> = (&'a str, ConnectMode, Edit, &'a [(F, P)]);

#[test]
fn valid_profiles_for_every_mode() {
    for mode in ConnectMode::ALL {
        assert_eq!(problems(&base(mode)), vec![], "{mode:?}");
    }
    let mut p = base(ConnectMode::RemoteLogin);
    p.linux_username = Some("drifttest".into());
    assert_eq!(problems(&p), vec![]);
}

#[test]
fn field_table() {
    #[rustfmt::skip]
    let table: &[Case<'_>] = &[
        ("empty name",            ConnectMode::Headless, |p| p.name = String::new(),        &[(F::Name, P::Empty)]),
        ("blank name",            ConnectMode::Headless, |p| p.name = "   ".into(),          &[(F::Name, P::Empty)]),
        ("name too long",         ConnectMode::Headless, |p| p.name = "x".repeat(65),        &[(F::Name, P::TooLong)]),
        ("name 64 chars ok",      ConnectMode::Headless, |p| p.name = "é".repeat(64),        &[]),
        ("name control char",     ConnectMode::Headless, |p| p.name = "a\u{7}b".into(),      &[(F::Name, P::InvalidCharacters)]),
        ("name padded",           ConnectMode::Headless, |p| p.name = " Home".into(),        &[(F::Name, P::SurroundingWhitespace)]),
        ("empty host",            ConnectMode::Headless, |p| p.host = String::new(),         &[(F::Host, P::Empty)]),
        ("ipv4 host",             ConnectMode::Headless, |p| p.host = "10.0.2.40".into(),    &[]),
        ("ipv6 host",             ConnectMode::Headless, |p| p.host = "fe80::1".into(),      &[]),
        ("localhost",             ConnectMode::Headless, |p| p.host = "localhost".into(),    &[]),
        ("mdns host",             ConnectMode::Headless, |p| p.host = "gnome.local".into(),  &[]),
        ("trailing dot fqdn",     ConnectMode::Headless, |p| p.host = "gnome.example.".into(), &[]),
        ("host with port",        ConnectMode::Headless, |p| p.host = "gnome.local:3389".into(), &[(F::Host, P::PortInHost)]),
        ("ipv4 with port",        ConnectMode::Headless, |p| p.host = "10.0.2.40:3390".into(), &[(F::Host, P::PortInHost)]),
        ("bracketed v6 port",     ConnectMode::Headless, |p| p.host = "[fe80::1]:3389".into(), &[(F::Host, P::PortInHost)]),
        ("host with scheme",      ConnectMode::Headless, |p| p.host = "rdp://gnome.local".into(), &[(F::Host, P::SchemeInHost)]),
        ("host with space",       ConnectMode::Headless, |p| p.host = "gnome local".into(),  &[(F::Host, P::InvalidHost)]),
        ("host padded",           ConnectMode::Headless, |p| p.host = "gnome.local ".into(), &[(F::Host, P::SurroundingWhitespace)]),
        ("host underscore",       ConnectMode::Headless, |p| p.host = "my_host".into(),      &[(F::Host, P::InvalidHost)]),
        ("host leading hyphen",   ConnectMode::Headless, |p| p.host = "-gnome".into(),       &[(F::Host, P::InvalidHost)]),
        ("host empty label",      ConnectMode::Headless, |p| p.host = "a..b".into(),         &[(F::Host, P::InvalidHost)]),
        ("bad ipv4",              ConnectMode::Headless, |p| p.host = "10.0.2.300".into(),   &[(F::Host, P::InvalidHost)]),
        ("label too long",        ConnectMode::Headless, |p| p.host = format!("{}.lan", "a".repeat(64)), &[(F::Host, P::InvalidHost)]),
        ("host too long",         ConnectMode::Headless, |p| p.host = "a.".repeat(127),      &[(F::Host, P::TooLong)]),
        ("port zero",             ConnectMode::Headless, |p| p.port = 0,                     &[(F::Port, P::InvalidPort)]),
        ("port 65535 ok",         ConnectMode::Headless, |p| p.port = 65535,                 &[]),
        ("empty rdp user",        ConnectMode::Headless, |p| p.rdp_username = String::new(), &[(F::RdpUsername, P::Empty)]),
        ("rdp user padded",       ConnectMode::DesktopSharing, |p| p.rdp_username = "u ".into(), &[(F::RdpUsername, P::SurroundingWhitespace)]),
        ("rdp user too long",     ConnectMode::RemoteLogin, |p| p.rdp_username = "u".repeat(257), &[(F::RdpUsername, P::TooLong)]),
        ("rdp user newline",      ConnectMode::RemoteLogin, |p| p.rdp_username = "a\nb".into(), &[(F::RdpUsername, P::InvalidCharacters)]),
        ("linux user headless",   ConnectMode::Headless, |p| p.linux_username = Some("drifttest".into()), &[(F::LinuxUsername, P::NotApplicable)]),
        ("linux user sharing",    ConnectMode::DesktopSharing, |p| p.linux_username = Some("x".into()), &[(F::LinuxUsername, P::NotApplicable)]),
        ("linux user empty",      ConnectMode::RemoteLogin, |p| p.linux_username = Some(String::new()), &[(F::LinuxUsername, P::Empty)]),
        ("linux user space",      ConnectMode::RemoteLogin, |p| p.linux_username = Some("drift test".into()), &[(F::LinuxUsername, P::InvalidCharacters)]),
        ("linux user colon",      ConnectMode::RemoteLogin, |p| p.linux_username = Some("a:b".into()), &[(F::LinuxUsername, P::InvalidCharacters)]),
        ("linux user dash",       ConnectMode::RemoteLogin, |p| p.linux_username = Some("-x".into()), &[(F::LinuxUsername, P::InvalidCharacters)]),
        ("linux user domain ok",  ConnectMode::RemoteLogin, |p| p.linux_username = Some("jane@corp.example".into()), &[]),
        ("several at once",       ConnectMode::Headless, |p| { p.name.clear(); p.port = 0; p.rdp_username.clear(); },
                                  &[(F::Name, P::Empty), (F::Port, P::InvalidPort), (F::RdpUsername, P::Empty)]),
    ];
    for (name, mode, edit, expected) in table {
        let mut p = base(*mode);
        edit(&mut p);
        assert_eq!(problems(&p), expected.to_vec(), "{name}");
    }
}

#[test]
fn every_issue_has_a_readable_message() {
    let mut p = base(ConnectMode::Headless);
    p.name.clear();
    p.host = "gnome:3389".into();
    p.port = 0;
    p.linux_username = Some("x".into());
    let issues = profile::validate(&p).unwrap_err();
    assert_eq!(issues.len(), 4);
    for i in &issues {
        assert!(i.message.ends_with('.'), "{i:?}");
        assert!(i.message.len() > 10, "{i:?}");
    }
    assert_eq!(issues[0].message, "Name is required.");
    assert_eq!(issues[1].message, "Put the port number in the Port field, not in the host.");
    assert_eq!(issues[3].message, "Linux user name only applies to Remote Login profiles.");
}

#[test]
fn secret_roles_and_keychain_accounts() {
    let id = Uuid::from_u128(1);
    assert_eq!(SecretRole::rdp_for(ConnectMode::RemoteLogin), SecretRole::RdpSystem);
    assert_eq!(SecretRole::rdp_for(ConnectMode::Headless), SecretRole::RdpUser);
    assert_eq!(SecretRole::rdp_for(ConnectMode::DesktopSharing), SecretRole::RdpUser);
    assert_eq!(SecretRole::RdpSystem.account(id), "00000000-0000-0000-0000-000000000001/rdp-system");
    assert_eq!(SecretRole::RdpUser.as_str(), "rdp-user");
    assert_eq!(SecretRole::LinuxLogin.as_str(), "linux-login");
}
