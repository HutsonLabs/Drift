//! The host's pointer, sent as RDP pointer updates (the picture is captured without it, HOST-3).
//!
//! Shapes go out once and are then referred to by cache slot. RDP's 32 bpp pointer image is BGRA
//! with the bottom row first; capture hands over straight RGBA with the top row first.

use drift_capture::cursor::{CursorEvent, CursorShape};
use ironrdp_pdu::pointer::PointerPositionAttribute;
use ironrdp_server::{DisplayUpdate, LargePointer, RGBAPointer};

/// Pointer cache slots offered to the client (MS-RDPBCGR's minimum pointer cache is 25).
pub const CACHE_SLOTS: u16 = 25;

/// The largest pointer `RGBAPointer` carries; bigger ones go as a large pointer (up to 384).
pub const RGBA_MAX_SIDE: u32 = 96;

/// Which shape sits in which client cache slot.
#[derive(Debug, Default)]
pub struct PointerCache {
    slots: Vec<u64>,
    next: u16,
}

impl PointerCache {
    /// The slot already holding `key`, if any.
    fn find(&self, key: u64) -> Option<u16> {
        self.slots.iter().position(|&k| k == key).and_then(|i| u16::try_from(i).ok())
    }

    /// Stores `key` in the next slot (round robin) and returns it.
    fn store(&mut self, key: u64) -> u16 {
        let slot = self.next;
        let i = usize::from(slot);
        if i < self.slots.len() {
            self.slots[i] = key;
        } else {
            self.slots.push(key);
        }
        self.next = (self.next + 1) % CACHE_SLOTS;
        slot
    }
}

/// RGBA top-down → BGRA bottom-up (the 32 bpp XOR mask of a pointer update).
pub fn to_rdp_pixels(shape: &CursorShape) -> Vec<u8> {
    let width = shape.size().width as usize;
    let stride = width * 4;
    let mut out = Vec::with_capacity(shape.rgba().len());
    for row in shape.rgba().chunks_exact(stride).rev() {
        for px in row.chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out
}

/// The display update for one pointer change (`None` for a shape too big to send).
pub fn pointer_update(event: &CursorEvent, cache: &mut PointerCache) -> Option<DisplayUpdate> {
    match event {
        CursorEvent::Moved(p) => Some(DisplayUpdate::PointerPosition(PointerPositionAttribute {
            x: u16::try_from(p.x).unwrap_or(u16::MAX),
            y: u16::try_from(p.y).unwrap_or(u16::MAX),
        })),
        CursorEvent::Hidden => Some(DisplayUpdate::HidePointer),
        CursorEvent::Shape(shape) => {
            if let Some(slot) = cache.find(shape.key()) {
                return Some(DisplayUpdate::CachedPointer(slot));
            }
            let size = shape.size();
            let side = size.width.max(size.height);
            if side > 384 {
                return None;
            }
            let cache_index = cache.store(shape.key());
            let (width, height) = (u16::try_from(size.width).ok()?, u16::try_from(size.height).ok()?);
            let (hot_x, hot_y) =
                (u16::try_from(shape.hotspot().x).ok()?, u16::try_from(shape.hotspot().y).ok()?);
            let data = to_rdp_pixels(shape);
            Some(if side <= RGBA_MAX_SIDE {
                DisplayUpdate::RGBAPointer(RGBAPointer { cache_index, width, height, hot_x, hot_y, data })
            } else {
                DisplayUpdate::LargePointer(LargePointer { cache_index, width, height, hot_x, hot_y, data })
            })
        }
    }
}
