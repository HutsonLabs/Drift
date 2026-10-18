//! # drift-rdp
//!
//! The per-tab RDP session. The **public interface** consumed by `drift-app` is fixed in
//! [`session`] (M0-5, `docs/adr/M0-5-session-interface.md`) so the actor (stream A) and
//! the app (stream D) can be built in parallel.
//!
//! | Module | Task |
//! |---|---|
//! | [`session`] | M0-5 interface; the actor (`actor.rs`) from M1-1 onwards |
//! | [`connect`] | M1-1: TCP, TLS with TOFU pin, NLA, capabilities |
//! | [`redirect`] | M3-1: Server Redirection loop (coverage-gated) |
//! | [`rdstls`] | M3-1: RDSTLS client glue (coverage-gated) |
//! | [`resize`] | M4-2: Display Control resize driver (debounce, layout change, Fit for Sharing) |
//! | `clipboard` | M5-2: CLIPRDR backend + `drift_clipboard::ClipboardSync` glue |
//! | [`greeter`] | M3-2/M7-3: opt-in Linux password typing at the GDM greeter |
//! | [`logging`] | M9-3: the tracing targets that must never log below INFO |
//! | [`pointer`] | M2-5: pointer outputs → `SessionEvent::Cursor` |
//! | [`stats`] | fps / bit rate sampling for `SessionEvent::Stats` |

mod actor;
pub mod audio;
mod clipboard;
pub mod connect;
mod fastpath;
mod gfx_ack;
mod graphics;
pub mod greeter;
pub mod logging;
pub mod pointer;
pub mod rdstls;
pub mod redirect;
pub mod resize;
pub mod session;
pub mod stats;
mod tls;

pub use session::{
    CertificateRole, CursorBitmap, CursorUpdate, ScaleMode, SessionCapabilities, SessionClosed,
    SessionCommand, SessionEvent, SessionEvents, SessionHandle, SessionOptions, SessionSecrets, SessionStats,
    spawn_session,
};
