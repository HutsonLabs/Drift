//! Thin CoreVideo/CoreFoundation helpers shared by the decoder and encoder (humble FFI).
// The writers are only used by the encoder (feature `recording`).
#![cfg_attr(not(feature = "recording"), allow(dead_code))]

use std::ptr::NonNull;

use drift_core::{Nv12Planes, Size};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
    CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetIOSurface,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetPlaneCount, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVReturnSuccess,
};

/// `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange` (`'420f'`).
pub const NV12_FULL_RANGE: u32 = 0x3432_3066;
/// `kCVPixelFormatType_32BGRA` (`'BGRA'`).
pub const BGRA: u32 = 0x4247_5241;

/// A retained `CVPixelBuffer` that may cross threads.
///
/// CoreVideo pixel buffers are reference counted with atomic retain/release and their
/// attachments/backing memory are safe to access from any thread; Drift only reads a decoded
/// buffer's pixels under `CVPixelBufferLockBaseAddress`, so sharing a handle is sound.
#[derive(Debug, Clone)]
pub struct SharedPixelBuffer(pub CFRetained<CVPixelBuffer>);

// SAFETY: see the type docs: CVPixelBuffer is an immutable-after-creation, atomically
// reference-counted CF object; concurrent access to pixel data is serialised by the base-address
// lock and Drift never mutates a buffer after handing it out.
unsafe impl Send for SharedPixelBuffer {}
// SAFETY: as above; `&SharedPixelBuffer` only exposes read access.
unsafe impl Sync for SharedPixelBuffer {}

/// Attributes for IOSurface-backed, Metal-compatible pixel buffers of `format`.
pub fn pixel_buffer_attributes(format: u32) -> CFRetained<CFDictionary> {
    let empty = CFDictionary::<CFString, CFType>::empty();
    let format = CFNumber::new_i32(i32::from_ne_bytes(format.to_ne_bytes()));
    // SAFETY: the CoreVideo attribute keys are immutable CFString constants exported by the framework.
    let keys: [&CFString; 3] = unsafe {
        [
            kCVPixelBufferPixelFormatTypeKey,
            kCVPixelBufferIOSurfacePropertiesKey,
            kCVPixelBufferMetalCompatibilityKey,
        ]
    };
    let values: [&CFType; 3] = [format.as_ref(), (*empty).as_ref(), CFBoolean::new(true).as_ref()];
    let dict = CFDictionary::from_slices(&keys, &values);
    // SAFETY: erasing the key/value types of a CFDictionary is always valid (CF collections are untyped).
    unsafe { CFRetained::cast_unchecked(dict) }
}

/// Creates an IOSurface-backed pixel buffer.
pub fn create_pixel_buffer(size: Size<u32>, format: u32) -> Result<CFRetained<CVPixelBuffer>, i32> {
    let attrs = pixel_buffer_attributes(format);
    let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
    // SAFETY: `out` is a valid out-pointer; `attrs` is a valid CFDictionary for the call's duration.
    let status = unsafe {
        objc2_core_video::CVPixelBufferCreate(
            None,
            size.width as usize,
            size.height as usize,
            format,
            Some(&attrs),
            NonNull::from(&mut out),
        )
    };
    if status != kCVReturnSuccess {
        return Err(status);
    }
    let out = NonNull::new(out).ok_or(-1)?;
    // SAFETY: CVPixelBufferCreate returned +1 retained buffer in `out` (Create rule).
    Ok(unsafe { CFRetained::from_raw(out) })
}

/// Size of a pixel buffer in pixels.
pub fn size_of(pb: &CVPixelBuffer) -> Size<u32> {
    let w = u32::try_from(CVPixelBufferGetWidth(pb)).unwrap_or(u32::MAX);
    let h = u32::try_from(CVPixelBufferGetHeight(pb)).unwrap_or(u32::MAX);
    Size::new(w, h)
}

/// Pixel format of a pixel buffer.
pub fn format_of(pb: &CVPixelBuffer) -> u32 {
    CVPixelBufferGetPixelFormatType(pb)
}

/// Whether the buffer is backed by an IOSurface.
pub fn is_iosurface_backed(pb: &CVPixelBuffer) -> bool {
    CVPixelBufferGetIOSurface(Some(pb)).is_some()
}

/// Base-address lock held for the guard's lifetime.
struct Lock<'a> {
    pb: &'a CVPixelBuffer,
    flags: CVPixelBufferLockFlags,
}

impl<'a> Lock<'a> {
    fn new(pb: &'a CVPixelBuffer, read_only: bool) -> Result<Self, i32> {
        let flags = if read_only { CVPixelBufferLockFlags::ReadOnly } else { CVPixelBufferLockFlags(0) };
        // SAFETY: `pb` is a valid pixel buffer; the matching unlock happens in Drop with the same flags.
        let status = unsafe { CVPixelBufferLockBaseAddress(pb, flags) };
        if status != kCVReturnSuccess {
            return Err(status);
        }
        Ok(Self { pb, flags })
    }

    /// Plane `index` as (base pointer, bytes per row, rows).
    fn plane(&self, index: usize) -> Option<(NonNull<u8>, usize, usize)> {
        let base = NonNull::new(CVPixelBufferGetBaseAddressOfPlane(self.pb, index).cast::<u8>())?;
        Some((
            base,
            CVPixelBufferGetBytesPerRowOfPlane(self.pb, index),
            CVPixelBufferGetHeightOfPlane(self.pb, index),
        ))
    }

