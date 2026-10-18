//! Which codec the host sends (ADR `AVC444-client`). Pure, so the capture pipeline can ask it
//! per output size.
//!
//! AVC444 needs a client that confirmed a capability set with AVC444 (V10 or later, AVC not
//! disabled; Drift advertises V10 to paired Drift Hosts) and a picture small enough for both
//! views at full rate: HOST-4 measured 60 fps up to about 1920×1080 on an M4 Pro.

use drift_core::Size;

/// The largest picture (in pixels) sent as AVC444; anything larger is AVC420.
pub const AVC444_MAX_PIXELS: u64 = 1920 * 1080;

/// The codec for one output size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// Two AVC420 views, full chroma.
    Avc444,
    /// One AVC420 stream, 4:2:0 chroma.
    Avc420,
    /// The client negotiated no H.264 at all.
    None,
}

/// Picks the codec from what the client confirmed (`avc420`, `avc444`) and the output `size`.
pub fn choose(avc420: bool, avc444: bool, size: Size<u32>) -> Codec {
    let pixels = u64::from(size.width) * u64::from(size.height);
    if avc444 && pixels <= AVC444_MAX_PIXELS {
        Codec::Avc444
    } else if avc420 || avc444 {
        Codec::Avc420
    } else {
        Codec::None
    }
}
