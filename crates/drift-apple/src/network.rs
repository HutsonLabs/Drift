//! Network reachability shared by the macOS and iOS layers (task **M7-2**, moved in IOS-3).
//!
//! * [`PathMonitor`] wraps Network.framework's `nw_path_monitor` (the path status of the default
//!   route) and reports [`PathStatus`] changes on a private dispatch queue.
//! * [`TriggerFeed`] turns path changes and the platform's "wake" signal (`drift-macos`: the
//!   `NSWorkspace` wake; `drift-ios`: returning to the foreground) into [`Trigger`]s and runs
//!   them through the pure [`TriggerMerger`] (drift-core), so the app receives at most one
//!   [`TriggerAction`] per real change.

use std::ffi::{c_int, c_void};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use block2::{DynBlock, RcBlock};
use dispatch2::{DispatchQueue, DispatchRetained};
use drift_core::{Clock, Trigger, TriggerAction, TriggerMerger};

/// `nw_path_status_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathStatus {
    /// `nw_path_status_invalid`: not yet determined.
    Invalid,
    /// `nw_path_status_satisfied`: usable route.
    Satisfied,
    /// `nw_path_status_unsatisfied`: no route.
    Unsatisfied,
    /// `nw_path_status_satisfiable`: a route may appear if a connection is attempted
    /// (VPN on demand, cellular); treated as online so the reconnect attempt brings it up.
    Satisfiable,
}

impl PathStatus {
    /// Converts the raw `nw_path_status_t` value (unknown values are `Invalid`).
    pub const fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Self::Satisfied,
            2 => Self::Unsatisfied,
            3 => Self::Satisfiable,
            _ => Self::Invalid,
        }
    }

    /// The reconnect trigger for this status (`None` for `Invalid`).
    pub const fn trigger(self) -> Option<Trigger> {
        match self {
            Self::Satisfied | Self::Satisfiable => Some(Trigger::NetworkOnline),
            Self::Unsatisfied => Some(Trigger::NetworkOffline),
            Self::Invalid => None,
        }
    }
}

type ActionSink = Box<dyn Fn(TriggerAction) + Send + Sync>;

/// Merges platform triggers and forwards the resulting actions.
pub struct TriggerFeed {
    merger: Mutex<TriggerMerger>,
    clock: Arc<dyn Clock>,
    sink: ActionSink,
}

impl std::fmt::Debug for TriggerFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TriggerFeed").field("online", &self.is_online()).finish_non_exhaustive()
    }
}

impl TriggerFeed {
    /// A feed with the default debounce. `online` is the initial reachability.
    pub fn new(
        online: bool,
        clock: Arc<dyn Clock>,
        sink: impl Fn(TriggerAction) + Send + Sync + 'static,
    ) -> Self {
        Self { merger: Mutex::new(TriggerMerger::new(online)), clock, sink: Box::new(sink) }
    }

    /// Feeds one trigger. The sink runs after the internal lock is released.
    pub fn trigger(&self, trigger: Trigger) {
        let now = self.clock.now();
        let action = match self.merger.lock() {
            Ok(mut m) => m.on_trigger(trigger, now),
            Err(poisoned) => poisoned.into_inner().on_trigger(trigger, now),
        };
        if let Some(action) = action {
            (self.sink)(action);
        }
    }

    /// Feeds a path status update.
    pub fn path_status(&self, status: PathStatus) {
        if let Some(trigger) = status.trigger() {
            self.trigger(trigger);
        }
    }

    /// Whether the network is currently considered reachable.
    pub fn is_online(&self) -> bool {
        match self.merger.lock() {
            Ok(m) => m.is_online(),
            Err(poisoned) => poisoned.into_inner().is_online(),
        }
    }
}

#[link(name = "Network", kind = "framework")]
unsafe extern "C" {
    fn nw_path_monitor_create() -> *mut c_void;
    fn nw_path_monitor_set_queue(monitor: *mut c_void, queue: NonNull<DispatchQueue>);
    fn nw_path_monitor_set_update_handler(monitor: *mut c_void, handler: &DynBlock<dyn Fn(*mut c_void)>);
    fn nw_path_monitor_start(monitor: *mut c_void);
    fn nw_path_monitor_cancel(monitor: *mut c_void);
    fn nw_path_get_status(path: *mut c_void) -> c_int;
    fn nw_release(object: *mut c_void);
}

/// A running `nw_path_monitor`. Cancelled on drop.
#[derive(Debug)]
pub struct PathMonitor {
    monitor: NonNull<c_void>,
    _queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: Network.framework objects are thread-safe reference-counted objects; we only call
// `nw_path_monitor_cancel` and `nw_release` on the handle after construction.
unsafe impl Send for PathMonitor {}
// SAFETY: as above; no method takes `&self` and mutates through the handle.
unsafe impl Sync for PathMonitor {}

impl PathMonitor {
    /// Starts monitoring; `handler` is called on a private serial queue with the initial status
    /// and every change. Returns `None` if Network.framework cannot create a monitor.
    pub fn start(handler: impl Fn(PathStatus) + Send + Sync + 'static) -> Option<Self> {
        let handler: Arc<dyn Fn(PathStatus) + Send + Sync> = Arc::new(handler);
        let queue = DispatchQueue::new("com.hutsonlabs.drift.path-monitor", None);
        // SAFETY: plain constructor; returns a +1 object or null.
        let monitor = NonNull::new(unsafe { nw_path_monitor_create() })?;
        let block = RcBlock::new(move |path: *mut c_void| {
            // SAFETY: Network.framework passes a valid `nw_path_t` for the duration of the call.
            let raw = if path.is_null() { 0 } else { unsafe { nw_path_get_status(path) } };
            handler(PathStatus::from_raw(raw));
        });
        // SAFETY: `monitor` is a live monitor; the queue is retained by the monitor and by us;
        // the block is copied by Network.framework and only captures Send + Sync state.
        unsafe {
            nw_path_monitor_set_queue(monitor.as_ptr(), DispatchRetained::as_ptr(&queue));
            nw_path_monitor_set_update_handler(monitor.as_ptr(), &block);
            nw_path_monitor_start(monitor.as_ptr());
        }
        Some(Self { monitor, _queue: queue })
    }
}

impl Drop for PathMonitor {
    fn drop(&mut self) {
        // SAFETY: we own one reference to a live monitor; cancel stops future callbacks.
        unsafe {
            nw_path_monitor_cancel(self.monitor.as_ptr());
            nw_release(self.monitor.as_ptr());
        }
    }
}
