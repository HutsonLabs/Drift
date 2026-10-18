//! Reconnect triggers on iOS (M7-2's platform part, IOS-3).
//!
//! iOS has no system wake notification for apps. What matters to a session is the app coming
//! back to the foreground: iOS suspends a backgrounded app and its sockets, so returning is when
//! a reconnect is due. [`ForegroundObserver`] turns
//! `UIApplicationWillEnterForegroundNotification` into the same [`Trigger::Wake`] the macOS layer
//! sends after `NSWorkspaceDidWakeNotification`.

use std::ptr::NonNull;
use std::sync::Arc;

use block2::RcBlock;
use drift_apple::network::{PathMonitor, TriggerFeed};
use drift_core::{Clock, Trigger, TriggerAction};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};
use objc2_ui_kit::UIApplicationWillEnterForegroundNotification;

/// A `UIApplicationWillEnterForegroundNotification` observer. Removed on drop.
#[derive(Debug)]
pub struct ForegroundObserver {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

// SAFETY: NSNotificationCenter is thread-safe; the token is only used to remove the observer.
unsafe impl Send for ForegroundObserver {}
// SAFETY: as above.
unsafe impl Sync for ForegroundObserver {}

impl ForegroundObserver {
    /// Calls `handler` on the posting thread (the main thread for real transitions) every time
    /// the app is about to enter the foreground.
    pub fn start(handler: impl Fn() + Send + Sync + 'static) -> Self {
        let center = NSNotificationCenter::defaultCenter();
        let block = RcBlock::new(move |_note: NonNull<NSNotification>| handler());
        // SAFETY: a constant notification name, no object filter, synchronous delivery on the
        // posting thread; the block is copied by the center and removed in `Drop`.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(UIApplicationWillEnterForegroundNotification),
                None,
                None,
                &block,
            )
        };
        Self { center, token }
    }
}

impl Drop for ForegroundObserver {
    fn drop(&mut self) {
        // SAFETY: removing the observer token we registered.
        unsafe { self.center.removeObserver(self.token.as_ref()) };
    }
}

/// Posts `UIApplicationWillEnterForegroundNotification` on the default center, as UIKit does.
///
/// For tests: it reaches every observer in the process, so the app must never call it.
#[doc(hidden)]
pub fn post_will_enter_foreground_for_tests() {
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: posting a constant notification name with no object or user info.
    unsafe { center.postNotificationName_object(UIApplicationWillEnterForegroundNotification, None) };
}

/// Path monitor + foreground observer feeding one [`TriggerFeed`] (the iOS twin of
/// `drift_macos::ReconnectTriggers`).
#[derive(Debug)]
pub struct ReconnectTriggers {
    feed: Arc<TriggerFeed>,
    _path: Option<PathMonitor>,
    _foreground: ForegroundObserver,
}

impl ReconnectTriggers {
    /// Starts both observers; `sink` receives the merged actions (from the monitor's queue or
    /// the main thread) and should forward them to every session.
    pub fn start(clock: Arc<dyn Clock>, sink: impl Fn(TriggerAction) + Send + Sync + 'static) -> Self {
        let feed = Arc::new(TriggerFeed::new(true, clock, sink));
        let f = feed.clone();
        let path = PathMonitor::start(move |status| f.path_status(status));
        let f = feed.clone();
        let foreground = ForegroundObserver::start(move || f.trigger(Trigger::Wake));
        Self { feed, _path: path, _foreground: foreground }
    }

    /// The shared feed (e.g. to inject a trigger).
    pub fn feed(&self) -> &Arc<TriggerFeed> {
        &self.feed
    }
}
