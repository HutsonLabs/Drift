//! The current pointer position and shape (humble object; change detection is
//! [`crate::cursor::CursorTracker`]).
//!
//! The stream runs with `showsCursor` off, so the pointer is sampled here and sent to the client
//! as RDP pointer updates. The position comes from a null `CGEvent` (global points, top-left
//! origin), the shape from `NSCursor.currentSystemCursor`, drawn into an RGBA bitmap.

use std::ffi::c_void;

use drift_core::{Point, Size};
use objc2_app_kit::NSCursor;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColorSpace, CGContext, CGEvent, CGImage, CGImageAlphaInfo};

use crate::cursor::{CursorSample, CursorShape};
use crate::display::DisplayInfo;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: Option<&CGColorSpace>,
        bitmap_info: u32,
    ) -> *mut CGContext;
    fn CGContextRelease(c: *mut CGContext);
}

/// Cursor images larger than this are skipped (RDP pointers are at most 384×384).
const MAX_SIDE: usize = 384;

/// Samples the pointer relative to `display`.
pub fn sample(display: &DisplayInfo) -> CursorSample {
    let position = CGEvent::new(None).and_then(|e| {
        let p = CGEvent::location(Some(&e));
        display.point_to_pixels(p.x, p.y)
    });
    CursorSample { position, shape: current_shape(display.scale()) }
}

fn current_shape(scale: f64) -> Option<CursorShape> {
    // Deprecated in favour of drawing the pointer into the stream, which is exactly what Drift
    // must not do (RDP sends the pointer separately); it is the only public API for the system
    // pointer's shape (ADR HOST-3-capture).
    #[allow(deprecated)]
    let cursor = NSCursor::currentSystemCursor()?;
    let image = cursor.image();
    let hotspot = cursor.hotSpot();
    let mut rect = objc2_foundation::NSRect::new(objc2_foundation::NSPoint::new(0.0, 0.0), image.size());
    // SAFETY: asks AppKit for a CGImage of the cursor image at its natural size; no context.
    let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    let (width, height) = (CGImage::width(Some(&cg)), CGImage::height(Some(&cg)));
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let mut rgba = vec![0_u8; width * height * 4];
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: `rgba` holds width × height × 4 bytes and outlives the context, which is
    // released below before the buffer is read.
    let ctx = unsafe {
        CGBitmapContextCreate(
            rgba.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    };
    // SAFETY: checked for null; drawn once, then released.
    unsafe {
        let ctx_ref = ctx.as_ref()?;
        let bounds = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: width as f64, height: height as f64 },
        };
        CGContext::draw_image(Some(ctx_ref), bounds, Some(&cg));
        CGContextRelease(ctx);
    }
    unpremultiply(&mut rgba);
    // The image is in pixels (Retina cursors are 2×); the hotspot is in points.
    let px_scale = width as f64 / image.size().width.max(1.0);
    let _ = scale;
    let hx = ((hotspot.x * px_scale).round() as u32).min(width as u32 - 1);
    let hy = ((hotspot.y * px_scale).round() as u32).min(height as u32 - 1);
    CursorShape::new(Size::new(width as u32, height as u32), Point::new(hx, hy), rgba)
}

/// Premultiplied RGBA to straight RGBA, in place.
fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = u16::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}
