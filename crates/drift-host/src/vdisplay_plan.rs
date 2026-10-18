//! Client layouts → virtual-display modes (task **HOST-10**, ADR `HOST-10-virtual-display`).
//! Pure: the private-API side ([`crate::cg_virtual`]) only applies a [`ModePlan`].

use drift_core::Size;

use crate::display::LayoutRequest;

/// The largest virtual display the host makes (5K). Larger layouts shrink, keeping their shape:
/// the encoder holds 60 fps up to 4K (HOST-4) and nothing larger is worth the bandwidth.
pub const MAX_PIXELS: Size<u32> = Size::new(5120, 2880);

/// The smallest virtual display (macOS offers nothing smaller in Displays settings).
pub const MIN_PIXELS: Size<u32> = Size::new(640, 480);

/// Desktop scale (percent) from which the client is treated as Retina and gets a HiDPI mode.
pub const HIDPI_FROM_PERCENT: u32 = 150;

/// The refresh rate of every mode.
pub const REFRESH_HZ: u32 = 60;

/// A virtual display's mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModePlan {
    /// Size in pixels: what capture and the encoder see.
    pub pixels: Size<u32>,
    /// Size in points: what macOS lays windows out in (half the pixels for HiDPI).
    pub points: Size<u32>,
    /// A HiDPI (Retina) mode.
    pub hidpi: bool,
    /// Refresh rate.
    pub refresh_hz: u32,
    /// Physical size reported to macOS, in millimetres (drives its default scaling).
    pub size_mm: (u32, u32),
}

impl ModePlan {
    /// Whether going from `current` to `next` needs the display changed.
    pub fn needs_change(current: Option<&ModePlan>, next: &ModePlan) -> bool {
        current != Some(next)
    }
}

/// The mode for a client's layout.
pub fn plan(request: &LayoutRequest) -> ModePlan {
    let (mut w, mut h) = (f64::from(request.pixels.width.max(1)), f64::from(request.pixels.height.max(1)));
    let fit = (f64::from(MAX_PIXELS.width) / w).min(f64::from(MAX_PIXELS.height) / h);
    if fit < 1.0 {
        w = (w * fit).floor();
        h = (h * fit).floor();
    }
    // Finite, positive and at most MAX_PIXELS, so the casts are exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pixels = Size::new((w as u32).max(MIN_PIXELS.width) & !1, (h as u32).max(MIN_PIXELS.height) & !1);
    let hidpi = request.desktop_scale.unwrap_or(100) >= HIDPI_FROM_PERCENT;
    let points = if hidpi { Size::new(pixels.width / 2, pixels.height / 2) } else { pixels };
    let size_mm = request.physical_mm.unwrap_or_else(|| {
        let ppi = if hidpi { 220.0 } else { 110.0 };
        let mm = |px: u32| (f64::from(px) / ppi * 25.4).round();
        // Small positive values, so the casts are exact.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        (mm(pixels.width) as u32, mm(pixels.height) as u32)
    });
    ModePlan { pixels, points, hidpi, refresh_hz: REFRESH_HZ, size_mm }
}
