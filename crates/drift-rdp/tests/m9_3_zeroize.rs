//! M9-3 Red: the one-time redirect credentials are zeroized, proven at the allocator.
//!
//! Plan §1.3: the Server Redirection PDU carries single-use credentials that must never be
//! persisted, and plan M9-3 requires them to be zeroized. `Zeroizing` fields make that true by
//! construction — but only for the copies someone remembered to wrap. A `Debug` assertion
//! cannot see the heap, so this test watches the **allocator**: while it is armed, every block
//! handed back to the system is scanned for a canary before it is freed.
//!
//! That is why this lives in its own test binary: a `#[global_allocator]` is process-wide.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use drift_core::ConnectMode;
use drift_rdp::rdstls::OneTimeCredentials;
use drift_rdp::redirect::RedirectLoop;
use ironrdp_pdu::rdp::server_redirection::{ServerRedirectionFlags, ServerRedirectionPdu};

/// A pattern that cannot plausibly occur in unrelated memory.
const CANARY: &[u8] = b"DRIFT-M9-3-ZEROIZE-CANARY-7f3a9c1e5b2d";

/// Scanning is off by default: arming it costs a memory scan per free.
static ARMED: AtomicBool = AtomicBool::new(false);
/// Freed blocks that still contained [`CANARY`].
static LEAKED: AtomicUsize = AtomicUsize::new(0);
/// Freed blocks inspected while armed (a sanity check that the hook runs at all).
static SCANNED: AtomicUsize = AtomicUsize::new(0);

/// A `System` allocator that inspects freed blocks for [`CANARY`].
struct CanaryAllocator;

// SAFETY: every method forwards to `System` with the same pointer and layout it was given.
// `dealloc` only reads `size` bytes of the block being freed — the block is still valid and
// owned by the caller at that point — and the read happens before it is handed to `System`.
unsafe impl GlobalAlloc for CanaryAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.load(Ordering::Relaxed) && layout.size() >= CANARY.len() {
            // SAFETY: the block is `layout.size()` readable bytes until `System::dealloc`.
            let bytes = unsafe { std::slice::from_raw_parts(ptr, layout.size()) };
            SCANNED.fetch_add(1, Ordering::Relaxed);
            if bytes.windows(CANARY.len()).any(|w| w == CANARY) {
                LEAKED.fetch_add(1, Ordering::Relaxed);
            }
        }
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CanaryAllocator = CanaryAllocator;

/// One armed window at a time: the counters and the flag are process-wide, and `cargo test`
/// runs the tests of one binary on several threads (`cargo nextest` gives each its own
/// process, where this lock is free).
static WINDOW: Mutex<()> = Mutex::new(());

/// Runs `body` with the allocator scanner armed; returns `(leaked blocks, scanned blocks)`.
fn watching<T>(body: impl FnOnce() -> T) -> (usize, usize) {
    let _window = WINDOW.lock().unwrap_or_else(PoisonError::into_inner);
    LEAKED.store(0, Ordering::Relaxed);
    SCANNED.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    drop(body());
    ARMED.store(false, Ordering::Relaxed);
    (LEAKED.load(Ordering::Relaxed), SCANNED.load(Ordering::Relaxed))
}

/// A redirection PDU whose one-time fields are all made of the canary.
fn canary_pdu() -> ServerRedirectionPdu {
    let text = String::from_utf8(CANARY.to_vec()).expect("ASCII canary");
    ServerRedirectionPdu {
        session_id: 7,
        redirection_flags: ServerRedirectionFlags::LOAD_BALANCE_INFO
            | ServerRedirectionFlags::USERNAME
            | ServerRedirectionFlags::PASSWORD
            | ServerRedirectionFlags::PASSWORD_IS_PK_ENCRYPTED
            | ServerRedirectionFlags::REDIRECTION_GUID,
        load_balance_info: Some(b"Cookie: msts=424242\r\n".to_vec()),
        username: Some(text),
        password: Some(CANARY.to_vec()),
        redirection_guid: Some(CANARY.to_vec()),
        ..Default::default()
    }
}

#[test]
fn the_detector_sees_a_secret_that_nobody_zeroizes() {
    // Positive control: without `Zeroizing`, the canary is still in the block when it is freed.
    let (leaked, scanned) = watching(|| {
        let plain = String::from_utf8(CANARY.to_vec()).expect("ASCII canary");
        std::hint::black_box(plain.len());
    });
    assert!(scanned > 0, "the allocator hook must run while armed");
    assert!(leaked > 0, "the detector itself is broken: an unzeroized secret went unnoticed");
}

#[test]
fn one_time_credentials_are_gone_from_the_heap_after_use() {
    let (leaked, scanned) = watching(|| {
        let mut pdu = canary_pdu();
        let next = RedirectLoop::new(ConnectMode::RemoteLogin, "10.0.2.40", 3389)
            .on_redirect(&mut pdu)
            .expect("the redirect is followed");
        // The PDU's own copies are cleared as soon as the credentials are extracted.
        assert_eq!(pdu.username, None);
        assert_eq!(pdu.password, None);
        assert_eq!(pdu.redirection_guid, None);
        // … and the connector's copy zeroizes when the leg is done with it.
        let connector = next.credentials.into_connector();
        assert_eq!(connector.password, CANARY);
        drop(connector);
        drop(next.target_certificate);
    });
    assert!(scanned > 0, "the allocator hook must run while armed");
    assert_eq!(leaked, 0, "{leaked} freed block(s) still held the one-time credentials");
}

#[test]
fn credentials_taken_from_a_pdu_and_dropped_unused_are_zeroized_too() {
    let (leaked, scanned) = watching(|| {
        let mut pdu = canary_pdu();
        let credentials = OneTimeCredentials::from_redirection(&pdu).expect("complete PDU");
        // A failed leg drops the credentials without ever handing them to the connector.
        drop(credentials);
        // The PDU is zeroized by its owner (the actor does this through `on_redirect`).
        use zeroize::Zeroize as _;
        pdu.username.zeroize();
        pdu.password.zeroize();
        pdu.redirection_guid.zeroize();
    });
    assert!(scanned > 0);
    assert_eq!(leaked, 0, "{leaked} freed block(s) still held the one-time credentials");
}
