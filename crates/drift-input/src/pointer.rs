//! Pointer state for touch and pointer-lock input (task **IOS-6**). Stub (Red).

use drift_core::{DesktopSize, InputEvent, MouseButton, Point, ViewGeometry};

use crate::coords::ScaleMode;
use crate::scroll::{ScrollConfig, ScrollDelta};

/// Pointer state of one remote view.
#[derive(Debug, Clone, Default)]
pub struct Pointer {}

impl Pointer {
    /// A pointer with no desktop attached.
    pub fn new(_scroll: ScrollConfig) -> Self {
        Self {}
    }
    /// The view's geometry changed.
    pub fn set_view_geometry(&mut self, _view: ViewGeometry) {}
    /// The desktop and its placement.
    pub fn set_desktop(&mut self, _desktop: DesktopSize, _mode: ScaleMode) {}
    /// Detaches the desktop.
    pub fn clear_desktop(&mut self) {}
    /// The pointer position in desktop pixels.
    pub fn position(&self) -> Option<Point<u16>> {
        None
    }
    /// Absolute move to a view point.
    pub fn move_to(&mut self, _point: Point<f64>) -> Vec<InputEvent> {
        Vec::new()
    }
    /// Relative move by view points.
    pub fn move_by(&mut self, _dx: f64, _dy: f64) -> Vec<InputEvent> {
        Vec::new()
    }
    /// Button press or release at the current position.
    pub fn button(&mut self, _button: MouseButton, _down: bool) -> Vec<InputEvent> {
        Vec::new()
    }
    /// Scroll.
    pub fn scroll(&mut self, _delta: ScrollDelta, _began: bool) -> Vec<InputEvent> {
        Vec::new()
    }
    /// Releases held buttons.
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        Vec::new()
    }
}

#[cfg(test)]
#[path = "tests/pointer.rs"]
mod tests;
