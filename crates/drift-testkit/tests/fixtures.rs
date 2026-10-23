//! M0-3 Red: the committed fixtures match `fixtures/MANIFEST.sha256`, the loader API works,
//! and the imported artefacts have the shapes recorded in plan §1.3–§1.7.

use drift_testkit::fixtures::{self, FixtureError, names};

#[test]
fn fixture_manifest_checksums_match() {
    match fixtures::verify_manifest() {
        Ok(n) => assert!(n >= names::ALL.len(), "only {n} files verified"),
        Err(problems) => panic!("fixture manifest problems:\n{}", problems.join("\n")),
    }
}

#[test]
fn every_named_fixture_is_in_the_manifest() {
    let text = std::fs::read_to_string(fixtures::path("MANIFEST.sha256"))
        .unwrap_or_else(|e| panic!("MANIFEST.sha256: {e}"));
    let listed: Vec<String> = fixtures::parse_manifest(&text)
        .unwrap_or_else(|e| panic!("{e}"))
        .into_iter()
        .map(|(_, p)| p)
        .collect();
    assert!(names::ALL.len() >= 38, "names::ALL lists {} fixtures", names::ALL.len());
    for n in names::ALL {
        assert!(listed.iter().any(|p| p == n), "{n} not in manifest");
    }
}

#[test]
fn manifest_parser_accepts_sha256sum_lines_only() {
    let sha = "a".repeat(64);
    let ok = fixtures::parse_manifest(&format!("{sha}  h264/x.h264\n\n")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(ok, vec![(sha.clone(), "h264/x.h264".to_string())]);
    for bad in ["zz  x", &format!("{sha} x"), &format!("{}  x", "g".repeat(64))] {
        assert!(matches!(fixtures::parse_manifest(bad), Err(FixtureError::BadManifest { line: 1 })), "{bad}");
    }
}

#[test]
fn records_roundtrip_and_truncation_is_an_error() {
    let mut data = Vec::new();
    for r in [&b"abc"[..], &b""[..], &[9u8; 300][..]] {
        data.extend_from_slice(&(r.len() as u32).to_le_bytes());
        data.extend_from_slice(r);
    }
    let recs = fixtures::parse_records(&data).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[0], b"abc");
    assert!(recs[1].is_empty());
    assert_eq!(recs[2].len(), 300);
    assert!(matches!(
        fixtures::parse_records(&data[..data.len() - 1]),
        Err(FixtureError::Truncated { offset: 11 })
    ));
    assert!(matches!(fixtures::parse_records(&[1, 0]), Err(FixtureError::Truncated { offset: 0 })));
}

#[test]
fn lfs_pointers_are_detected() {
    let pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 12\n";
    assert!(fixtures::is_lfs_pointer(pointer));
    assert!(!fixtures::is_lfs_pointer(b"\x00\x00\x00\x01\x09\x30"));
    assert!(fixtures::try_read("does/not/exist.bin").is_err());
}

#[test]
fn h264_fixtures_are_annex_b_with_leading_aud() {
    for n in [
        names::H264_LEG2,
        names::H264_LEG3,
        names::H264_LEG3_MOTION,
        names::H264_HEADLESS_MOTION,
        names::H264_HEADLESS_SCALE200,
        names::H264_HEADLESS_ANIM,
    ] {
        let d = fixtures::read(n);
        assert!(d.starts_with(&[0, 0, 0, 1, 0x09, 0x30]), "{n} must start with an AUD (plan §1.4)");
    }
}

#[test]
fn gfx_streams_are_records_and_raw_differs_from_decompressed() {
    for (plain, raw) in [
        (names::GFX_LEG2_GREETER_AVC420, names::GFX_LEG2_GREETER_AVC420_RAW),
        (names::GFX_LEG2_GREETER_PROGRESSIVE, names::GFX_LEG2_GREETER_PROGRESSIVE_RAW),
        (names::GFX_HEADLESS_MOTION_AVC420, names::GFX_HEADLESS_MOTION_AVC420_RAW),
    ] {
        let p = fixtures::records(plain);
        let r = fixtures::records(raw);
        assert_eq!(p.len(), r.len(), "{plain}: one decompressed record per DVC payload");
        assert!(p.len() > 5);
        // RDP_SEGMENTED_DATA descriptor: 0xE0 (single) or 0xE1 (multipart).
        assert!(r.iter().all(|x| matches!(x.first(), Some(0xE0 | 0xE1))), "{raw}");
        // First server PDU is CapabilitiesConfirm (cmdId 0x0013).
        assert_eq!(p[0].get(..2), Some(&[0x13, 0x00][..]), "{plain}");
    }
}

#[test]
fn redirection_and_rdstls_fixtures_match_plan_1_3_and_are_sanitized() {
    let frame = fixtures::read(names::SERVER_REDIRECTION_LEG1);
    let has = |needle: &[u8]| frame.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"Cookie: msts="), "LB info kept");
    assert!(has(&0x1C016u32.to_le_bytes()), "redirFlags 0x1C016");
    let redacted: Vec<u8> = "REDACTEDREDACTED".encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert!(has(&redacted), "one-time user name replaced by a same-length placeholder");

    assert_eq!(fixtures::read(names::RDSTLS_CAPS), [0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x03, 0x00]);
    assert_eq!(fixtures::read(names::RDSTLS_AUTH_RESPONSE_SUCCESS), [1, 0, 4, 0, 1, 0, 0, 0, 0, 0]);
    assert_eq!(
        fixtures::read(names::RDSTLS_AUTH_RESPONSE_LOGON_FAILURE),
        [1, 0, 4, 0, 1, 0, 0x2E, 0x05, 0, 0]
    );
    let req = fixtures::read(names::RDSTLS_AUTH_REQUEST_LEG2);
    assert_eq!(req.get(..6), Some(&[1, 0, 2, 0, 1, 0][..]), "version 1, type 2, dataType 1");

    let cert = fixtures::read(names::TLS_CERT_LEG2);
    assert_eq!(cert.first(), Some(&0x30), "DER SEQUENCE");
    assert_eq!(fixtures::read(names::TARGET_CERT_LEG1).len(), 3256);
}

#[test]
fn pointer_and_clipboard_fixtures_have_the_expected_shape() {
    let p100 = fixtures::records(names::FASTPATH_POINTER_SCALE100);
    let p200 = fixtures::records(names::FASTPATH_POINTER_SCALE200);
    assert!(!p100.is_empty() && !p200.is_empty());
    // Fast-path output header action bits are 0 (FASTPATH_OUTPUT_ACTION_FASTPATH).
    assert!(p100.iter().chain(&p200).all(|r| r.first().is_some_and(|b| b & 0x03 == 0)));

    let png = fixtures::read(names::CLIP_REMOTE_PNG);
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let text = fixtures::read(names::CLIP_REMOTE_UNICODETEXT);
    let units: Vec<u16> = text.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let s = String::from_utf16_lossy(&units);
    assert!(s.contains("héllo 世界 🚀"), "{s:?}");
    assert!(s.ends_with('\0'), "CF_UNICODETEXT is NUL-terminated");

    let golden = fixtures::read(names::GOLDEN_GREETER);
    assert!(golden.starts_with(b"\x89PNG\r\n\x1a\n"));
}
