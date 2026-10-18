//! Damage in output pixels: dirty rects from ScreenCaptureKit, clipped, merged and capped.

use drift_core::Size;

/// A rectangle in output pixels (top-left origin, right/bottom exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixelRect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl PixelRect {
    /// Right edge (exclusive).
    pub const fn right(&self) -> u32 {
        self.x.saturating_add(self.width)
    }

    /// Bottom edge (exclusive).
    pub const fn bottom(&self) -> u32 {
        self.y.saturating_add(self.height)
    }

    /// Area in pixels.
    pub const fn area(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// A rect in the stream's frame coordinates, scaled by `scale` to output pixels, rounded
    /// outward to whole pixels and clipped to `bounds`. `None` when empty, non-finite or outside.
    pub fn from_frame_coords(
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        scale: f64,
        bounds: Size<u32>,
    ) -> Option<Self> {
        if ![x, y, width, height, scale].iter().all(|v| v.is_finite())
            || width <= 0.0
            || height <= 0.0
            || scale <= 0.0
        {
            return None;
        }
        let clamp = |v: f64, max: u32| v.max(0.0).min(f64::from(max));
        let left = clamp((x * scale).floor(), bounds.width);
        let top = clamp((y * scale).floor(), bounds.height);
        let right = clamp(((x + width) * scale).ceil(), bounds.width);
        let bottom = clamp(((y + height) * scale).ceil(), bounds.height);
        if right <= left || bottom <= top {
            return None;
        }
        // The values are clamped to `bounds`, which are u32, so the casts are exact.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(Self {
            x: left as u32,
            y: top as u32,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        })
    }

    /// The part inside `bounds`, if any.
    pub fn clip(&self, bounds: Size<u32>) -> Option<Self> {
        let right = self.right().min(bounds.width);
        let bottom = self.bottom().min(bounds.height);
        (right > self.x && bottom > self.y).then(|| Self {
            x: self.x,
            y: self.y,
            width: right - self.x,
            height: bottom - self.y,
        })
    }

    /// Whether the two overlap or share an edge.
    pub const fn touches(&self, other: &Self) -> bool {
        self.x <= other.right()
            && other.x <= self.right()
            && self.y <= other.bottom()
            && other.y <= self.bottom()
    }

    /// The bounding box of both.
    pub fn union(&self, other: &Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            width: self.right().max(other.right()) - x,
            height: self.bottom().max(other.bottom()) - y,
        }
    }
}

/// What changed in a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Damage {
    /// Nothing (an idle frame): no need to encode.
    None,
    /// These disjoint rects, in output pixels.
    Rects(Vec<PixelRect>),
    /// Everything.
    Full,
}

/// More than this share of the frame changed: encode it as a full update.
const FULL_FRACTION: f64 = 0.5;

/// Clips `rects` to `bounds`, merges overlapping or touching ones, and caps the result: more
/// than `max_rects` become their bounding box, and more than half the frame becomes
/// [`Damage::Full`]. The result is sorted top-to-bottom, left-to-right.
pub fn coalesce(rects: &[PixelRect], bounds: Size<u32>, max_rects: usize) -> Damage {
    let mut out: Vec<PixelRect> = rects.iter().filter_map(|r| r.clip(bounds)).collect();
    // Merge until no two rects touch (quadratic, but dirty-rect lists are short).
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..out.len() {
            for j in i + 1..out.len() {
                if out[i].touches(&out[j]) {
                    out[i] = out[i].union(&out[j]);
                    out.swap_remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    if out.is_empty() {
        return Damage::None;
    }
    if out.len() > max_rects.max(1) {
        let bbox = out.iter().skip(1).fold(out[0], |acc, r| acc.union(r));
        out = vec![bbox];
    }
    let area: u64 = out.iter().map(PixelRect::area).sum();
    #[allow(clippy::cast_precision_loss)]
    let full = area as f64 > FULL_FRACTION * (u64::from(bounds.width) * u64::from(bounds.height)) as f64;
    if full {
        return Damage::Full;
    }
    out.sort_by_key(|r| (r.y, r.x));
    Damage::Rects(out)
}
