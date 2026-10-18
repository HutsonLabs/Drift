//! Pointer state for iPad pointers, pointer lock and touch (tasks **IOS-6**, **IOS-7**).
//!
//! macOS always knows where the pointer is ([`crate::coords`] maps every `mouseMoved:`). On iOS
//! the pointer is sometimes absolute (an iPad pointer or Apple Pencil hovering over the picture,
//! a direct touch) and sometimes relative (`GCMouse` deltas under pointer lock, a finger on the
//! trackpad). RDP fast-path has only absolute moves, so [`Pointer`] keeps the position in
//! fractional desktop pixels, turns relative motion into absolute moves clamped to the desktop,
//! and presses buttons where the pointer is.

use std::collections::BTreeSet;

use drift_core::{
    DesktopSize, InputEvent, MomentumPhase, MouseButton, Point, ScrollPhase, Size, ViewGeometry,
};

use crate::coords::{ScaleMode, Viewport};
use crate::scroll::{ScrollAccumulator, ScrollConfig, ScrollDelta, centi, wheel_units};

/// Pointer state of one remote view.
#[derive(Debug, Clone)]
pub struct Pointer {
    scroll: ScrollAccumulator,
    view: ViewGeometry,
    desktop: Option<(DesktopSize, ScaleMode)>,
    viewport: Option<Viewport>,
    /// Position in fractional desktop pixels (relative moves accumulate here).
    position: Option<Point<f64>>,
    /// Last position sent, in whole pixels.
    sent: Option<Point<u16>>,
    /// Held buttons, by [`button_index`].
    buttons: BTreeSet<u8>,
}

impl Default for Pointer {
    fn default() -> Self {
        Self::new(ScrollConfig::default())
    }
}

fn button_index(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::X1 => 3,
        MouseButton::X2 => 4,
    }
}

fn button_from_index(i: u8) -> MouseButton {
    match i {
        0 => MouseButton::Left,
        1 => MouseButton::Right,
        2 => MouseButton::Middle,
        3 => MouseButton::X1,
        _ => MouseButton::X2,
    }
}

impl Pointer {
    /// A pointer with no desktop attached: everything is dropped until
    /// [`Pointer::set_desktop`].
    pub fn new(scroll: ScrollConfig) -> Self {
        Self {
            scroll: ScrollAccumulator::new(scroll),
            view: ViewGeometry { points: Size::new(0.0, 0.0), backing_scale: 1.0 },
            desktop: None,
            viewport: None,
            position: None,
            sent: None,
            buttons: BTreeSet::new(),
        }
    }

    /// The view's size or scale changed.
    pub fn set_view_geometry(&mut self, view: ViewGeometry) {
        self.view = view;
        self.update_viewport();
    }

    /// The remote desktop size and how it is placed in the view.
    pub fn set_desktop(&mut self, desktop: DesktopSize, mode: ScaleMode) {
        self.desktop = Some((desktop, mode));
        self.update_viewport();
    }

    /// Detaches the desktop (disconnected): the pointer is forgotten.
    pub fn clear_desktop(&mut self) {
        self.desktop = None;
        self.viewport = None;
        self.position = None;
        self.sent = None;
    }

    fn update_viewport(&mut self) {
        self.viewport = self.desktop.map(|(desktop, mode)| Viewport::new(self.view, desktop, mode));
        if let (Some(v), Some(p)) = (self.viewport, self.position) {
            self.position = Some(clamp(p, v.desktop));
        }
    }

    /// The current viewport (where the desktop is drawn), if a desktop is attached.
    pub fn viewport(&self) -> Option<Viewport> {
        self.viewport
    }

    /// The pointer position in desktop pixels, once it has been placed.
    pub fn position(&self) -> Option<Point<u16>> {
        let v = self.viewport?;
        self.position.map(|p| whole(p, v.desktop))
    }

    /// Absolute move to a view point (iPad pointer, Pencil hover, direct touch).
    pub fn move_to(&mut self, point: Point<f64>) -> Vec<InputEvent> {
        let Some(v) = self.viewport else { return Vec::new() };
        let Some(p) = v.view_to_desktop(point) else { return Vec::new() };
        self.position = Some(Point::new(f64::from(p.x), f64::from(p.y)));
        self.emit_move()
    }

