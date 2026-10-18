//! # drift-ext
//!
//! The `Drift::Mac::v1` dynamic virtual channel (task **EXT-1**; ADR `IOS-0` decision 3, ADR
//! `EXT-1-channel-framing`): what a Drift client and Drift Host say to each other beyond plain
//! RDP. Anything else (GNOME, `mstsc`, Windows App) never opens it and gets plain RDP.
//!
//! | Module | What |
//! |---|---|
//! | [`wire`] | versioned, length-delimited framing; strict decode (fuzzed: `fuzz/`) |
//! | [`channel`] | the client and server DVC processors and the session-facing [`channel::Link`] |
//! | [`gestures`] | precise scroll and trackpad gestures (EXT-2) |
//! | [`status`] | host status and quality hints (EXT-4) |
//! | [`pairing`] | pairing and device authentication messages, and the client's side (HOST-8) |
//!
//! [`negotiate`] settles the version and the shared capabilities from two [`wire::Hello`]s.

pub mod channel;
pub mod gestures;
pub mod pairing;
pub mod status;
pub mod wire;

use wire::{Capabilities, Hello};

/// What both ends agreed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Negotiated {
    /// The highest protocol version both speak.
    pub version: u16,
    /// The capabilities both offered.
    pub capabilities: Capabilities,
}

/// Why two Hellos do not agree.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NegotiationError {
    /// A Hello whose minimum version is above its maximum.
    #[error("invalid version range {0}..={1}")]
    InvalidRange(u16, u16),
    /// No version both ends speak.
    #[error("no common version: ours {ours:?}, theirs {theirs:?}")]
    NoCommonVersion {
        /// Our `(min, max)`.
        ours: (u16, u16),
        /// Their `(min, max)`.
        theirs: (u16, u16),
    },
}

/// The highest version both Hellos allow, and the capabilities both offer.
pub fn negotiate(ours: &Hello, theirs: &Hello) -> Result<Negotiated, NegotiationError> {
    for h in [ours, theirs] {
        if h.min_version > h.max_version {
            return Err(NegotiationError::InvalidRange(h.min_version, h.max_version));
        }
    }
    let low = ours.min_version.max(theirs.min_version);
    let high = ours.max_version.min(theirs.max_version);
    if low > high {
        return Err(NegotiationError::NoCommonVersion {
            ours: (ours.min_version, ours.max_version),
            theirs: (theirs.min_version, theirs.max_version),
        });
    }
    Ok(Negotiated { version: high, capabilities: ours.capabilities & theirs.capabilities })
}
