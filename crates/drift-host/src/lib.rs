//! # drift-host
//!
//! Drift Host, the Mac end of Drift-to-Drift sessions (ADR `IOS-0` decisions 1–2; task HOST-2,
//! ADR `HOST-2-host-skeleton`). It is an RDP server built on the vendored `ironrdp-server` with
//! EGFX, so Drift's own client (and any RDP client) can connect.
//!
//! | Module | What | Task |
//! |---|---|---|
//! | [`advert`] | the Bonjour advert and the pairing link / QR code | HOST-8 |
//! | [`config`] | `host.toml`: listen address (loopback by default), account, desktop size | HOST-2 |
//! | [`identity`] | the host's self-signed TLS certificate, created once and reused | HOST-2 |
//! | [`gate`] | one session at a time | HOST-2 |
//! | [`menus`] | the frontmost app's menu bar for the client (Accessibility), polls and presses | EXT-3 |
//! | [`launch`] | start the host from its config directory (the command and the menu-bar app) | HOST-2, HOST-9 |
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
//! | [`clipboard`] | the CLIPRDR server role over `drift-clipboard`'s sync state and the Mac pasteboard | HOST-11 |
//! | [`audio`] | the rdpsnd server: 16-bit PCM from ScreenCaptureKit (or a test tone) | HOST-11 |
//! | [`vdisplay_plan`] | client layouts → virtual-display modes (pure) | HOST-10 |
//! | [`display`] | the display manager: a virtual display for the client, or the physical one | HOST-10 |
//! | `cg_virtual` | the private `CGVirtualDisplay` API, isolated, with a launch self-test | HOST-10 |
//! | [`pairing`] | the pairing code, the paired devices and each connection's authentication | HOST-8 |
//! | [`server`] | the listener, the connection loop, EGFX and Display Control wiring | HOST-2 |
//!
//! Pure modules are unit tested; [`server`] is covered by a loopback test against `drift-rdp`.

pub mod advert;
pub mod audio;
pub mod capture;
#[cfg(target_os = "macos")]
pub mod cg_virtual;
pub mod clipboard;
pub mod codec;
pub mod config;
pub mod display;
pub mod frames;
pub mod gate;
pub mod identity;
pub mod inject;
pub mod input;
pub mod launch;
pub mod layout;
pub mod link;
mod live;
pub mod menus;
pub mod pairing;
pub mod pipeline;
pub mod pointer;
pub mod server;
pub mod setup;
pub mod vdisplay_plan;
