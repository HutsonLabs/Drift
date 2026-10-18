//! Pointer lock (IOS-6). Stub (Red).

use objc2::MainThreadMarker;
use objc2::runtime::AnyObject;

/// Locks or unlocks the pointer for one view controller.
pub fn set_locked(_mtm: MainThreadMarker, _vc: &AnyObject, _locked: bool) {}
