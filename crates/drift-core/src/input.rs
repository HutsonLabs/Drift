//! Wire-level input events (plan §3).
//!
//! Produced by `drift-input` (pure mapping) from `drift-macos` NSEvents and consumed by
//! the session actor, which encodes them as fast-path input PDUs.

use serde::{Deserialize, Serialize};

/// Pointer buttons supported by RDP fast-path input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Secondary button.
    Right,
    /// Middle button / wheel click.
    Middle,
    /// Extended button 1 (back).
    X1,
    /// Extended button 2 (forward).
    X2,
}

/// Where a trackpad scroll or a pinch is (AppKit's `NSEvent.phase`; EXT-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum ScrollPhase {
    /// No phase: a mouse wheel, or a momentum event.
    None,
    /// Fingers rest on the trackpad; nothing has moved yet.
    MayBegin,
    /// The gesture started.
    Began,
    /// The gesture continues.
    Changed,
    /// Fingers lifted.
    Ended,
    /// The system cancelled the gesture.
    Cancelled,
}

/// Momentum after the fingers lifted (AppKit's `NSEvent.momentumPhase`; EXT-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum MomentumPhase {
    /// Not a momentum event.
    None,
    /// Momentum starts.
    Began,
    /// Momentum continues.
    Changed,
    /// Momentum ends.
    Ended,
}

/// Which way the fingers moved in a swipe (EXT-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "kebab-case")]
pub enum SwipeDirection {
    /// Towards the left.
    Left,
    /// Towards the right.
    Right,
    /// Upwards.
    Up,
    /// Downwards.
    Down,
}

/// One input event in remote desktop coordinates (plan §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum InputEvent {
    /// Set-1 scancode key event (positional; the remote XKB layout picks the character).
    Key {
        /// Scancode (without the `0xE0` prefix).
        scancode: u8,
        /// Extended (`0xE0`-prefixed) key.
        extended: bool,
        /// Press (`true`) or release.
        down: bool,
    },
    /// Unicode key event (layout-independent), one UTF-16 code unit.
    Unicode {
        /// UTF-16 code unit; surrogate pairs are sent as two events.
        ch: u16,
        /// Press (`true`) or release.
        down: bool,
    },
    /// Absolute pointer move in desktop pixels.
    MouseMove {
        /// X in desktop pixels.
        x: u16,
        /// Y in desktop pixels.
        y: u16,
    },
    /// Pointer button press/release at a desktop position.
    MouseButton {
        /// Which button.
        button: MouseButton,
        /// Press (`true`) or release.
        down: bool,
        /// X in desktop pixels.
        x: u16,
        /// Y in desktop pixels.
        y: u16,
    },
    /// Wheel rotation. Fractional-capable: any value in `-255..=255` (120 = one notch;
    /// positive = scroll up / right before g-r-d's HWHEEL sign inversion, see M2-3).
    Wheel {
        /// Horizontal (`PTR_FLAGS_HWHEEL`) instead of vertical.
        horizontal: bool,
        /// Wheel units, clamped to `-255..=255` by the producer.
        units: i16,
    },
    /// Synchronize lock-key toggle state.
    SyncToggles {
        /// Caps Lock on.
        caps: bool,
        /// Num Lock on.
        num: bool,
    },
    /// Release every key and button the remote believes is held (focus loss, reconnect).
    ReleaseAll,
    /// A trackpad or touch scroll with its phase and momentum (EXT-2). A Drift Host gets it on
    /// `Drift::Mac::v1`; any other server gets the wheel units it carries instead.
    PreciseScroll {
        /// Horizontal delta in hundredths of a point; positive scrolls towards the left
        /// (AppKit's `scrollingDeltaX`, after the profile's reverse setting).
        dx: i32,
        /// Vertical delta in hundredths of a point; positive scrolls towards the top.
        dy: i32,
        /// Scroll phase.
        phase: ScrollPhase,
        /// Momentum phase.
        momentum: MomentumPhase,
        /// Vertical wheel units plain RDP gets instead (RDP sign: positive scrolls up).
        wheel_v: i32,
        /// Horizontal wheel units plain RDP gets instead (RDP sign: positive scrolls right).
        wheel_h: i32,
    },
    /// A multi-finger swipe (EXT-2). Only a Drift Host gets it; plain RDP has no equivalent.
    Swipe {
        /// Which way the fingers moved.
        direction: SwipeDirection,
        /// Fingers on the trackpad (3 or 4).
        fingers: u8,
    },
    /// A pinch (EXT-2): change in magnification since the last event, in thousandths.
    Pinch {
        /// `NSEvent.magnification × 1000` (positive zooms in).
        magnification: i32,
        /// Gesture phase.
        phase: ScrollPhase,
    },
    /// A two-finger double tap (smart zoom; EXT-2).
    SmartZoom,
}
