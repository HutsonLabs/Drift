//! `NSCursor` from decoded remote pointers (task **M2-5**, AppKit part).
//!
//! The image is sized in points as `bitmap / (scale / 100)` so an 86×86 pointer at desktop
//! scale 200 shows as a crisp 43×43-point cursor on a Retina display; the hotspot is converted
//! the same way.

use objc2::rc::Retained;
use objc2::{AllocAnyThread as _, MainThreadMarker};
use objc2_app_kit::{NSBitmapFormat, NSBitmapImageRep, NSCursor, NSDeviceRGBColorSpace, NSImage};
use objc2_foundation::{NSPoint, NSSize};

use crate::cursor::{CursorImage, CursorShape};

/// Builds an `NSBitmapImageRep` (premultiplied RGBA, 8 bits per channel) from premultiplied
/// BGRA pixels.
fn bitmap_rep(width: usize, height: usize, bgra: &[u8]) -> Option<Retained<NSBitmapImageRep>> {
    if width == 0 || height == 0 || bgra.len() != width * height * 4 {
        return None;
    }
    // SAFETY: null planes make AppKit allocate the buffer; the arguments describe a valid
    // 8-bit, 4-sample, non-planar, premultiplied RGBA layout.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            width as isize,
            height as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::empty(),
            (width * 4) as isize,
            32,
        )
    }?;
    let row_bytes = usize::try_from(rep.bytesPerRow()).ok()?;
    let dst = rep.bitmapData();
    if dst.is_null() || row_bytes < width * 4 {
        return None;
    }
    for (y, src_row) in bgra.chunks_exact(width * 4).enumerate() {
        // SAFETY: AppKit allocated `row_bytes * height` bytes; row `y < height` starts inside it
        // and we write `width * 4 <= row_bytes` bytes.
        let dst_row = unsafe { std::slice::from_raw_parts_mut(dst.add(y * row_bytes), width * 4) };
        for (d, s) in dst_row.as_chunks_mut::<4>().0.iter_mut().zip(src_row.as_chunks::<4>().0) {
            d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
    }
    Some(rep)
}

/// Builds an `NSCursor` for `image` at desktop scale `scale_percent`. `None` if AppKit cannot
/// allocate the bitmap (or the image is empty).
pub fn ns_cursor(
    mtm: MainThreadMarker,
    image: &CursorImage,
    scale_percent: u32,
) -> Option<Retained<NSCursor>> {
    let _ = mtm;
    let rep = bitmap_rep(image.size.width as usize, image.size.height as usize, &image.bgra)?;
    let points = image.size_points(scale_percent);
    let size = NSSize::new(points.width, points.height);
    rep.setSize(size);
    let ns_image = NSImage::initWithSize(NSImage::alloc(), size);
    ns_image.addRepresentation(&rep);
    let hs = image.hotspot_points(scale_percent);
    Some(NSCursor::initWithImage_hotSpot(NSCursor::alloc(), &ns_image, NSPoint::new(hs.x, hs.y)))
}

/// A fully transparent cursor (the remote hid the pointer).
pub fn hidden_cursor(mtm: MainThreadMarker) -> Retained<NSCursor> {
    let transparent = CursorImage {
        size: drift_core::Size::new(1, 1),
        hotspot: drift_core::Point::new(0, 0),
        bgra: vec![0u8; 4].into(),
    };
    ns_cursor(mtm, &transparent, 100).unwrap_or_else(NSCursor::arrowCursor)
}

/// The cursor to show for `shape`; falls back to the arrow if an image cannot be built.
pub fn cursor_for(mtm: MainThreadMarker, shape: &CursorShape, scale_percent: u32) -> Retained<NSCursor> {
    match shape {
        CursorShape::Default => NSCursor::arrowCursor(),
        CursorShape::Hidden => hidden_cursor(mtm),
        CursorShape::Image(img) => ns_cursor(mtm, img, scale_percent).unwrap_or_else(NSCursor::arrowCursor),
    }
}
