//! # drift-gfx
//!
//! Drift's own client for the `Microsoft::Windows::RDS::Graphics` dynamic virtual channel
//! (plan §2 decision 2). It reuses IronRDP's PDU codecs (`ironrdp_egfx::pdu`) and ZGFX
//! (`ironrdp_graphics::zgfx`) but owns surface/cache state, codec dispatch and the
//! frame-acknowledgement policy. `ironrdp_egfx::client::GraphicsPipelineClient` and
//! `ironrdp_egfx::decode` are deliberately not used (the latter assumes length-prefixed AVC,
//! plan §1.4).
//!
//! | Item | Role |
//! |---|---|
//! | [`GfxClient`] | the [`ironrdp_dvc::DvcProcessor`]: ZGFX, PDU dispatch, surfaces, caches |
//! | [`advertised_caps`] | exactly `[V8_1{AVC420_ENABLED}, V8{}]` (plan §1.4) |
//! | [`AckOutbox`] | `FrameAcknowledge`s produced by the `presented` callbacks |
//! | [`FrameSink`] | the renderer contract (plan §3), implemented by `drift-render` |
//! | [`GfxError`] | malformed/unsupported server input; maps to `DisconnectReason::ProtocolError` |

mod ack;
mod caps;
mod client;
mod error;
pub mod sink;
mod state;

pub use ack::AckOutbox;
pub use caps::{advertised_caps, caps_advertise_pdu};
pub use client::{CHANNEL_NAME, GfxClient};
pub use error::GfxError;
pub use sink::{FrameSink, PresentedCallback};
