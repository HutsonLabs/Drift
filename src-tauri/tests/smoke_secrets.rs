//! IOS-5: smoke-run passwords from the environment (`drift_app::smoke`), shared by the macOS
//! `smoke` example and the iOS simulator's `smoke-env` build. Test values only.
#![allow(clippy::unwrap_used)]

use drift_app::secrets::SecretStore as _;
use drift_app::smoke::{parse_secret, secrets_from};
use drift_core::SecretRole;
use uuid::Uuid;

#[test]
fn parses_id_role_and_a_password_with_colons() {
    let id = Uuid::new_v4();
    let (got, role, password) = parse_secret(&format!("{id}:rdp-user:fake:with:colons")).unwrap();
    assert_eq!((got, role, password.as_str()), (id, SecretRole::RdpUser, "fake:with:colons"));
}

#[test]
fn rejects_malformed_specs() {
    assert!(parse_secret("not-a-uuid:rdp-user:x").is_err());
    assert!(parse_secret(&format!("{}:root:x", Uuid::new_v4())).is_err());
    assert!(parse_secret(&format!("{}:rdp-user", Uuid::new_v4())).is_err());
}

#[test]
fn loads_every_smoke_variable_and_nothing_else() {
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let vars = vec![
        ("DRIFT_SMOKE_SECRET".to_owned(), format!("{a}:rdp-user:fake-a")),
        ("DRIFT_SMOKE_SECRET_2".to_owned(), format!("{b}:linux-login:fake-b")),
        ("DRIFT_SMOKE_SECRET_3".to_owned(), "garbage".to_owned()),
        ("OTHER".to_owned(), format!("{a}:rdp-system:fake-c")),
    ];
    let store = secrets_from(vars);
    assert_eq!(store.get(a, SecretRole::RdpUser).unwrap().as_deref().map(String::as_str), Some("fake-a"));
    assert_eq!(store.get(b, SecretRole::LinuxLogin).unwrap().as_deref().map(String::as_str), Some("fake-b"));
    assert!(!store.has(a, SecretRole::RdpSystem));
}

#[test]
fn smoke_scripts_parse_waits_connects_and_picks() {
    use drift_app::smoke::{SmokeStep, parse_script};
    use std::time::Duration;
    assert_eq!(
        parse_script("connect: Fake host 2 ; wait:1.5; pick:session-0;").unwrap(),
        vec![
            SmokeStep::Connect("Fake host 2".into()),
            SmokeStep::Wait(Duration::from_millis(1500)),
            SmokeStep::Pick("session-0".into()),
        ]
    );
    assert!(parse_script("wait:soon").is_err());
    assert!(parse_script("jump:1").is_err());
    assert!(parse_script("pick").is_err());
}
