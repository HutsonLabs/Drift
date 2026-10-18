//! The pump: a thread per session that moves pictures from a [`LiveSource`] to EGFX and pointer
//! changes to the display updates (ADR `HOST-pipeline`).
//!
//! The server side is behind [`GfxTarget`], so this file holds no `ironrdp-server` state. The
//! pump never encodes under backpressure, asks for a keyframe when the server could not queue a
//! frame, and stops when the session ends ([`Pump`] is dropped).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ironrdp_server::DisplayUpdate;

use crate::pipeline::{LiveFrame, LiveSource};
use crate::pointer::{PointerCache, pointer_update};

/// How long the pump waits for a frame before checking for a stop or a keyframe request.
pub const POLL: Duration = Duration::from_millis(20);

/// What the pump sends to.
pub(crate) trait GfxTarget: Send + Sync {
    /// The client has too many frames unacknowledged.
    fn backpressure(&self) -> bool;
    /// Sends one picture: `Some(true)` queued, `Some(false)` refused (backpressure or not ready),
    /// `None` when the session's graphics are gone.
    fn send(&self, frame: &LiveFrame, timestamp_ms: u32) -> Option<bool>;
    /// A pointer change.
    fn pointer(&self, update: DisplayUpdate);
    /// Takes a pending keyframe request (a resize).
    fn take_keyframe_request(&self) -> bool;
}

/// A running pump; dropping it stops the thread and waits for it.
pub(crate) struct Pump {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts pumping `source` to `target`.
pub(crate) fn start(mut source: Box<dyn LiveSource>, target: Arc<dyn GfxTarget>) -> std::io::Result<Pump> {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let thread = std::thread::Builder::new().name("drift-host-pump".into()).spawn(move || {
        let started = Instant::now();
        let mut pointers = PointerCache::default();
        let mut sent: u64 = 0;
        while !stopped.load(Ordering::Relaxed) {
            if target.take_keyframe_request() {
                source.request_keyframe();
            }
            for event in source.pointer() {
                if let Some(update) = pointer_update(&event, &mut pointers) {
                    target.pointer(update);
                }
            }
            let frames = match source.next(POLL, target.backpressure()) {
                Ok(frames) => frames,
                Err(e) => {
                    tracing::warn!(error = %e, "encoding failed");
                    std::thread::sleep(POLL);
                    continue;
                }
            };
            let timestamp = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
            for frame in &frames {
                match target.send(frame, timestamp) {
                    Some(true) => sent += 1,
                    // The picture never reached the client, so later P-frames would reference
                    // nothing: start again from a keyframe.
                    Some(false) => source.request_keyframe(),
                    None => return,
                }
            }
        }
        tracing::debug!(sent, "pump stopped");
    })?;
    Ok(Pump { stop, thread: Some(thread) })
}
