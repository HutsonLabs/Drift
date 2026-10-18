//! Reconnect triggers from the OS (task **M7-2**, platform part).
//!
//! * [`PathMonitor`], [`PathStatus`] and [`TriggerFeed`] live in `drift-apple` (shared with iOS)
//!   and are re-exported here.
//! * [`WakeObserver`] observes `NSWorkspaceDidWakeNotification`.
//! * [`ReconnectTriggers`] owns the monitor, the observer and the feed for the app's lifetime.

use std::ptr::NonNull;
use std::sync::Arc;

use block2::RcBlock;
pub use drift_apple::network::{PathMonitor, PathStatus, TriggerFeed};
use drift_core::{Clock, Trigger, TriggerAction};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};

/// An `NSWorkspaceDidWakeNotification` observer. Removed on drop.
#[derive(Debug)]
pub struct WakeObserver {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

// SAFETY: NSNotificationCenter is thread-safe; the token is only used to remove the observer.
unsafe impl Send for WakeObserver {}
// SAFETY: as above.
unsafe impl Sync for WakeObserver {}

impl WakeObserver {
    /// Calls `handler` (on the posting thread, the main thread for real wakes) after every wake.
    pub fn start(handler: impl Fn() + Send + Sync + 'static) -> Self {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let block = RcBlock::new(move |_note: NonNull<NSNotification>| handler());
        // SAFETY: a constant notification name, no object filter, synchronous delivery on the
        // posting thread; the block is copied by the center and removed in `Drop`.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                None,
                &block,
            )
        };
        Self { center, token }
    }
}

impl Drop for WakeObserver {
    fn drop(&mut self) {
        // SAFETY: removing the observer token we registered.
        unsafe { self.center.removeObserver(self.token.as_ref()) };
    }
}

/// Path monitor + wake observer feeding one [`TriggerFeed`].
#[derive(Debug)]
pub struct ReconnectTriggers {
    feed: Arc<TriggerFeed>,
    _path: Option<PathMonitor>,
    _wake: WakeObserver,
}

impl ReconnectTriggers {
    /// Starts both observers; `sink` receives the merged actions (from the monitor's queue or
    /// the main thread) and should forward them to every session (`NetworkReachable`,
    /// `ReconnectNow`).
    pub fn start(clock: Arc<dyn Clock>, sink: impl Fn(TriggerAction) + Send + Sync + 'static) -> Self {
        let feed = Arc::new(TriggerFeed::new(true, clock, sink));
        let f = feed.clone();
        let path = PathMonitor::start(move |status| f.path_status(status));
        let f = feed.clone();
        let wake = WakeObserver::start(move || f.trigger(Trigger::Wake));
        Self { feed, _path: path, _wake: wake }
    }

    /// The shared feed (e.g. to inject a trigger).
    pub fn feed(&self) -> &Arc<TriggerFeed> {
        &self.feed
    }
}
