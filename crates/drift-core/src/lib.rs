//! # drift-core
//!
//! Pure (no I/O, no FFI) model shared by every Drift crate. This crate holds the
//! **core contracts** fixed by plan §3 (task M0-5):
//!
//! - [`profile`]: [`ConnectMode`], [`ConnectionProfile`], per-profile preferences and
//!   [`CertFingerprint`] (formatted exactly like `grdctl status`).
//! - [`state`]: the [`SessionState`] finite-state machine and [`DisconnectReason`].
//! - [`input`]: the wire-level [`InputEvent`] produced by `drift-input` / `drift-macos`
//!   and consumed by the session actor.
//! - [`geometry`]: [`Size`], [`Point`], [`Rect`], [`Bgra`] and [`ViewGeometry`].
//! - [`video`]: the opaque [`Nv12Frame`] handle passed from `drift-video` through
//!   `drift-gfx` to `drift-render` (see `docs/adr/M0-5-nv12-frame-location.md`).
//! - [`clock`]: the [`Clock`] abstraction (tests use `drift_testkit::ManualClock`).
//!
//! - [`layout`]: `desired_layout`, the Display Control monitor layout policy (M4-1).
//! - [`reconnect`]: [`ReconnectPolicy`], exponential full-jitter backoff (M7-1).
//! - [`triggers`]: [`TriggerMerger`], network/wake reconnect triggers (M7-2).
//! - [`store`]: [`ProfileStore`], the `profiles.toml` document (M1-6).
//! - [`messages`]: user-facing disconnect explanations with next steps (M1-6, M9-4).
//! - [`mailbox`]: the one-slot newest-item mailbox between Drift Host's pipeline stages (HOST-5).

pub mod audio;
pub mod clock;
pub mod geometry;
pub mod input;
pub mod layout;
pub mod mailbox;
pub mod messages;
pub mod pairing;
pub mod profile;
pub mod reconnect;
pub mod state;
pub mod store;
pub mod triggers;
pub mod video;

pub use clock::{Clock, SystemClock};
pub use geometry::{Bgra, DesktopSize, Point, Rect, Size, ViewGeometry};
pub use input::{InputEvent, MouseButton};
pub use layout::{DisplayControlCaps, MonitorLayout, desired_layout};
pub use messages::{ErrorAction, ErrorExplanation, explain_disconnect};
pub use profile::{
    CertFingerprint, ClipboardPrefs, CmdAs, ConnectMode, ConnectionProfile, DisplayPrefs, KeyboardPrefs,
    ProfileError, ProfileField, ProfileIssue, ProfileProblem, SecretRole, TouchMode,
};
pub use reconnect::{ReconnectConfig, ReconnectDecision, ReconnectPolicy};
pub use state::{ConnectStage, DisconnectReason, InvalidTransition, SessionState};
pub use store::{ProfileStore, StoreError};
pub use triggers::{Trigger, TriggerAction, TriggerMerger};
pub use video::{BgraPicture, Nv12Frame, Nv12Planes, Nv12Source};
