//! Local zoom of the picture on iPhone (task **IOS-7**).
//!
//! A phone screen shows a whole desktop at a size where text is hard to hit, so a pinch
//! magnifies the picture locally (the remote desktop size does not change). [`Zoom`] keeps the
//! magnification and the visible region in *unzoomed view points*: the view itself is
//! transformed ([`Zoom::transform`]), so UIKit still reports touches in unzoomed view
//! coordinates, and screen points go through [`Zoom::to_view`] for the touch state machine.

use drift_core::{Point, Size};

/// One pinch step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomChange {
    /// Pinch centre, in screen points.
    pub focus: Point<f64>,
    /// Spread ratio since the last step.
    pub factor: f64,
    /// Centre movement since the last step, in screen points.
    pub pan: Point<f64>,
}

/// Zoom state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    /// Magnification, 1 = none.
    pub scale: f64,
    /// Top-left of the visible region in unzoomed view points.
    pub offset: Point<f64>,
    /// View size in points.
    pub view: Size<f64>,
}

impl Default for Zoom {
    fn default() -> Self {
        Self { scale: 1.0, offset: Point::new(0.0, 0.0), view: Size::new(0.0, 0.0) }
    }
}

impl Zoom {
    /// Largest magnification.
    pub const MAX_SCALE: f64 = 4.0;
    /// Applies one pinch step: scales about the pinch centre (which stays under the fingers),
    /// then follows the centre's movement. The scale stays in `1..=MAX_SCALE` and the visible
    /// region inside the picture.
    pub fn apply(&mut self, c: ZoomChange) {
        let factor = if c.factor.is_finite() && c.factor > 0.0 { c.factor } else { 1.0 };
        let scale = (self.scale * factor).clamp(1.0, Self::MAX_SCALE);
        let anchor = self.to_view(c.focus);
        self.offset = Point::new(anchor.x - c.focus.x / scale, anchor.y - c.focus.y / scale);
        self.scale = scale;
        if c.pan.x.is_finite() && c.pan.y.is_finite() {
            self.offset = Point::new(self.offset.x - c.pan.x / scale, self.offset.y - c.pan.y / scale);
        }
        self.clamp();
    }

    /// Back to 1×.
    pub fn reset(&mut self) {
        self.scale = 1.0;
        self.offset = Point::new(0.0, 0.0);
    }

    /// The view's size changed.
    pub fn set_view(&mut self, view: Size<f64>) {
        self.view = view;
        self.clamp();
    }

    /// Whether the picture is magnified.
    pub fn is_zoomed(&self) -> bool {
        self.scale > 1.0
    }

    fn clamp(&mut self) {
        let max_x = (self.view.width - self.view.width / self.scale).max(0.0);
        let max_y = (self.view.height - self.view.height / self.scale).max(0.0);
        self.offset = Point::new(self.offset.x.clamp(0.0, max_x), self.offset.y.clamp(0.0, max_y));
    }

    /// Screen point → unzoomed view point.
    pub fn to_view(&self, screen: Point<f64>) -> Point<f64> {
        Point::new(self.offset.x + screen.x / self.scale, self.offset.y + screen.y / self.scale)
    }

    /// The view transform `(scale, tx, ty)` for a `UIView.transform`, which UIKit applies about
    /// the view's centre: view point `p` lands on screen at `s·(p − offset)`.
    pub fn transform(&self) -> (f64, f64, f64) {
        let (cx, cy) = (self.view.width / 2.0, self.view.height / 2.0);
        let s = self.scale;
        // 0.0 + x turns a -0.0 into 0.0, so the identity compares equal.
        (s, 0.0 + s * (cx - self.offset.x) - cx, 0.0 + s * (cy - self.offset.y) - cy)
    }
}

#[cfg(test)]
#[path = "tests/zoom.rs"]
mod tests;