    /// Relative move by view points (pointer lock, trackpad). Starts from the desktop centre the
    /// first time; fractions of a pixel accumulate.
    pub fn move_by(&mut self, dx: f64, dy: f64) -> Vec<InputEvent> {
        let Some(v) = self.viewport else { return Vec::new() };
        if !dx.is_finite() || !dy.is_finite() {
            return Vec::new();
        }
        let start = self
            .position
            .unwrap_or_else(|| Point::new(f64::from(v.desktop.width / 2), f64::from(v.desktop.height / 2)));
        let scale = v.points_per_pixel;
        self.position = Some(clamp(Point::new(start.x + dx / scale, start.y + dy / scale), v.desktop));
        self.emit_move()
    }

    fn emit_move(&mut self) -> Vec<InputEvent> {
        let Some(p) = self.position() else { return Vec::new() };
        if self.sent == Some(p) {
            return Vec::new();
        }
        self.sent = Some(p);
        vec![InputEvent::MouseMove { x: p.x, y: p.y }]
    }

    /// Presses or releases `button` where the pointer is. A press of a held button or a release
    /// of one not held is dropped.
    pub fn button(&mut self, button: MouseButton, down: bool) -> Vec<InputEvent> {
        let Some(p) = self.position() else { return Vec::new() };
        let index = button_index(button);
        let changed = if down { self.buttons.insert(index) } else { self.buttons.remove(&index) };
        if !changed {
            return Vec::new();
        }
        vec![InputEvent::MouseButton { button, down, x: p.x, y: p.y }]
    }

    /// Whether `button` is held on the remote.
    pub fn is_held(&self, button: MouseButton) -> bool {
        self.buttons.contains(&button_index(button))
    }

    /// Scroll; `began` drops the remainder of the previous gesture.
    pub fn scroll(&mut self, delta: ScrollDelta, began: bool) -> Vec<InputEvent> {
        if began {
            self.scroll.reset();
        }
        self.scroll.push(delta)
    }

    /// A precise (trackpad or touch) scroll of `dx`, `dy` points (AppKit's `scrollingDelta`
    /// signs) as one [`InputEvent::PreciseScroll`] (EXT-2): the points and phases for a Drift
    /// Host, and the same wheel units [`Self::scroll`] would send for anyone else.
    pub fn precise_scroll(
        &mut self,
        dx: f64,
        dy: f64,
        phase: ScrollPhase,
        momentum: MomentumPhase,
    ) -> InputEvent {
        // A new gesture drops the previous one's remainder, as `scroll(…, began)` does.
        let fallback = self.scroll(ScrollDelta::Precise { dx, dy }, phase == ScrollPhase::Began);
        let (wheel_v, wheel_h) = wheel_units(&fallback);
        let sign = if self.scroll.config().reverse { -1.0 } else { 1.0 };
        InputEvent::PreciseScroll {
            dx: centi(sign * dx),
            dy: centi(sign * dy),
            phase,
            momentum,
            wheel_v,
            wheel_h,
        }
    }

    /// Releases every held button where the pointer is (focus loss, background).
    pub fn release_all(&mut self) -> Vec<InputEvent> {
        let buttons = std::mem::take(&mut self.buttons);
        self.scroll.reset();
        let Some(p) = self.position() else { return Vec::new() };
        buttons
            .into_iter()
            .map(|i| InputEvent::MouseButton { button: button_from_index(i), down: false, x: p.x, y: p.y })
            .collect()
    }
}

/// Keeps a fractional position inside the desktop.
fn clamp(p: Point<f64>, desktop: DesktopSize) -> Point<f64> {
    let max_x = f64::from(desktop.width.saturating_sub(1));
    let max_y = f64::from(desktop.height.saturating_sub(1));
    Point::new(p.x.clamp(0.0, max_x), p.y.clamp(0.0, max_y))
}

/// The whole pixel a clamped fractional position lies in.
fn whole(p: Point<f64>, desktop: DesktopSize) -> Point<u16> {
    let c = clamp(p, desktop);
    // Clamped to the desktop, whose size fits u16 in RDP; `min` keeps the cast exact anyway.
    let to = |v: f64| v.floor().min(f64::from(u16::MAX)) as u16;
    Point::new(to(c.x), to(c.y))
}

#[cfg(test)]
#[path = "tests/pointer.rs"]
mod tests;
