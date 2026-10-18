//! Client monitor layouts (Display Control) → the desktop size the host renders.

use drift_core::Size;

/// The smallest desktop side the host accepts (MS-RDPEDISP's minimum is 200 pixels).
pub const MIN_SIDE: u32 = 200;

/// The largest desktop side (MS-RDPEDISP's maximum is 8192 pixels).
pub const MAX_SIDE: u32 = 8192;

/// Clamps a requested size to `MIN_SIDE..=MAX_SIDE` and rounds each side down to an even number,
/// which H.264 4:2:0 needs.
pub fn clamp_layout(requested: Size<u32>) -> Size<u32> {
    let side = |v: u32| v.clamp(MIN_SIDE, MAX_SIDE) & !1;
    Size::new(side(requested.width), side(requested.height))
}
