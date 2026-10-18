//! IOS-9: the pairing QR scanner reports "no camera" at once where there is none (the
//! simulator), without presenting anything. Runs in the iOS simulator.
#![cfg(target_os = "ios")]
#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::mpsc;

use drift_ios::qr::{ScanError, scan};
use objc2::MainThreadMarker;

#[test]
fn without_a_camera_the_scanner_says_so() {
    // libtest runs tests off the main thread; the camera check fails before any UIKit call, but
    // `scan` needs a marker, so take it unchecked only for this query-only path.
    // SAFETY: with no camera, `scan` returns before touching UIKit or `presenter`.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let (tx, rx) = mpsc::channel();
    // SAFETY: a null presenter is never dereferenced on the no-camera path.
    unsafe { scan(mtm, std::ptr::null_mut(), move |r| tx.send(r).unwrap()) };
    assert_eq!(rx.recv().unwrap(), Err(ScanError::NoCamera));
}
