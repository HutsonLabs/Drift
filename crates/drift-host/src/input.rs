//! Where the client's keyboard and mouse go.
//!
//! The skeleton only counts and logs input; HOST-6's `drift-inject` becomes another
//! [`InputSink`] behind `HostConfig::inject_input`. Logs name the kind of event, never the key or
//! character: a client typing a password must not leave it in the host's log.

use std::sync::{Arc, Mutex, PoisonError};

use ironrdp_server::{KeyboardEvent, MouseEvent, RdpServerInputHandler};

/// Receives the client's input events.
pub trait InputSink: Send {
    /// A keyboard event (scancode, Unicode or lock-key synchronisation).
    fn keyboard(&mut self, event: KeyboardEvent);
    /// A mouse event (move, button, wheel).
    fn mouse(&mut self, event: MouseEvent);
}

/// What a [`RecordingInput`] has seen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputCounts {
    /// Keyboard events.
    pub keys: u64,
    /// Mouse events.
    pub mouse: u64,
}

/// Counts events (shared between clones), logging their kind at `debug`. The skeleton's sink.
#[derive(Debug, Clone, Default)]
pub struct RecordingInput(Arc<Mutex<InputCounts>>);

impl RecordingInput {
    /// Keyboard events so far.
    pub fn key_events(&self) -> u64 {
        self.counts().keys
    }

    /// Mouse events so far.
    pub fn mouse_events(&self) -> u64 {
        self.counts().mouse
    }

    /// Both counters.
    pub fn counts(&self) -> InputCounts {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl InputSink for RecordingInput {
    fn keyboard(&mut self, event: KeyboardEvent) {
        let kind = match event {
            KeyboardEvent::Pressed { .. } | KeyboardEvent::UnicodePressed(_) => "key down",
            KeyboardEvent::Released { .. } | KeyboardEvent::UnicodeReleased(_) => "key up",
            KeyboardEvent::Synchronize(_) => "lock-key sync",
        };
        tracing::debug!(kind, "input");
        self.0.lock().unwrap_or_else(PoisonError::into_inner).keys += 1;
    }

    fn mouse(&mut self, event: MouseEvent) {
        tracing::trace!(?event, "input");
        self.0.lock().unwrap_or_else(PoisonError::into_inner).mouse += 1;
    }
}

/// Adapts an [`InputSink`] to `ironrdp-server`'s input handler.
pub(crate) struct SinkHandler(pub(crate) Box<dyn InputSink>);

impl RdpServerInputHandler for SinkHandler {
    fn keyboard(&mut self, event: KeyboardEvent) {
        self.0.keyboard(event);
    }

    fn mouse(&mut self, event: MouseEvent) {
        self.0.mouse(event);
    }
}
