//! The pump: a thread per session that moves pictures from a [`LiveSource`] to EGFX and pointer
//! changes to the display updates (ADR `HOST-pipeline`).
//!
//! The server side is behind [`GfxTarget`], so this file holds no `ironrdp-server` state. The
//! pump never encodes under backpressure, asks for a keyframe when the server could not queue a
//! frame, and stops when the session ends ([`Pump`] is dropped). It also runs HOST-7's link
//! hygiene: a heartbeat when the link has been idle, and the backlog level.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ironrdp_server::DisplayUpdate;

use crate::link::{Backlog, BacklogMeter, BacklogSample, HEARTBEAT_INTERVAL, IdleHeartbeat};
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
    /// Frames in flight and bytes waiting in the socket (HOST-7).
    fn sample(&self) -> BacklogSample;
    /// The backlog level changed.
    fn report_backlog(&self, level: Backlog);
    /// The link has been idle: send something tiny and harmless. Returns whether it did.
    fn heartbeat(&self) -> bool;
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
        let mut idle = IdleHeartbeat::new(HEARTBEAT_INTERVAL);
        let mut backlog = BacklogMeter::default();
        while !stopped.load(Ordering::Relaxed) {
            if target.take_keyframe_request() {
                source.request_keyframe();
            }
            let before = backlog.level();
            if backlog.update(target.sample()) != before {
                target.report_backlog(backlog.level());
            }
            for event in source.pointer() {
                if let Some(update) = pointer_update(&event, &mut pointers) {
                    target.pointer(update);
                    idle.sent(Instant::now());
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
                    Some(true) => {
                        sent += 1;
                        idle.sent(Instant::now());
                    }
                    // The picture never reached the client, so later P-frames would reference
                    // nothing: start again from a keyframe.
                    Some(false) => source.request_keyframe(),
                    None => return,
                }
            }
            let now = Instant::now();
            if idle.due(now) && target.heartbeat() {
                idle.sent(now);
            }
        }
        tracing::debug!(sent, "pump stopped");
    })?;
    Ok(Pump { stop, thread: Some(thread) })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Mutex;

    use drift_core::Size;

    use super::*;
    use crate::frames::FrameError;

    #[derive(Default)]
    struct Fake {
        heartbeats: Mutex<u32>,
        levels: Mutex<Vec<Backlog>>,
        sample: Mutex<BacklogSample>,
    }

    impl GfxTarget for Fake {
        fn backpressure(&self) -> bool {
            false
        }
        fn send(&self, _: &LiveFrame, _: u32) -> Option<bool> {
            Some(true)
        }
        fn pointer(&self, _: DisplayUpdate) {}
        fn take_keyframe_request(&self) -> bool {
            false
        }
        fn sample(&self) -> BacklogSample {
            *self.sample.lock().unwrap()
        }
        fn report_backlog(&self, level: Backlog) {
            self.levels.lock().unwrap().push(level);
        }
        fn heartbeat(&self) -> bool {
            *self.heartbeats.lock().unwrap() += 1;
            true
        }
    }

    struct Idle;

    impl LiveSource for Idle {
        fn size(&self) -> Size<u32> {
            Size::new(64, 64)
        }
        fn request_keyframe(&mut self) {}
        fn next(&mut self, timeout: Duration, _: bool) -> Result<Vec<LiveFrame>, FrameError> {
            std::thread::sleep(timeout);
            Ok(Vec::new())
        }
    }

    #[test]
    fn after_a_refused_frame_only_a_keyframe_goes_out_next() {
        let mut gate = KeyframeGate::default();
        assert!(gate.admit(true) && gate.admit(false), "a running stream goes out");
        gate.refused();
        assert!(!gate.admit(false), "a P-frame after the gap would reference nothing");
        assert!(!gate.admit(false));
        assert!(gate.admit(true), "the keyframe restarts the stream");
        gate.sent(true);
        assert!(gate.admit(false));
    }

    /// Frames K P P, then P P K P; the target refuses the second frame it is offered.
    struct Script(Vec<Vec<bool>>);

    impl LiveSource for Script {
        fn size(&self) -> Size<u32> {
            Size::new(64, 64)
        }
        fn request_keyframe(&mut self) {}
        fn next(&mut self, timeout: Duration, _: bool) -> Result<Vec<LiveFrame>, FrameError> {
            if self.0.is_empty() {
                std::thread::sleep(timeout);
                return Ok(Vec::new());
            }
            Ok(self
                .0
                .remove(0)
                .into_iter()
                .map(|keyframe| LiveFrame {
                    au: vec![u8::from(keyframe)],
                    regions: Vec::new(),
                    keyframe,
                    chroma: None,
                    hevc: true,
                })
                .collect())
        }
    }

    #[derive(Default)]
    struct Refusing {
        offered: Mutex<Vec<bool>>,
    }

    impl GfxTarget for Refusing {
        fn backpressure(&self) -> bool {
            false
        }
        fn send(&self, frame: &LiveFrame, _: u32) -> Option<bool> {
            let mut offered = self.offered.lock().unwrap();
            offered.push(frame.keyframe);
            Some(offered.len() != 2)
        }
        fn pointer(&self, _: DisplayUpdate) {}
        fn take_keyframe_request(&self) -> bool {
            false
        }
        fn sample(&self) -> BacklogSample {
            BacklogSample::default()
        }
        fn report_backlog(&self, _: Backlog) {}
        fn heartbeat(&self) -> bool {
            true
        }
    }

    #[test]
    fn the_pump_skips_p_frames_after_a_refusal_until_a_keyframe() {
        let target = Arc::new(Refusing::default());
        let source = Script(vec![vec![true, false, false], vec![false, false, true, false]]);
        let pump = start(Box::new(source), Arc::clone(&target) as Arc<dyn GfxTarget>).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        drop(pump);
        // K sent, P refused, P P P skipped, K sent, P sent.
        assert_eq!(target.offered.lock().unwrap().as_slice(), &[true, false, true, false]);
    }

    #[test]
    fn an_idle_link_gets_heartbeats_and_a_slow_one_reports_its_backlog() {
        let fake = Arc::new(Fake::default());
        *fake.sample.lock().unwrap() = BacklogSample { frames_in_flight: 9, unsent_bytes: 0 };
        let pump = start(Box::new(Idle), Arc::clone(&fake) as Arc<dyn GfxTarget>).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        drop(pump);
        let beats = *fake.heartbeats.lock().unwrap();
        assert!((2..=7).contains(&beats), "about one per 100 ms of idle link, got {beats}");
        assert_eq!(fake.levels.lock().unwrap().as_slice(), &[Backlog::Backlogged]);
    }
}
