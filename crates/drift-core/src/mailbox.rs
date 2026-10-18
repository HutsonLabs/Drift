//! A one-slot newest-item mailbox (task **HOST-5**; ADR `IOS-0` decision 4, `HOST-5-frame-mailbox`).
//!
//! Drift Host passes frames capture → encode → socket. A slow stage must never build a queue,
//! which would only add latency: the mailbox holds at most one item, a new post replaces an
//! unread one (handing it back so its buffer can be reused), and the taker always gets the
//! newest. Every replaced item is counted as dropped. The idea comes from Sill; the code is
//! Drift's own.
//!
//! One poster and one taker. Dropping either side closes the mailbox: the taker still gets an
//! item posted before the close, then `None`; the poster gets its item back as [`Closed`].

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

/// Counters since the mailbox was created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MailboxStats {
    /// Items posted.
    pub posted: u64,
    /// Items taken.
    pub taken: u64,
    /// Items replaced before anyone took them.
    pub dropped: u64,
}

/// The mailbox was closed; the item comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the mailbox is closed")]
pub struct Closed<T>(pub T);

/// Why [`Taker::take_timeout`] returned without an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TakeError {
    /// Nothing arrived in time.
    #[error("nothing was posted in time")]
    Timeout,
    /// The poster is gone and the slot is empty.
    #[error("the mailbox is closed")]
    Closed,
}

#[derive(Debug)]
struct State<T> {
    slot: Option<T>,
    closed: bool,
    stats: MailboxStats,
    waker: Option<Waker>,
}

#[derive(Debug)]
struct Shared<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        // A panic while holding the lock cannot leave the slot inconsistent (every update is a
        // single assignment), so a poisoned lock is still usable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn close(&self) {
        let waker = {
            let mut s = self.lock();
            s.closed = true;
            s.waker.take()
        };
        self.ready.notify_all();
        if let Some(w) = waker {
            w.wake();
        }
    }
}

/// Creates a mailbox and returns its two ends.
pub fn mailbox<T>() -> (Poster<T>, Taker<T>) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State { slot: None, closed: false, stats: MailboxStats::default(), waker: None }),
        ready: Condvar::new(),
    });
    (Poster { shared: Arc::clone(&shared) }, Taker { shared })
}

/// The producing end.
#[derive(Debug)]
pub struct Poster<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Poster<T> {
    /// Puts `item` in the slot. Returns the unread item it replaced (counted as dropped), or
    /// `Err(Closed(item))` if the taker is gone. Never blocks on the consumer.
    pub fn post(&self, item: T) -> Result<Option<T>, Closed<T>> {
        let (replaced, waker) = {
            let mut s = self.shared.lock();
            if s.closed {
                return Err(Closed(item));
            }
            s.stats.posted += 1;
            let replaced = s.slot.replace(item);
            if replaced.is_some() {
                s.stats.dropped += 1;
            }
            (replaced, s.waker.take())
        };
        self.shared.ready.notify_one();
        if let Some(w) = waker {
            w.wake();
        }
        Ok(replaced)
    }

    /// Like [`Self::post`], but an unread item is first folded into `item` with
    /// `merge(&mut item, unread)` (still counted as dropped): a replaced frame's damage carries
    /// over to the newer frame instead of being lost.
    pub fn post_merging(&self, mut item: T, merge: impl FnOnce(&mut T, T)) -> Result<(), Closed<T>> {
        let waker = {
            let mut s = self.shared.lock();
            if s.closed {
                return Err(Closed(item));
            }
            s.stats.posted += 1;
            if let Some(unread) = s.slot.take() {
                s.stats.dropped += 1;
                merge(&mut item, unread);
            }
            s.slot = Some(item);
            s.waker.take()
        };
        self.shared.ready.notify_one();
        if let Some(w) = waker {
            w.wake();
        }
        Ok(())
    }

    /// Whether the taker is gone.
    pub fn is_closed(&self) -> bool {
        self.shared.lock().closed
    }

    /// The counters.
    pub fn stats(&self) -> MailboxStats {
        self.shared.lock().stats
    }
}

impl<T> Drop for Poster<T> {
    fn drop(&mut self) {
        self.shared.close();
    }
}

/// The consuming end.
#[derive(Debug)]
pub struct Taker<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Taker<T> {
    fn take_locked(s: &mut State<T>) -> Option<T> {
        let item = s.slot.take();
        if item.is_some() {
            s.stats.taken += 1;
        }
        item
    }

    /// The newest item, if one is waiting.
    pub fn try_take(&self) -> Option<T> {
        Self::take_locked(&mut self.shared.lock())
    }

    /// Waits for an item; `None` once the poster is gone and the slot is empty.
    pub fn take(&self) -> Option<T> {
        let mut s = self.shared.lock();
        loop {
            if let Some(item) = Self::take_locked(&mut s) {
                return Some(item);
            }
            if s.closed {
                return None;
            }
            s = self.shared.ready.wait(s).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Waits up to `timeout` for an item.
    pub fn take_timeout(&self, timeout: Duration) -> Result<T, TakeError> {
        let deadline = Instant::now() + timeout;
        let mut s = self.shared.lock();
        loop {
            if let Some(item) = Self::take_locked(&mut s) {
                return Ok(item);
            }
            if s.closed {
                return Err(TakeError::Closed);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(TakeError::Timeout);
            }
            s = self.shared.ready.wait_timeout(s, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// Waits asynchronously for an item; `None` once the poster is gone and the slot is empty.
    /// Works with any executor (the poster wakes the stored waker).
    pub fn recv(&self) -> Recv<'_, T> {
        Recv { taker: self }
    }

    /// The counters.
    pub fn stats(&self) -> MailboxStats {
        self.shared.lock().stats
    }
}

impl<T> Drop for Taker<T> {
    fn drop(&mut self) {
        self.shared.close();
    }
}

/// The future returned by [`Taker::recv`].
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct Recv<'a, T> {
    taker: &'a Taker<T>,
}

impl<T> Future for Recv<'_, T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let mut s = self.taker.shared.lock();
        if let Some(item) = Taker::take_locked(&mut s) {
            return Poll::Ready(Some(item));
        }
        if s.closed {
            return Poll::Ready(None);
        }
        s.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}
