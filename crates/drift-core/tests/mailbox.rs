//! HOST-5 Red: the one-slot newest-frame mailbox between capture → encode and encode → socket.
//! It always keeps the newest item and drops older ones instead of queueing (ADR IOS-0
//! decision 4; the idea is from Sill, the code is ours).
#![allow(missing_docs, clippy::unwrap_used)]

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use drift_core::mailbox::{Closed, MailboxStats, TakeError, mailbox};
use proptest::prelude::*;

#[test]
fn keeps_only_the_newest_item_and_counts_drops() {
    let (tx, rx) = mailbox();
    assert_eq!(tx.post(1), Ok(None));
    assert_eq!(tx.post(2), Ok(Some(1)), "the unread item comes back for reuse");
    assert_eq!(tx.post(3), Ok(Some(2)));
    assert_eq!(rx.try_take(), Some(3));
    assert_eq!(rx.try_take(), None);
    assert_eq!(rx.stats(), MailboxStats { posted: 3, taken: 1, dropped: 2 });
    assert_eq!(tx.stats(), rx.stats());
}

#[test]
fn take_timeout_waits_for_a_post_then_times_out() {
    let (tx, rx) = mailbox::<u32>();
    assert_eq!(rx.take_timeout(Duration::from_millis(20)), Err(TakeError::Timeout));
    let t = thread::spawn(move || {
        thread::sleep(Duration::from_millis(30));
        tx.post(7).unwrap();
        tx
    });
    assert_eq!(rx.take_timeout(Duration::from_secs(5)), Ok(7));
    drop(t.join().unwrap());
}

#[test]
fn closing_either_side_ends_the_other() {
    let (tx, rx) = mailbox::<u32>();
    tx.post(1).unwrap();
    drop(tx);
    assert_eq!(rx.take(), Some(1), "an item posted before closing is still delivered");
    assert_eq!(rx.take(), None, "then the mailbox reports closed");
    assert_eq!(rx.take_timeout(Duration::from_millis(1)), Err(TakeError::Closed));

    let (tx, rx) = mailbox::<u32>();
    drop(rx);
    assert_eq!(tx.post(5), Err(Closed(5)), "posting to a closed mailbox hands the item back");
    assert!(tx.is_closed());
}

#[test]
fn a_blocked_taker_wakes_when_the_poster_goes_away() {
    let (tx, rx) = mailbox::<u32>();
    let t = thread::spawn(move || rx.take());
    thread::sleep(Duration::from_millis(20));
    drop(tx);
    assert_eq!(t.join().unwrap(), None);
}

struct Flag(std::sync::atomic::AtomicBool);
impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn async_recv_is_woken_by_a_post() {
    let (tx, rx) = mailbox::<u32>();
    let flag = Arc::new(Flag(false.into()));
    let waker = Waker::from(flag.clone());
    let mut cx = Context::from_waker(&waker);
    let mut fut = pin!(rx.recv());
    assert_eq!(fut.as_mut().poll(&mut cx), Poll::Pending);
    tx.post(9).unwrap();
    assert!(flag.0.load(std::sync::atomic::Ordering::SeqCst), "post wakes the pending recv");
    assert_eq!(fut.as_mut().poll(&mut cx), Poll::Ready(Some(9)));
    drop(tx);
    let mut fut = pin!(rx.recv());
    assert_eq!(fut.as_mut().poll(&mut cx), Poll::Ready(None));
}

#[test]
fn a_slow_consumer_sees_increasing_items_and_always_the_last() {
    let (tx, rx) = mailbox::<u32>();
    const N: u32 = 100_000;
    let producer = thread::spawn(move || {
        for i in 0..N {
            tx.post(i).unwrap();
        }
    });
    let mut seen = Vec::new();
    while let Some(v) = rx.take() {
        seen.push(v);
        if seen.len() % 64 == 0 {
            thread::sleep(Duration::from_micros(50));
        }
    }
    producer.join().unwrap();
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "never an older item after a newer one");
    assert_eq!(seen.last(), Some(&(N - 1)), "the newest item is never lost");
    let stats = rx.stats();
    assert_eq!(stats.posted, u64::from(N));
    assert_eq!(stats.taken + stats.dropped, stats.posted);
    assert_eq!(stats.taken, seen.len() as u64);
}

#[derive(Debug, Clone)]
enum Op {
    Post(u8),
    Take,
}

proptest! {
    #[test]
    fn behaves_like_a_single_slot(ops in prop::collection::vec(prop_oneof![any::<u8>().prop_map(Op::Post), Just(Op::Take)], 0..64)) {
        let (tx, rx) = mailbox();
        let mut slot: Option<u8> = None;
        let mut model = MailboxStats::default();
        for op in ops {
            match op {
                Op::Post(v) => {
                    let replaced = tx.post(v).unwrap();
                    prop_assert_eq!(replaced, slot.replace(v));
                    model.posted += 1;
                    if replaced.is_some() { model.dropped += 1; }
                }
                Op::Take => {
                    let got = rx.try_take();
                    prop_assert_eq!(got, slot.take());
                    if got.is_some() { model.taken += 1; }
                }
            }
            prop_assert_eq!(rx.stats(), model);
        }
    }
}

#[test]
fn post_merging_folds_the_dropped_item_into_the_new_one() {
    // A replaced frame's damage must not be lost: the new frame takes the union.
    let (tx, rx) = mailbox::<Vec<u8>>();
    tx.post_merging(vec![1], |new, old| new.extend(old)).unwrap();
    tx.post_merging(vec![2], |new, old| new.extend(old)).unwrap();
    tx.post_merging(vec![3], |new, old| new.extend(old)).unwrap();
    assert_eq!(rx.try_take(), Some(vec![3, 2, 1]));
    assert_eq!(rx.stats(), MailboxStats { posted: 3, taken: 1, dropped: 2 });
    drop(rx);
    assert_eq!(tx.post_merging(vec![4], |_, _| unreachable!()), Err(Closed(vec![4])));
}
