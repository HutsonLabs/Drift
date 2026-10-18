//! # drift-transport
//!
//! Transports for sessions between a Drift client and a Drift Host (task HOST-12; ADR
//! `IOS-0` decision 6 and ADR `HOST-12-quic-transport`).
//!
//! | Module | What |
//! |---|---|
//! | [`plan`] | the transport order: QUIC first with a paired Drift Host, TCP otherwise; fallback with a timeout |
//! | [`quic`] | RDP over QUIC: the RDP byte stream on one reliable stream, one unidirectional stream per GFX frame, the host certificate pinned |
//! | [`stream`] | one byte-stream type for TCP and QUIC, which the RDP client and server run over |
//! | [`lossy`] | a deterministic lossy-link model and a loopback UDP proxy for measurements and tests |
//!
//! GNOME hosts and third-party RDP clients never see any of this: they keep TLS over TCP.

pub mod lossy;
pub mod plan;
pub mod quic;
pub mod stream;
