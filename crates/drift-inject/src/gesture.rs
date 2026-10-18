//! Precise scrolling and trackpad gestures from a Drift client, as Mac events (task **EXT-2**;
//! ADR `EXT-2-scroll-gestures`). Pure.
//!
//! Public CGEvent cannot synthesize trackpad gestures, so swipes become the keyboard shortcuts
//! macOS assigns to the same actions (System Settings › Keyboard › Shortcuts › Mission Control):
//!
//! | Gesture | Shortcut | Action |
//! |---|---|---|
//! | swipe up (3–4 fingers) | Control-Up | Mission Control |
//! | swipe down | Control-Down | App Exposé |
//! | swipe left | Control-Right | the Space on the right |
//! | swipe right | Control-Left | the Space on the left |
//! | pinch, every [`PINCH_STEP`] | Command-= / Command-- | zoom in / out in the front app |
//! | smart zoom | — | none: no shortcut does it everywhere |
//!
//! Precise scrolls become pixel scroll events with their phase, so momentum and rubber-banding
//! work as they do on the Mac itself.

use drift_core::{MomentumPhase, ScrollPhase as WirePhase, SwipeDirection};
use drift_ext::gestures::Gesture;
use drift_input::keymap::kvk;

use crate::plan::{InjectEvent, ModFlags, ScrollPhase};

/// Magnification (thousandths) a pinch must add up to for one zoom shortcut: 0.25, so a full
/// two-finger spread zooms two or three steps, about what a trackpad pinch does in Safari.
pub const PINCH_STEP: u32 = 250;

/// Plans gestures for one session; keeps the current pinch's progress.
#[derive(Debug, Default)]
pub struct GesturePlanner {
    /// Magnification (thousandths) of the current pinch not yet turned into a shortcut.
    pinch: i32,
}

fn chord(modifier: u16, flag: ModFlags, key: u16) -> Vec<InjectEvent> {
    vec![
        InjectEvent::Key { kvk: modifier, down: true, flags: flag },
        InjectEvent::Key { kvk: key, down: true, flags: flag },
        InjectEvent::Key { kvk: key, down: false, flags: flag },
        InjectEvent::Key { kvk: modifier, down: false, flags: ModFlags::NONE },
    ]
}

impl GesturePlanner {
    /// The events to post for `gesture`.
    pub fn plan(&mut self, gesture: &Gesture) -> Vec<InjectEvent> {
        match *gesture {
            Gesture::Scroll { dx, dy, phase, momentum } => {
                let phase = match (phase, momentum) {
                    (_, MomentumPhase::Began) => ScrollPhase::MomentumBegan,
                    (_, MomentumPhase::Changed) => ScrollPhase::Momentum,
                    (_, MomentumPhase::Ended) => ScrollPhase::MomentumEnded,
                    (WirePhase::MayBegin, MomentumPhase::None) => return Vec::new(),
                    (WirePhase::None, MomentumPhase::None) => ScrollPhase::None,
                    (WirePhase::Began, MomentumPhase::None) => ScrollPhase::Began,
                    (WirePhase::Changed, MomentumPhase::None) => ScrollPhase::Changed,
                    (WirePhase::Ended, MomentumPhase::None) => ScrollPhase::Ended,
                    (WirePhase::Cancelled, MomentumPhase::None) => ScrollPhase::Cancelled,
                };
                vec![InjectEvent::Scroll { dx: f64::from(dx) / 100.0, dy: f64::from(dy) / 100.0, phase }]
            }
            Gesture::Swipe { direction, .. } => {
                let key = match direction {
                    SwipeDirection::Up => kvk::UP_ARROW,
                    SwipeDirection::Down => kvk::DOWN_ARROW,
                    SwipeDirection::Left => kvk::RIGHT_ARROW,
                    SwipeDirection::Right => kvk::LEFT_ARROW,
                };
                chord(kvk::CONTROL, ModFlags::CONTROL, key)
            }
            Gesture::Pinch { magnification, phase } => {
                if phase == WirePhase::Began {
                    self.pinch = 0;
                }
                self.pinch = self.pinch.saturating_add(magnification);
                let step = i32::try_from(PINCH_STEP).unwrap_or(i32::MAX);
                let mut out = Vec::new();
                while self.pinch >= step {
                    self.pinch -= step;
                    out.extend(chord(kvk::COMMAND, ModFlags::COMMAND, kvk::ANSI_EQUAL));
                }
                while self.pinch <= -step {
                    self.pinch += step;
                    out.extend(chord(kvk::COMMAND, ModFlags::COMMAND, kvk::ANSI_MINUS));
                }
                if matches!(phase, WirePhase::Ended | WirePhase::Cancelled) {
                    self.pinch = 0;
                }
                out
            }
            Gesture::SmartZoom => Vec::new(),
        }
    }
}
