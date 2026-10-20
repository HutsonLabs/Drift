//! M0-5 Red: `ConnectionProfile` TOML round-trip and `CertFingerprint` grdctl format.

use drift_core::{
    CertFingerprint, ClipboardPrefs, CmdAs, ConnectMode, ConnectionProfile, DisplayPrefs, KeyboardPrefs,
};
use uuid::Uuid;

fn full_profile() -> ConnectionProfile {
    ConnectionProfile {
        id: Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
        name: "Homelab login".into(),
        host: "10.1.2.40".into(),
        port: 3389,
        mode: ConnectMode::RemoteLogin,
        rdp_username: "rdp-system".into(),
        linux_username: Some("drifttest".into()),
        cert_pin: Some(CertFingerprint::from_bytes(std::array::from_fn(|i| (i as u8).wrapping_mul(37)))),
        keyboard: KeyboardPrefs { cmd_as: CmdAs::Ctrl, type_with_mac_layout: true },
        display: DisplayPrefs { adaptive: false, retina: true },
        clipboard: ClipboardPrefs::Text,
        touch: drift_core::TouchMode::Trackpad,
    }
}

#[test]
fn full_profile_round_trips_through_toml() {
    let p = full_profile();
    let toml = p.to_toml().expect("serialize");
    let back = ConnectionProfile::from_toml(&toml).expect("parse");
    assert_eq!(back, p);
}

#[test]
fn every_mode_and_clipboard_pref_round_trips() {
    for mode in ConnectMode::ALL {
        for clipboard in [ClipboardPrefs::Off, ClipboardPrefs::Text, ClipboardPrefs::TextAndImages] {
            let mut p = ConnectionProfile::new("x", "host.example", mode);
            p.clipboard = clipboard;
            let back = ConnectionProfile::from_toml(&p.to_toml().unwrap()).unwrap();
            assert_eq!(back, p, "{mode:?}/{clipboard:?}");
        }
    }
}

#[test]
fn minimal_toml_gets_defaults() {
    let toml = r#"
id = "01234567-89ab-cdef-0123-456789abcdef"
name = "Headless"
host = "127.0.0.1"
port = 13392
mode = "headless"
rdp_username = "someone"
"#;
    let p = ConnectionProfile::from_toml(toml).unwrap();
    assert_eq!(p.mode, ConnectMode::Headless);
    assert_eq!(p.port, 13392);
    assert_eq!(p.linux_username, None);
    assert_eq!(p.cert_pin, None);
    assert_eq!(p.keyboard, KeyboardPrefs { cmd_as: CmdAs::Super, type_with_mac_layout: false });
    assert_eq!(p.display, DisplayPrefs { adaptive: true, retina: true });
    assert_eq!(p.clipboard, ClipboardPrefs::TextAndImages);
}

#[test]
fn toml_uses_grdctl_fingerprint_and_kebab_case() {
    let toml = full_profile().to_toml().unwrap();
    assert!(toml.contains(r#"mode = "remote-login""#), "{toml}");
    assert!(toml.contains(r#"cmd_as = "ctrl""#), "{toml}");
    assert!(toml.contains(r#"cert_pin = "00:25:4a:6f:"#), "{toml}");
}

#[test]
fn malformed_toml_is_an_error_not_a_panic() {
    assert!(ConnectionProfile::from_toml("name = 3").is_err());
    assert!(ConnectionProfile::from_toml("").is_err());
    let bad_pin = full_profile().to_toml().unwrap().replace("cert_pin = \"00:", "cert_pin = \"zz:");
    assert!(ConnectionProfile::from_toml(&bad_pin).is_err());
}

#[test]
fn fingerprint_display_matches_grdctl_status_format() {
    // Format printed by `grdctl status` (FreeRDP freerdp_certificate_get_fingerprint):
    // lowercase hex pairs joined by ':'.
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&[0xf3, 0xe7, 0xa2, 0x0b]);
    bytes[31] = 0xff;
    let s = CertFingerprint::from_bytes(bytes).to_string();
    assert_eq!(s.len(), 32 * 3 - 1);
    assert!(s.starts_with("f3:e7:a2:0b:00:"), "{s}");
    assert!(s.ends_with(":00:ff"), "{s}");
    assert_eq!(s.parse::<CertFingerprint>().unwrap().as_bytes(), &bytes);
}

#[test]
fn fingerprint_parse_accepts_uppercase_and_rejects_garbage() {
    let fp = CertFingerprint::from_bytes([0xab; 32]);
    assert_eq!(fp.to_string().to_uppercase().parse::<CertFingerprint>().unwrap(), fp);
    for bad in ["", "ab", "ab:cd", &"ab:".repeat(32), &"abc:".repeat(31), &"ab-".repeat(31)] {
        assert!(bad.parse::<CertFingerprint>().is_err(), "{bad:?}");
    }
    let plus_sign = format!("+f{}", &fp.to_string()[2..]);
    assert!(plus_sign.parse::<CertFingerprint>().is_err(), "from_str_radix accepts '+'");
    let too_long = format!("{fp}:00");
    assert!(too_long.parse::<CertFingerprint>().is_err());
}

// ---- IOS-7: touch mode ---------------------------------------------------------------------

#[test]
fn touch_mode_defaults_to_auto_and_is_not_written() {
    let p = ConnectionProfile::new("x", "h", ConnectMode::Headless);
    assert_eq!(p.touch, drift_core::TouchMode::Auto);
    assert!(!p.to_toml().unwrap().contains("touch"), "existing files stay unchanged");
}

#[test]
fn touch_mode_round_trips_and_resolves_per_device() {
    use drift_core::TouchMode;
    let mut p = ConnectionProfile::new("x", "h", ConnectMode::Headless);
    p.touch = TouchMode::Direct;
    let text = p.to_toml().unwrap();
    assert!(text.contains("touch = \"direct\""), "{text}");
    assert_eq!(ConnectionProfile::from_toml(&text).unwrap().touch, TouchMode::Direct);
    assert_eq!(TouchMode::Auto.resolve(true), TouchMode::Trackpad, "iPhone: trackpad");
    assert_eq!(TouchMode::Auto.resolve(false), TouchMode::Direct, "iPad: direct");
    assert_eq!(TouchMode::Trackpad.resolve(false), TouchMode::Trackpad);
}
