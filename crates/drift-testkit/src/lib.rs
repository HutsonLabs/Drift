//! # drift-testkit
//!
//! Shared test support (dev-dependency only):
//!
//! - [`ManualClock`]: a [`drift_core::Clock`] that only moves when told to.
//! - [`RecordingFrameSink`]: a [`drift_gfx::FrameSink`] that records every call for
//!   assertions and `insta` snapshots, with controllable `presented` callbacks.
//! - [`golden`]: PNG goldens with a per-channel tolerance (M1-5).
//! - [`fixtures`]: the M0-3 fixture loader.
//! - [`fake_server`]: [`FakeServer`], a scripted loopback RDP server (NLA, RDSTLS, per-leg
//!   certificates, Server Redirection injection), M1-1/M3-1.
//! - [`e2e`]: the scripted input [`e2e::Script`] used by the real-host tests (GDM login).

pub mod clock;
pub mod e2e;
pub mod fake_channels;
pub mod fake_server;
pub mod fixtures;
pub mod frame_sink;
pub mod golden;

pub use clock::ManualClock;
pub use fake_channels::{Channels, RecordedLayout, ServerClipFormat};
pub use fake_server::{
    FakeServer, FakeServerLog, LegAuth, LegRecord, LegScript, RdstlsRequest, ServerAction, TestCert,
    redirection_pdu,
};
pub use frame_sink::{FrameLog, FrameSinkCall, PresentMode, RecordingFrameSink};
