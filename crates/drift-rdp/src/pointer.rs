//! Server pointer updates → [`SessionEvent::Cursor`](crate::SessionEvent::Cursor) (M2-5 actor side). Pure.
//!
//! IronRDP's `ActiveStage` already decodes and caches fast-path pointer updates
//! (colour/new/large/cached/null/default/position). With hardware pointer rendering it hands
//! out **straight-alpha RGBA** bitmaps; [`CursorBitmap`] carries **premultiplied BGRA**, the
//! layout `NSBitmapImageRep`/`NSCursor` wants (the same as `drift_macos::CursorImage`), plus
//! the desktop scale so the app can size the cursor in points (`bitmap × 100 / scale`,
//! plan §1.4).

use drift_core::{Point, Size};
use ironrdp_session::ActiveStageOutput;

use crate::session::{CursorBitmap, CursorUpdate};

/// The cursor update for a pointer output of the active stage, `None` for other outputs.
pub fn cursor_update(output: &ActiveStageOutput, scale: u32) -> Option<CursorUpdate> {
    match output {
        ActiveStageOutput::PointerDefault => Some(CursorUpdate::Default),
        ActiveStageOutput::PointerHidden => Some(CursorUpdate::Hidden),
        ActiveStageOutput::PointerPosition { x, y } => {
            Some(CursorUpdate::Position(Point::new(u32::from(*x), u32::from(*y))))
        }
        ActiveStageOutput::PointerBitmap(pointer) => Some(CursorUpdate::Bitmap(CursorBitmap {
            size: Size::new(u32::from(pointer.width), u32::from(pointer.height)),
            hotspot: Point::new(u32::from(pointer.hotspot_x), u32::from(pointer.hotspot_y)),
            bgra: rgba_to_premultiplied_bgra(&pointer.bitmap_data).into(),
            scale,
        })),
        _ => None,
    }
}

/// Straight-alpha RGBA → premultiplied BGRA (same length; a trailing partial pixel is dropped).
pub fn rgba_to_premultiplied_bgra(rgba: &[u8]) -> Vec<u8> {
    let mul = |c: u8, a: u8| u8::try_from((u16::from(c) * u16::from(a) + 127) / 255).unwrap_or(u8::MAX);
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [mul(p[2], p[3]), mul(p[1], p[3]), mul(p[0], p[3]), p[3]])
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ironrdp_graphics::pointer::DecodedPointer;

    use super::*;

    #[test]
    fn pointer_outputs_become_cursor_updates() {
        assert_eq!(cursor_update(&ActiveStageOutput::PointerDefault, 100), Some(CursorUpdate::Default));
        assert_eq!(cursor_update(&ActiveStageOutput::PointerHidden, 100), Some(CursorUpdate::Hidden));
        assert_eq!(
            cursor_update(&ActiveStageOutput::PointerPosition { x: 7, y: 9 }, 100),
            Some(CursorUpdate::Position(Point::new(7, 9)))
        );
        assert_eq!(cursor_update(&ActiveStageOutput::DeactivateAll, 100), None);
    }

    #[test]
    fn a_bitmap_pointer_is_premultiplied_bgra_with_its_scale() {
        // One opaque red pixel and one half-transparent blue pixel.
        let decoded = DecodedPointer {
            width: 2,
            height: 1,
            hotspot_x: 1,
            hotspot_y: 0,
            bitmap_data: vec![255, 0, 0, 255, 0, 0, 255, 128],
        };
        let update = cursor_update(&ActiveStageOutput::PointerBitmap(Arc::new(decoded)), 200);
        let Some(CursorUpdate::Bitmap(bitmap)) = update else { panic!("{update:?}") };
        assert_eq!(bitmap.size, Size::new(2, 1));
        assert_eq!(bitmap.hotspot, Point::new(1, 0));
        assert_eq!(bitmap.scale, 200);
        assert_eq!(&*bitmap.bgra, &[0, 0, 255, 255, 128, 0, 0, 128]);
    }

    #[test]
    fn a_partial_trailing_pixel_is_dropped() {
        assert_eq!(rgba_to_premultiplied_bgra(&[1, 2, 3, 255, 9]), vec![3, 2, 1, 255]);
        assert!(rgba_to_premultiplied_bgra(&[]).is_empty());
    }
}
