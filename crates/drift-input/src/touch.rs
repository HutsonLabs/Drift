//! Touch gestures on the remote picture (task **IOS-7**).
//!
//! Fingers on an iPhone or iPad drive the remote pointer in one of two [`Mode`]s:
//!
//! | Gesture | Trackpad | Direct |
//! |---|---|---|
//! | one finger, drag | moves the pointer by the drag (×[`TRACKPAD_GAIN`]) | left-drag from where the finger went down |
//! | one finger, tap | left click where the pointer is | left click where you touch |
//! | tap, then touch again and drag | left-drag (button held) | — |
//! | one finger, long press | — | right click where you touch |
//! | two fingers, tap | right click where the pointer is | the same |
//! | two fingers, drag | scroll (natural direction, like a Mac trackpad) | the same |
//! | two fingers, pinch | local zoom, when enabled (iPhone) | the same |
//!
//! A gesture with three or more fingers is ignored, and once two fingers were down nothing more
//! happens until every finger has lifted, so the finger left over after a scroll does not move
//! the pointer. Points are **screen points** (the view's superview); direct mode maps them
//! through the local [`Zoom`]. Times are seconds on any monotonic clock.

use std::collections::BTreeMap;

use drift_core::{InputEvent, MomentumPhase, MouseButton, Point, ScrollPhase};

use crate::pointer::Pointer;
use crate::zoom::{Zoom, ZoomChange};

/// How far (points) a finger may wander and still tap.
pub const TAP_SLOP: f64 = 10.0;
/// Longest touch that still counts as a tap (seconds).
pub const TAP_MAX: f64 = 0.3;
/// Longest gap between a tap and the next touch for tap-and-drag (seconds).
pub const DOUBLE_TAP_GAP: f64 = 0.35;
/// Direct mode: a still finger held this long is a right click (seconds).
pub const LONG_PRESS: f64 = 0.6;
/// Change in finger spread (points) that makes a two-finger gesture a pinch.
pub const PINCH_SLOP: f64 = 20.0;
/// Trackpad mode: pointer points per finger point.
pub const TRACKPAD_GAIN: f64 = 1.5;

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

impl TouchOutput {
    fn events(events: Vec<InputEvent>) -> Self {
        Self { events, zoom: None }
    }
}

/// One finger in a one-finger gesture.
#[derive(Debug, Clone, Copy)]
struct One {
    id: u64,
    start: Point<f64>,
    last: Point<f64>,
    start_t: f64,
    /// Left the tap slop.
    moved: bool,
    /// Holding the left button (trackpad tap-and-drag, direct drag).
    dragging: bool,
    /// Started right after a tap (trackpad tap-and-drag candidate).
    after_tap: bool,
}

/// What a two-finger gesture turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TwoKind {
    Undecided,
    Scroll,
    Pinch,
}

#[derive(Debug, Clone, Copy)]
struct Two {
    start_t: f64,
    start_centre: Point<f64>,
    start_spread: f64,
    last_centre: Point<f64>,
    last_spread: f64,
    kind: TwoKind,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Idle,
    One(One),
    Two(Two),
    /// Wait until every finger has lifted.
    Ignore,
}

/// Touch state of one remote view.
#[derive(Debug, Clone)]
pub struct Touches {
    mode: Mode,
    zoom: bool,
    fingers: BTreeMap<u64, Point<f64>>,
    state: State,
    /// When and where the last tap ended (trackpad tap-and-drag).
    last_tap: Option<(f64, Point<f64>)>,
}

impl Default for Touches {
    fn default() -> Self {
        Self::new(Mode::default(), false)
    }
}

fn distance(a: Point<f64>, b: Point<f64>) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

fn click(pointer: &mut Pointer, button: MouseButton) -> Vec<InputEvent> {
    let mut out = pointer.button(button, true);
    out.extend(pointer.button(button, false));
    out
}

impl Touches {
    /// Touches in `mode`; `zoom` enables pinch-to-zoom (iPhone).
    pub fn new(mode: Mode, zoom: bool) -> Self {
        Self { mode, zoom, fingers: BTreeMap::new(), state: State::Idle, last_tap: None }
    }

    /// The mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Changes the mode (takes effect with the next gesture).
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// Enables or disables pinch-to-zoom.
    pub fn set_zoom(&mut self, zoom: bool) {
        self.zoom = zoom;
    }