    fn base(&self) -> Option<(NonNull<u8>, usize, usize)> {
        let base = NonNull::new(objc2_core_video::CVPixelBufferGetBaseAddress(self.pb).cast::<u8>())?;
        Some((base, objc2_core_video::CVPixelBufferGetBytesPerRow(self.pb), CVPixelBufferGetHeight(self.pb)))
    }
}

impl Drop for Lock<'_> {
    fn drop(&mut self) {
        // SAFETY: the buffer was locked with `self.flags` in `Lock::new`.
        unsafe { CVPixelBufferUnlockBaseAddress(self.pb, self.flags) };
    }
}

/// Copies an NV12 pixel buffer's planes into CPU memory.
pub fn read_nv12(pb: &CVPixelBuffer) -> Result<Nv12Planes, String> {
    let size = size_of(pb);
    if CVPixelBufferGetPlaneCount(pb) != 2 {
        return Err("not a bi-planar pixel buffer".into());
    }
    let lock = Lock::new(pb, true).map_err(|s| format!("CVPixelBufferLockBaseAddress: {s}"))?;
    let w = size.width as usize;
    let h = size.height as usize;
    let copy = |index: usize, rows: usize| -> Result<Vec<u8>, String> {
        let (base, stride, plane_rows) = lock.plane(index).ok_or("missing plane")?;
        if stride < w || plane_rows < rows {
            return Err(format!("plane {index}: stride {stride} rows {plane_rows}"));
        }
        let mut out = Vec::with_capacity(w * rows);
        for row in 0..rows {
            // SAFETY: the plane is locked, `row < plane_rows` and `w <= stride`, so the slice lies
            // inside the plane's `stride * plane_rows` bytes.
            let line = unsafe { std::slice::from_raw_parts(base.as_ptr().add(row * stride), w) };
            out.extend_from_slice(line);
        }
        Ok(out)
    };
    let y = copy(0, h)?;
    let uv = copy(1, h.div_ceil(2))?;
    drop(lock);
    Nv12Planes::new(size, y, uv).ok_or_else(|| "plane size mismatch".into())
}

/// Writes CPU NV12 planes into an NV12 pixel buffer of the same size.
pub fn write_nv12(pb: &CVPixelBuffer, planes: &Nv12Planes) -> Result<(), String> {
    let size = size_of(pb);
    let w = size.width as usize;
    let h = size.height as usize;
    if planes.y().len() != w * h || CVPixelBufferGetPlaneCount(pb) != 2 {
        return Err("NV12 size mismatch".into());
    }
    let lock = Lock::new(pb, false).map_err(|s| format!("CVPixelBufferLockBaseAddress: {s}"))?;
    for (index, src, rows) in [(0, planes.y(), h), (1, planes.uv(), h.div_ceil(2))] {
        let (base, stride, plane_rows) = lock.plane(index).ok_or("missing plane")?;
        if stride < w || plane_rows < rows {
            return Err(format!("plane {index}: stride {stride} rows {plane_rows}"));
        }
        for row in 0..rows {
            // SAFETY: the plane is locked for writing and the destination row lies inside it
            // (`row < plane_rows`, `w <= stride`); the source row is in bounds of `src`.
            unsafe {
                std::ptr::copy_nonoverlapping(src[row * w..].as_ptr(), base.as_ptr().add(row * stride), w);
            }
        }
    }
    Ok(())
}

/// Copies a BGRA pixel buffer into CPU memory, tightly packed (`4 * width` bytes per row).
pub fn read_bgra(pb: &CVPixelBuffer) -> Result<Vec<u8>, String> {
    if format_of(pb) != BGRA {
        return Err("not a BGRA buffer".into());
    }
    let size = size_of(pb);
    let lock = Lock::new(pb, true).map_err(|s| format!("CVPixelBufferLockBaseAddress: {s}"))?;
    let (base, stride, rows) = lock.base().ok_or("no base address")?;
    let (w, h) = (size.width as usize, size.height as usize);
    if stride < w * 4 || rows < h {
        return Err("BGRA size mismatch".into());
    }
    let mut out = Vec::with_capacity(w * 4 * h);
    for y in 0..h {
        // SAFETY: the buffer is locked; row `y < rows` spans `stride >= 4 * w` bytes.
        let line = unsafe { std::slice::from_raw_parts(base.as_ptr().add(y * stride), w * 4) };
        out.extend_from_slice(line);
    }
    Ok(out)
}

/// Fills a BGRA pixel buffer: `f(x, y)` returns `[b, g, r, a]`.
pub fn fill_bgra(pb: &CVPixelBuffer, f: impl Fn(u32, u32) -> [u8; 4]) -> Result<(), String> {
    let size = size_of(pb);
    let lock = Lock::new(pb, false).map_err(|s| format!("CVPixelBufferLockBaseAddress: {s}"))?;
    let (base, stride, rows) = lock.base().ok_or("no base address")?;
    let w = size.width as usize;
    if stride < w * 4 || rows < size.height as usize {
        return Err("BGRA size mismatch".into());
    }
    for y in 0..size.height {
        // SAFETY: the buffer is locked for writing; row `y < rows` spans `stride >= 4 * w` bytes.
        let line = unsafe { std::slice::from_raw_parts_mut(base.as_ptr().add(y as usize * stride), w * 4) };
        for (x, px) in line.chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&f(x as u32, y));
        }
    }
    Ok(())
}
