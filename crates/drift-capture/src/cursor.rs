//! The pointer, reported separately from the picture (`showsCursor` is off), and only when it
//! changes: RDP sends pointer position and shape as their own updates.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use drift_core::{Point, Size};

/// A pointer image: straight (not premultiplied) RGBA, top row first.
#[derive(Debug, Clone)]
pub struct CursorShape {
    size: Size<u32>,
    hotspot: Point<u32>,
    rgba: Arc<[u8]>,
    key: u64,
}

impl CursorShape {
    /// Checks that `rgba` holds `size` pixels and the hotspot lies inside; `None` otherwise.
    pub fn new(size: Size<u32>, hotspot: Point<u32>, rgba: Vec<u8>) -> Option<Self> {
        let expected = usize::try_from(u64::from(size.width) * u64::from(size.height) * 4).ok()?;
        if size.width == 0 || size.height == 0 || rgba.len() != expected {
            return None;
        }
        if hotspot.x >= size.width || hotspot.y >= size.height {
            return None;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (size.width, size.height, hotspot.x, hotspot.y).hash(&mut h);
        rgba.hash(&mut h);
        Some(Self { size, hotspot, rgba: rgba.into(), key: h.finish() })
    }

    /// Size in pixels.
    pub const fn size(&self) -> Size<u32> {
        self.size
    }

    /// Hotspot in pixels from the top-left corner.
    pub const fn hotspot(&self) -> Point<u32> {
        self.hotspot
    }

    /// The pixels.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// A content hash (equal shapes have equal keys; used to cache shapes on the wire).
    pub const fn key(&self) -> u64 {
        self.key
    }
}

impl PartialEq for CursorShape {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.size == other.size
            && self.hotspot == other.hotspot
            && self.rgba == other.rgba
    }
}

impl Eq for CursorShape {}

/// One poll of the pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorSample {
    /// Position in the captured display's pixels; `None` when it is on another display.
    pub position: Option<Point<u32>>,
    /// The current shape, if it could be read.
    pub shape: Option<CursorShape>,
}

/// What changed since the previous sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorEvent {
    /// A new shape (sent before the move it accompanies).
    Shape(CursorShape),
    /// The pointer moved to this position.
    Moved(Point<u32>),
    /// The pointer left the captured display.
    Hidden,
}

/// Turns samples into change events.
#[derive(Debug, Default)]
pub struct CursorTracker {
    position: Option<Point<u32>>,
    shape: Option<u64>,
    hidden: bool,
}

impl CursorTracker {
    /// The events between the previous sample and `sample`.
    pub fn update(&mut self, sample: &CursorSample) -> Vec<CursorEvent> {
        let mut events = Vec::new();
        if let Some(shape) = &sample.shape
            && self.shape != Some(shape.key())
        {
            self.shape = Some(shape.key());
            events.push(CursorEvent::Shape(shape.clone()));
        }
        match sample.position {
            None => {
                if !self.hidden {
                    self.hidden = true;
                    self.position = None;
                    events.push(CursorEvent::Hidden);
                }
            }
            Some(p) => {
                self.hidden = false;
                if self.position != Some(p) {
                    self.position = Some(p);
                    events.push(CursorEvent::Moved(p));
                }
            }
        }
        events
    }
}
