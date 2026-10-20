//! The capability sets Drift advertises (plan §1.4, §2 decision 8).
//!
//! Verified against g-r-d 50.2: `[V8_1{AVC420_ENABLED}, V8{}]` is confirmed as V8.1 and the
//! server sends AVC420 when it has a hardware encoder, RFX Progressive otherwise. Any V10.x set
//! would make it choose AVC444v2 (without `AVC_DISABLED`) or V10.7 + Progressive (with it), so
//! Drift never advertises a V10 set, and its list is fixed.

use ironrdp_egfx::pdu::{
    CapabilitiesAdvertisePdu, CapabilitiesV8Flags, CapabilitiesV81Flags, CapabilitySet, GfxPdu,
};

/// The capability sets sent in `RDPGFX_CAPS_ADVERTISE_PDU` for GNOME and every other host:
/// `V8_1 { AVC420_ENABLED }`, then `V8 {}`.
pub fn advertised_caps() -> Vec<CapabilitySet> {
    vec![
        CapabilitySet::V8_1 { flags: CapabilitiesV81Flags::AVC420_ENABLED },
        CapabilitySet::V8 { flags: CapabilitiesV8Flags::empty() },
    ]
}

/// The `CapabilitiesAdvertise` PDU carrying [`advertised_caps`] (34 bytes on the wire).
pub fn caps_advertise_pdu() -> GfxPdu {
    GfxPdu::CapabilitiesAdvertise(CapabilitiesAdvertisePdu::from_typed(&advertised_caps()))
}
