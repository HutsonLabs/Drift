//! Scroll accumulator producing fractional wheel units. Owned by task **M2-3**.
//!
//! g-r-d turns every wheel event into `value / 120 × 10 px` of smooth scrolling, fractional values
//! included (plan §1.5: 30 × +12 scrolls exactly as far as 3 × +120). So trackpad deltas are
//! converted to small wheel-unit events rather than whole notches.
//!
//! Sign conventions (RDP): vertical `+` = scroll up (towards the top of the document), horizontal
//! `+` = scroll right. AppKit's `scrollingDeltaX/Y` already include the user's *natural scrolling*
//! preference (`isDirectionInvertedFromDevice`), and a positive delta means "towards the top /
//! left", so vertical passes through and horizontal is negated. The HWHEEL sign as g-r-d applies it
//! is locked in by the `e2e_hscroll` test.

use drift_core::InputEvent;

/// Largest magnitude of one wheel event (the fast-path rotation field is 9 bits signed).
pub const MAX_UNITS_PER_EVENT: i16 = 255;

/// One wheel notch.
pub const UNITS_PER_NOTCH: i16 = 120;

/// Scroll conversion settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollConfig {
    /// Wheel units per point of precise (trackpad / Magic Mouse) delta. Default 2.
    pub units_per_point: f64,
    /// Reverse both axes on top of the system's natural-scrolling setting. Default off.
    pub reverse: bool,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self { units_per_point: 2.0, reverse: false }
    }
}

/// One `scrollWheel:` event's deltas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollDelta {
    /// `hasPreciseScrollingDeltas == true`: `scrollingDeltaX/Y` in points.
    Precise {
        /// `scrollingDeltaX`.
        dx: f64,
        /// `scrollingDeltaY`.
        dy: f64,
    },
    /// Line-based mouse wheel: `scrollingDeltaX/Y` in lines (one notch ≈ 1 line). Each line is
    /// ±120 units, sent as events of at most one notch.
    Lines {
        /// `scrollingDeltaX`.
        dx: f64,
        /// `scrollingDeltaY`.
        dy: f64,
    },
}

/// Upper bound on the units one `scrollWheel:` event may produce per axis (64 full events), so a
/// bogus delta cannot flood the connection.
const MAX_UNITS_PER_PUSH: f64 = 255.0 * 64.0;

/// Accumulates scroll deltas and emits whole wheel units, carrying the fractional remainder so
/// that nothing is lost or invented over a gesture.
#[derive(Debug, Clone, Default)]
pub struct ScrollAccumulator {
    config: ScrollConfig,
    vertical: f64,
    horizontal: f64,
}

impl ScrollAccumulator {
    /// Creates an accumulator.
    pub fn new(config: ScrollConfig) -> Self {
        Self { config, vertical: 0.0, horizontal: 0.0 }
    }

    /// The active configuration.
    pub fn config(&self) -> ScrollConfig {
        self.config
    }

    /// Adds one event's deltas and returns the wheel events to send (vertical first).
    pub fn push(&mut self, delta: ScrollDelta) -> Vec<InputEvent> {
        let sign = if self.config.reverse { -1.0 } else { 1.0 };
        let (dx, dy, per_unit, chunk) = match delta {
            ScrollDelta::Precise { dx, dy } => (dx, dy, self.config.units_per_point, MAX_UNITS_PER_EVENT),
            ScrollDelta::Lines { dx, dy } => (dx, dy, f64::from(UNITS_PER_NOTCH), UNITS_PER_NOTCH),
        };
        let mut out = Vec::new();
        emit(&mut self.vertical, sign * dy * per_unit, chunk, false, &mut out);
        emit(&mut self.horizontal, -sign * dx * per_unit, chunk, true, &mut out);
        out
    }

    /// Fractional units not yet sent, `(vertical, horizontal)`, each in `(-1, 1)`.
    pub fn residual(&self) -> (f64, f64) {
        (self.vertical, self.horizontal)
    }

    /// Drops the remainder (e.g. when a new gesture starts in the opposite direction).
    pub fn reset(&mut self) {
        self.vertical = 0.0;
        self.horizontal = 0.0;
    }
}

/// Wheel events for whole `vertical` and `horizontal` units (RDP signs), in events of at most
/// [`MAX_UNITS_PER_EVENT`], vertical first: what a server without `Drift::Mac::v1` gets for an
/// [`InputEvent::PreciseScroll`] (EXT-2).
pub fn wheel_events(vertical: i32, horizontal: i32) -> Vec<InputEvent> {
    let mut out = Vec::new();
    for (mut remaining, horizontal) in [(vertical, false), (horizontal, true)] {
        let chunk = i32::from(MAX_UNITS_PER_EVENT);
        while remaining != 0 {
            let step = remaining.clamp(-chunk, chunk);
            remaining -= step;
            // `step` is within ±255, so it fits in i16.
            out.push(InputEvent::Wheel { horizontal, units: i16::try_from(step).unwrap_or(0) });
        }
    }
    out
}

/// Sums the wheel units of `events` per axis: `(vertical, horizontal)`.
pub fn wheel_units(events: &[InputEvent]) -> (i32, i32) {
    events.iter().fold((0, 0), |(v, h), e| match e {
        InputEvent::Wheel { horizontal: false, units } => (v + i32::from(*units), h),
        InputEvent::Wheel { horizontal: true, units } => (v, h + i32::from(*units)),
        _ => (v, h),
    })
}

/// Points to hundredths of a point, saturating.
pub fn centi(points: f64) -> i32 {
    let v = (points * 100.0).round();
    if v.is_nan() {
        0
    } else {
        // Clamped to the i32 range first, so the conversion is exact.
        v.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    }
}

/// Adds `units` to `residual` and emits its whole part in events of at most `chunk` units.
fn emit(residual: &mut f64, units: f64, chunk: i16, horizontal: bool, out: &mut Vec<InputEvent>) {
    let units = if units.is_nan() { 0.0 } else { units.clamp(-MAX_UNITS_PER_PUSH, MAX_UNITS_PER_PUSH) };
    *residual += units;
    let whole = residual.trunc();
    *residual -= whole;
    // `whole` is bounded by MAX_UNITS_PER_PUSH + 1, so the conversion is exact.
    let mut remaining = whole as i32;
    let chunk = i32::from(chunk);
    while remaining != 0 {
        let step = remaining.clamp(-chunk, chunk);
        remaining -= step;
        // `step` is within ±255, so it fits in i16.
        out.push(InputEvent::Wheel { horizontal, units: step as i16 });
    }
}

#[cfg(test)]
#[path = "tests/scroll.rs"]
mod tests;
