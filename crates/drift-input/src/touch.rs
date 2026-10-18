//! Touch gestures on the remote picture (task **IOS-7**). Stub (Red).

use drift_core::{InputEvent, Point};

use crate::pointer::Pointer;
use crate::zoom::{Zoom, ZoomChange};

/// How a finger drives the remote pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// The screen is a trackpad: relative pointer, taps click where the pointer is.
    #[default]
    Trackpad,
    /// Tap where you touch.
    Direct,
}

/// What one touch callback produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TouchOutput {
    /// Events for the session.
    pub events: Vec<InputEvent>,
    /// A change to the local zoom.
    pub zoom: Option<ZoomChange>,
}

/// Touch state of one remote view.
#[derive(Debug, Clone, Default)]
pub struct Touches {
    _mode: Mode,
}

impl Touches {
    /// Touches in `mode`; `zoom` enables pinch-to-zoom.
    pub fn new(mode: Mode, _zoom: bool) -> Self {
        Self { _mode: mode }
    }
    /// The mode.
    pub fn mode(&self) -> Mode {
        self._mode
    }
    /// Changes the mode.
    pub fn set_mode(&mut self, mode: Mode) {
        self._mode = mode;
    }
    /// A finger touched down.
    pub fn began(&mut self, _p: &mut Pointer, _z: &Zoom, _id: u64, _at: Point<f64>, _t: f64) -> TouchOutput {
        TouchOutput::default()
    }
    /// A finger moved.
    pub fn moved(&mut self, _p: &mut Pointer, _z: &Zoom, _id: u64, _at: Point<f64>, _t: f64) -> TouchOutput {
        TouchOutput::default()
    }
    /// A finger lifted.
    pub fn ended(&mut self, _p: &mut Pointer, _z: &Zoom, _id: u64, _at: Point<f64>, _t: f64) -> TouchOutput {
        TouchOutput::default()
    }
    /// UIKit cancelled a finger.
    pub fn cancelled(&mut self, _p: &mut Pointer, _id: u64) -> TouchOutput {
        TouchOutput::default()
    }
}

#[cfg(test)]
#[path = "tests/touch.rs"]
mod tests;
