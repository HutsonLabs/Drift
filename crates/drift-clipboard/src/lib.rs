//! # drift-clipboard
//!
//! Clipboard synchronisation between NSPasteboard and CLIPRDR (plan §1.6).
//!
//! | Module | Task |
//! |---|---|
//! | [`contents`] | M0-5: [`ClipboardContents`], the payload exchanged with the session actor |
//! | [`formats`] | M5-1: pure format mapping/conversion (UTF-16/CRLF, PNG/TIFF/DIB, 32 MiB cap) |
//! | [`sync`] | pure CLIPRDR sync state (initial list, echo suppression, newest-wins, focus) used by the M5-2 actor glue |
//! | [`poll`] | M5-3: pure pasteboard polling ([`poll::PasteboardWatcher`], [`poll::PasteboardPort`]) |
//! | `pasteboard` (feature `macos`, macOS only) | M5-3: NSPasteboard adapter |

pub mod contents;
pub mod formats;
#[cfg(all(feature = "macos", target_os = "macos"))]
pub mod pasteboard;
pub mod poll;
pub mod sync;

pub use contents::{ClipboardContents, ClipboardItem};
pub use formats::{ClipError, ClipFormat, FormatKind};
pub use sync::{ClipboardSync, SyncAction, SyncInput};
