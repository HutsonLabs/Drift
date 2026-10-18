//! Which display to capture, and display geometry.

use drift_core::{Point, Size};

/// A display as capture sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    /// `CGDirectDisplayID`.
    pub id: u32,
    /// Top-left corner in global points (CoreGraphics' top-left-origin space).
    pub origin: (f64, f64),
    /// Size in points.
    pub points: Size<u32>,
    /// Size in pixels (what the stream outputs).
    pub pixels: Size<u32>,
}

impl DisplayInfo {
    /// Pixels per point (at least 1).
    pub fn scale(&self) -> f64 {
        if self.points.width == 0 {
            return 1.0;
        }
        (f64::from(self.pixels.width) / f64::from(self.points.width)).max(1.0)
    }

    /// A global point (top-left origin) in this display's pixels; `None` outside it.
    pub fn point_to_pixels(&self, x: f64, y: f64) -> Option<Point<u32>> {
        let (dx, dy) = (x - self.origin.0, y - self.origin.1);
        if !dx.is_finite() || !dy.is_finite() || dx < 0.0 || dy < 0.0 {
            return None;
        }
        if dx >= f64::from(self.points.width) || dy >= f64::from(self.points.height) {
            return None;
        }
        let scale = self.scale();
        let px = (dx * scale).floor().min(f64::from(self.pixels.width.saturating_sub(1)));
        let py = (dy * scale).floor().min(f64::from(self.pixels.height.saturating_sub(1)));
        // Non-negative and clamped to the pixel size, so the casts are exact.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(Point::new(px as u32, py as u32))
    }
}

/// Which display a stream captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayChoice {
    /// The main display.
    #[default]
    Main,
    /// A specific `CGDirectDisplayID`.
    Id(u32),
}

/// The display `choice` names; `Main` falls back to the first display when `main_id` is not in
/// the list.
pub fn select(displays: &[DisplayInfo], choice: DisplayChoice, main_id: u32) -> Option<&DisplayInfo> {
    match choice {
        DisplayChoice::Id(id) => displays.iter().find(|d| d.id == id),
        DisplayChoice::Main => displays.iter().find(|d| d.id == main_id).or_else(|| displays.first()),
    }
}
