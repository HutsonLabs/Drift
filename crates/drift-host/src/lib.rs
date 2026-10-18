//! # drift-host
//!
//! Drift Host, the Mac end of Drift-to-Drift sessions (ADR `IOS-0` decisions 1–2; task HOST-2,
//! ADR `HOST-2-host-skeleton`). It is an RDP server built on the vendored `ironrdp-server` with
//! EGFX, so Drift's own client (and any RDP client) can connect.
//!
//! | Module | What | Task |
//! |---|---|---|
//! | [`config`] | `host.toml`: listen address (loopback by default), account, desktop size | HOST-2 |
//! | [`identity`] | the host's self-signed TLS certificate, created once and reused | HOST-2 |
//! | [`gate`] | one session at a time | HOST-2 |
//! | [`layout`] | client monitor layouts → desktop sizes the encoder accepts | HOST-2 |
//! | [`input`] | input sinks: count/log only for now; HOST-6's injection plugs in here | HOST-2 |
//! | [`frames`] | the test pattern, shown when the screen cannot be captured | HOST-2 |
//! | [`pipeline`] | the live picture: raw frames → real-time encoder → AVC420 frames with regions | HOST-pipeline |
//! | [`capture`] | the screen (ScreenCaptureKit) as raw frames, and the pointer beside it | HOST-pipeline |
//! | [`pointer`] | pointer changes → RDP pointer updates, shapes cached | HOST-pipeline |
//! | [`link`] | link hygiene: video service class, idle heartbeat, backlog level | HOST-7 |
//! | [`setup`] | the binary's wiring: screen or pattern, injection or logging | HOST-pipeline |
//! | [`inject`] | client input → `drift-inject` plans → CoreGraphics events | HOST-pipeline |
//! | [`codec`] | AVC444 or AVC420 for an output size and what the client confirmed | AVC444-client |
//! | [`server`] | the listener, the connection loop, EGFX and Display Control wiring | HOST-2 |
//!
//! Pure modules are unit tested; [`server`] is covered by a loopback test against `drift-rdp`.

pub mod capture;
pub mod codec;
pub mod config;
pub mod frames;
pub mod gate;
pub mod identity;
pub mod inject;
pub mod input;
pub mod layout;
pub mod link;
mod live;
pub mod pipeline;
pub mod pointer;
pub mod server;
pub mod setup;
