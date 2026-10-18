//! The capability sets Drift advertises (plan §1.4, §2 decision 8; ADR `AVC444-client`).
//!
//! Verified against g-r-d 50.2: `[V8_1{AVC420_ENABLED}, V8{}]` is confirmed as V8.1 and the
//! server sends AVC420 when it has a hardware encoder, RFX Progressive otherwise. Any V10.x set
//! would make it choose AVC444v2 (without `AVC_DISABLED`) or V10.7 + Progressive (with it), so
//! GNOME hosts never see a V10 set: [`GfxCaps::Gnome`] is the default and its list is fixed.
//!
//! A paired Drift Host ([`GfxCaps::DriftHost`]) gets `V10 {}` first. Drift Host sends AVC444
//! (v1) or AVC420; Drift decodes both, and still not AVC444v2.

use ironrdp_egfx::pdu::{
    CapabilitiesAdvertisePdu, CapabilitiesV8Flags, CapabilitiesV10Flags, CapabilitiesV81Flags, CapabilitySet,
    GfxPdu,
};

/// Which capability list a session advertises.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum GfxCaps {
    /// The verified V8.1 list (every server that is not a paired Drift Host).
    #[default]
    Gnome,
    /// V10 first, so a Drift Host can send AVC444 (then the V8.1 list as fallback).
    DriftHost,
}

/// The capability sets sent in `RDPGFX_CAPS_ADVERTISE_PDU` for GNOME and every other host:
/// `V8_1 { AVC420_ENABLED }`, then `V8 {}`.
pub fn advertised_caps() -> Vec<CapabilitySet> {
    advertised_caps_for(GfxCaps::Gnome)
}

/// The capability sets for `caps`, in order of preference.
pub fn advertised_caps_for(caps: GfxCaps) -> Vec<CapabilitySet> {
    let v8 = [
        CapabilitySet::V8_1 { flags: CapabilitiesV81Flags::AVC420_ENABLED },
        CapabilitySet::V8 { flags: CapabilitiesV8Flags::empty() },
    ];
    match caps {
        GfxCaps::Gnome => v8.to_vec(),
        GfxCaps::DriftHost => {
            let mut sets = vec![CapabilitySet::V10 { flags: CapabilitiesV10Flags::empty() }];
            sets.extend(v8);
            sets
        }
    }
}

/// The `CapabilitiesAdvertise` PDU carrying [`advertised_caps`] (34 bytes on the wire).
pub fn caps_advertise_pdu() -> GfxPdu {
    caps_advertise_pdu_for(GfxCaps::Gnome)
}

/// The `CapabilitiesAdvertise` PDU for `caps`.
pub fn caps_advertise_pdu_for(caps: GfxCaps) -> GfxPdu {
    GfxPdu::CapabilitiesAdvertise(CapabilitiesAdvertisePdu::from_typed(&advertised_caps_for(caps)))
}
