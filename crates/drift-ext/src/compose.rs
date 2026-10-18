//! Several tasks on one channel end (EXT-5): pairing (HOST-8), codec negotiation (EXT-5) and
//! whatever comes next each bring a [`Responder`]; [`Responders`] lets every one of them speak
//! once the Hellos agree and hands each later message to the first one that answers it.

use crate::Negotiated;
use crate::channel::Responder;
use crate::wire::Message;

/// Responders in order.
#[derive(Debug, Default)]
pub struct Responders(Vec<Box<dyn Responder>>);

impl Responders {
    /// None yet.
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Adds `responder` after the others.
    #[must_use]
    pub fn with(mut self, responder: Box<dyn Responder>) -> Self {
        self.0.push(responder);
        self
    }

    /// How many responders there are.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Responder for Responders {
    fn on_negotiated(&mut self, negotiated: &Negotiated) -> Vec<Message> {
        self.0.iter_mut().flat_map(|r| r.on_negotiated(negotiated)).collect()
    }

    fn on_message(&mut self, message: &Message) -> Option<Vec<Message>> {
        self.0.iter_mut().find_map(|r| r.on_message(message))
    }
}
