//! Local zoom of the picture on iPhone (task **IOS-7**). Stub (Red).

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
    /// Applies one pinch step.
    pub fn apply(&mut self, _c: ZoomChange) {}
    /// Back to 1×.
    pub fn reset(&mut self) {}
    /// The view's size changed.
    pub fn set_view(&mut self, view: Size<f64>) {
        self.view = view;
    }
    /// Screen point → unzoomed view point.
    pub fn to_view(&self, screen: Point<f64>) -> Point<f64> {
        screen
    }
    /// The view transform `(scale, tx, ty)` for a centre-anchored `UIView.transform`.
    pub fn transform(&self) -> (f64, f64, f64) {
        (1.0, 0.0, 0.0)
    }
}

#[cfg(test)]
#[path = "tests/zoom.rs"]
mod tests;