    fn centre_and_spread(&self) -> Option<(Point<f64>, f64)> {
        let mut it = self.fingers.values();
        let (a, b) = (*it.next()?, *it.next()?);
        Some((Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0), distance(a, b)))
    }

    /// A finger touched down at screen point `at`.
    pub fn began(
        &mut self,
        pointer: &mut Pointer,
        zoom: &Zoom,
        id: u64,
        at: Point<f64>,
        t: f64,
    ) -> TouchOutput {
        self.fingers.insert(id, at);
        match (self.state, self.fingers.len()) {
            (State::Idle, 1) => {
                let after_tap = self.mode == Mode::Trackpad
                    && self.last_tap.is_some_and(|(when, where_)| {
                        t - when <= DOUBLE_TAP_GAP && distance(where_, at) <= 4.0 * TAP_SLOP
                    });
                self.state = State::One(One {
                    id,
                    start: at,
                    last: at,
                    start_t: t,
                    moved: false,
                    dragging: false,
                    after_tap,
                });
                match self.mode {
                    Mode::Direct => TouchOutput::events(pointer.move_to(zoom.to_view(at))),
                    Mode::Trackpad => TouchOutput::default(),
                }
            }
            (State::One(one), 2) => {
                let mut events = Vec::new();
                if one.dragging {
                    events.extend(pointer.button(MouseButton::Left, false));
                }
                if let Some((centre, spread)) = self.centre_and_spread() {
                    self.state = State::Two(Two {
                        start_t: t,
                        start_centre: centre,
                        start_spread: spread,
                        last_centre: centre,
                        last_spread: spread,
                        kind: TwoKind::Undecided,
                    });
                }
                TouchOutput::events(events)
            }
            _ => {
                if let State::Two(_) | State::One(_) = self.state {
                    // A third finger (or a stray second): abandon the gesture.
                    let events = pointer.button(MouseButton::Left, false);
                    self.state = State::Ignore;
                    return TouchOutput::events(events);
                }
                self.state = State::Ignore;
                TouchOutput::default()
            }
        }
    }

    /// A finger moved to screen point `at`.
    pub fn moved(
        &mut self,
        pointer: &mut Pointer,
        zoom: &Zoom,
        id: u64,
        at: Point<f64>,
        _t: f64,
    ) -> TouchOutput {
        if let Some(p) = self.fingers.get_mut(&id) {
            *p = at;
        } else {
            return TouchOutput::default();
        }
        match self.state {
            State::One(mut one) if one.id == id => {
                let mut events = Vec::new();
                if !one.moved {
                    if distance(one.start, at) <= TAP_SLOP {
                        return TouchOutput::default();
                    }
                    one.moved = true;
                    // Direct drags and trackpad tap-and-drag hold the button from here on.
                    if self.mode == Mode::Direct || one.after_tap {
                        events.extend(pointer.button(MouseButton::Left, true));
                        one.dragging = true;
                    }
                }
                match self.mode {
                    Mode::Direct => events.extend(pointer.move_to(zoom.to_view(at))),
                    Mode::Trackpad => {
                        events.extend(pointer.move_by(
                            (at.x - one.last.x) * TRACKPAD_GAIN,
                            (at.y - one.last.y) * TRACKPAD_GAIN,
                        ))
                    }
                }
                one.last = at;
                self.state = State::One(one);
                TouchOutput::events(events)
            }
            State::Two(mut two) => {
                let Some((centre, spread)) = self.centre_and_spread() else { return TouchOutput::default() };
                let mut out = TouchOutput::default();
                if two.kind == TwoKind::Undecided {
                    if (spread - two.start_spread).abs() > PINCH_SLOP {
                        two.kind = TwoKind::Pinch;
                        if !self.zoom {
                            // No local zoom (iPad): the pinch goes to the remote (EXT-2).
                            out.events.push(remote_pinch(spread, two.start_spread, ScrollPhase::Began));
                        }
                    } else if distance(centre, two.start_centre) > TAP_SLOP {
                        two.kind = TwoKind::Scroll;
                        out.events.push(pointer.precise_scroll(
                            centre.x - two.start_centre.x,
                            centre.y - two.start_centre.y,
                            ScrollPhase::Began,
                            MomentumPhase::None,
                        ));
                        two.last_centre = centre;
                    }
                }
                match two.kind {
                    TwoKind::Scroll if out.events.is_empty() => {
                        out.events.push(pointer.precise_scroll(
                            centre.x - two.last_centre.x,
                            centre.y - two.last_centre.y,
                            ScrollPhase::Changed,
                            MomentumPhase::None,
                        ));
                    }
                    TwoKind::Pinch if !self.zoom => {
                        if out.events.is_empty() {
                            out.events.push(remote_pinch(spread, two.last_spread, ScrollPhase::Changed));
                        }
                    }
                    TwoKind::Pinch => {
                        let factor = if two.last_spread > 0.0 { spread / two.last_spread } else { 1.0 };
                        out.zoom = Some(ZoomChange {
                            focus: centre,
                            factor,
                            pan: Point::new(centre.x - two.last_centre.x, centre.y - two.last_centre.y),
                        });
                    }
                    _ => {}
                }
                if two.kind != TwoKind::Undecided {
                    two.last_centre = centre;
                    two.last_spread = spread;
                }
                self.state = State::Two(two);
                out
            }
            _ => TouchOutput::default(),
        }
    }

    /// A finger lifted at screen point `at`.
    pub fn ended(
        &mut self,
        pointer: &mut Pointer,
        zoom: &Zoom,
        id: u64,
        at: Point<f64>,
        t: f64,
    ) -> TouchOutput {
        if self.fingers.remove(&id).is_none() {
            return TouchOutput::default();
        }
        let out = match self.state {
            State::One(one) if one.id == id => {
                self.state = State::Idle;
                if one.dragging {
                    let mut events = Vec::new();
                    if self.mode == Mode::Direct {
                        events.extend(pointer.move_to(zoom.to_view(at)));
                    }
                    events.extend(pointer.button(MouseButton::Left, false));
                    TouchOutput::events(events)
                } else if one.moved {
                    TouchOutput::default()
                } else {
                    let held = t - one.start_t;
                    match self.mode {
                        Mode::Direct if held >= LONG_PRESS => {
                            TouchOutput::events(click(pointer, MouseButton::Right))
                        }
                        _ if held <= TAP_MAX => {
                            self.last_tap = Some((t, at));
                            TouchOutput::events(click(pointer, MouseButton::Left))
                        }
                        Mode::Direct => TouchOutput::events(click(pointer, MouseButton::Left)),
                        Mode::Trackpad => TouchOutput::default(),
                    }
                }
            }
            State::Two(two) => {
                self.state = State::Ignore;
                match two.kind {
                    TwoKind::Undecided if t - two.start_t <= TAP_MAX => {
                        TouchOutput::events(click(pointer, MouseButton::Right))
                    }
                    // EXT-2: a Drift Host sees the fingers lift (no movement, no wheel units).
                    TwoKind::Scroll => TouchOutput::events(vec![InputEvent::PreciseScroll {
                        dx: 0,
                        dy: 0,
                        phase: ScrollPhase::Ended,
                        momentum: MomentumPhase::None,
                        wheel_v: 0,
                        wheel_h: 0,
                    }]),
                    TwoKind::Pinch if !self.zoom => TouchOutput::events(vec![InputEvent::Pinch {
                        magnification: 0,
                        phase: ScrollPhase::Ended,
                    }]),
                    _ => TouchOutput::default(),
                }
            }
            _ => TouchOutput::default(),
        };
        if self.fingers.is_empty() {
            self.state = State::Idle;
        }
        out
    }

    /// UIKit cancelled a finger (a system gesture took over): a held button is released.
    pub fn cancelled(&mut self, pointer: &mut Pointer, id: u64) -> TouchOutput {
        self.fingers.remove(&id);
        let events = match self.state {
            State::One(one) if one.dragging => pointer.button(MouseButton::Left, false),
            _ => Vec::new(),
        };
        self.state = if self.fingers.is_empty() { State::Idle } else { State::Ignore };
        TouchOutput::events(events)
    }
}

/// A pinch from `before` to `now` finger spread as a remote pinch (EXT-2), in thousandths of
/// magnification as AppKit reports it.
fn remote_pinch(now: f64, before: f64, phase: ScrollPhase) -> InputEvent {
    let change = if before > 0.0 { now / before - 1.0 } else { 0.0 };
    let milli = (change * 1000.0).round();
    let magnification = if milli.is_nan() { 0 } else { milli.clamp(-1_000_000.0, 1_000_000.0) as i32 };
    InputEvent::Pinch { magnification, phase }
}

#[cfg(test)]
#[path = "tests/touch.rs"]
mod tests;
