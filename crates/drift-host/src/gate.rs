//! One session at a time (ADR `HOST-2-host-skeleton`): a second client is refused while one is
//! connected, and the running session is never interrupted.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Admits one session at a time.
#[derive(Debug, Clone, Default)]
pub struct SessionGate(Arc<AtomicBool>);

/// Held for the length of a session; dropping it lets the next client in.
#[derive(Debug)]
pub struct SessionTicket(Arc<AtomicBool>);

impl SessionGate {
    /// A free gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Admits a session if none is running.
    pub fn try_begin(&self) -> Option<SessionTicket> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| SessionTicket(Arc::clone(&self.0)))
    }

    /// Whether a session is running.
    pub fn busy(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl Drop for SessionTicket {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
