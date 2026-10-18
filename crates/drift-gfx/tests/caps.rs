//! M1-2 Red: the capabilities Drift advertises are exactly the verified set
//! `[V8_1{AVC420_ENABLED}, V8{}]` (plan §1.4), byte for byte.
#![allow(clippy::unwrap_used)]

mod support;

use drift_core::Size;
use drift_gfx::{CHANNEL_NAME, advertised_caps, caps_advertise_pdu};
use drift_testkit::PresentMode;
use drift_testkit::fixtures::{self, names};
use ironrdp_core::encode_vec;
use ironrdp_dvc::DvcProcessor;
use ironrdp_egfx::pdu::{CapabilitiesV8Flags, CapabilitiesV81Flags, CapabilitySet};

/// `RDPGFX_CAPS_ADVERTISE_PDU`: header (cmdId 0x12, flags 0, length 34), capsSetCount 2,
/// then V8.1 (0x80105, 4 bytes, flags AVC420_ENABLED=0x10) and V8 (0x80004, 4 bytes, flags 0).
const VERIFIED_CAPS_ADVERTISE: [u8; 34] = [
    0x12, 0x00, 0x00, 0x00, 0x22, 0x00, 0x00, 0x00, // RDPGFX_HEADER
    0x02, 0x00, // capsSetCount
    0x05, 0x01, 0x08, 0x00, 0x04, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, // V8.1 {AVC420_ENABLED}
    0x04, 0x00, 0x08, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // V8 {}
];

#[test]
fn advertised_caps_are_v81_avc420_then_v8() {
    assert_eq!(
        advertised_caps(),
        vec![
            CapabilitySet::V8_1 { flags: CapabilitiesV81Flags::AVC420_ENABLED },
            CapabilitySet::V8 { flags: CapabilitiesV8Flags::empty() },
        ]
    );
}

#[test]
fn caps_advertise_bytes_equal_the_verified_set() {
    assert_eq!(encode_vec(&caps_advertise_pdu()).unwrap(), VERIFIED_CAPS_ADVERTISE);
}

#[test]
fn caps_advertise_bytes_equal_the_captured_client_pdu() {
    // The first client→server GFX PDU of the Remote Login greeter capture (plan §1.7).
    let client = fixtures::records(names::GFX_LEG2_GREETER_AVC420_CLIENT);
    assert_eq!(client[0], VERIFIED_CAPS_ADVERTISE);
}

#[test]
fn channel_start_sends_exactly_the_caps_advertise() {
    let mut h = support::harness(PresentMode::Immediate, Size::new(64, 64));
    assert_eq!(h.client.channel_name(), CHANNEL_NAME);
    assert_eq!(CHANNEL_NAME, "Microsoft::Windows::RDS::Graphics");
    let msgs = h.client.start(7).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(encode_vec(msgs[0].as_ref()).unwrap(), VERIFIED_CAPS_ADVERTISE);
}

// ---- AVC444 end to end: a paired Drift Host gets V10 ----------------------------------------

/// GNOME keeps the verified V8.1 list (any V10 set makes g-r-d send AVC444v2, which Drift does
/// not decode); a Drift Host gets V10 first, so it can choose AVC444 (ADR `AVC444-client`).
#[test]
fn drift_hosts_get_v10_first_and_gnome_keeps_the_verified_list() {
    use drift_gfx::{GfxCaps, advertised_caps_for};
    use ironrdp_egfx::pdu::CapabilitiesV10Flags;

    assert_eq!(advertised_caps_for(GfxCaps::Gnome), advertised_caps());
    assert_eq!(
        advertised_caps_for(GfxCaps::DriftHost),
        vec![
            CapabilitySet::V10 { flags: CapabilitiesV10Flags::empty() },
            CapabilitySet::V8_1 { flags: CapabilitiesV81Flags::AVC420_ENABLED },
            CapabilitySet::V8 { flags: CapabilitiesV8Flags::empty() },
        ]
    );
    assert_eq!(GfxCaps::default(), GfxCaps::Gnome);
}

#[test]
fn a_drift_host_client_advertises_v10_on_channel_start() {
    use drift_gfx::{GfxCaps, caps_advertise_pdu_for};
    let mut h = support::harness(PresentMode::Immediate, Size::new(64, 64));
    h.client.set_caps(GfxCaps::DriftHost);
    let out = h.client.start(1).unwrap();
    assert_eq!(out.len(), 1);
    let bytes = encode_vec(out[0].as_ref()).unwrap();
    assert_eq!(bytes, encode_vec(&caps_advertise_pdu_for(GfxCaps::DriftHost)).unwrap());
}
