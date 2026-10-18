//! ProMotion frame pacing (task **IOS-4**).
//!
//! `drift-render` presents each frame as soon as it is decoded, from the render thread. On a
//! ProMotion display iOS still picks the refresh rate per app, and without a hint it may hold
//! the app at 60 Hz. [`FramePacing`] runs a `CADisplayLink` whose preferred range is 80–120 Hz
//! while a session is on screen, which tells the system the app wants up to 120 Hz; it is paused
//! otherwise so an idle app lets the display slow down. iPhones also need
//! `CADisableMinimumFrameDurationOnPhone` in `Info.plist` to go above 60 Hz.

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{AllocAnyThread, MainThreadMarker, define_class, msg_send, sel};
use objc2_foundation::{NSDefaultRunLoopMode, NSRunLoop};
use objc2_quartz_core::{CADisplayLink, CAFrameRateRange};

/// The range asked for while a session is visible.
pub const PREFERRED_RANGE: CAFrameRateRange =
    CAFrameRateRange { minimum: 60.0, maximum: 120.0, preferred: 120.0 };

define_class!(
    /// The display link's target; the tick does nothing (the render thread presents).
    #[unsafe(super(NSObject))]
    #[name = "DriftFramePacer"]
    struct Pacer;

    impl Pacer {
        #[unsafe(method(tick:))]
        fn tick(&self, _link: &CADisplayLink) {}
    }
);

/// A paused-by-default `CADisplayLink` asking for up to 120 Hz. Invalidated on drop.
#[derive(Debug)]
pub struct FramePacing {
    link: Retained<CADisplayLink>,
}

impl FramePacing {
    /// Creates the display link on the main run loop, paused.
    pub fn new(_mtm: MainThreadMarker) -> Self {
        // SAFETY: `init` on a fresh NSObject subclass without ivars.
        let pacer: Retained<Pacer> = unsafe { msg_send![Pacer::alloc(), init] };
        // SAFETY: `tick:` takes one object argument and returns nothing, matching what
        // CADisplayLink sends; the link retains its target.
        let link = unsafe { CADisplayLink::displayLinkWithTarget_selector(&pacer, sel!(tick:)) };
        link.setPreferredFrameRateRange(PREFERRED_RANGE);
        link.setPaused(true);
        // SAFETY: the main run loop on the main thread (`_mtm`), default mode.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSDefaultRunLoopMode) };
        Self { link }
    }

    /// Runs the link while a session is visible, pauses it otherwise.
    pub fn set_active(&self, active: bool) {
        self.link.setPaused(!active);
    }

    /// Whether the link is paused.
    pub fn is_paused(&self) -> bool {
        self.link.isPaused()
    }

    /// The frame-rate range the link asks for.
    pub fn preferred_range(&self) -> CAFrameRateRange {
        self.link.preferredFrameRateRange()
    }
}

impl Drop for FramePacing {
    fn drop(&mut self) {
        self.link.invalidate();
    }
}
