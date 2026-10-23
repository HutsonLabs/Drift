//! Golden-image utilities (task M1-5): PNG load/save of BGRA8 images and a per-channel
//! tolerance comparison (plan §0: GPU goldens use a per-channel tolerance ≤ 2).
//!
//! Images are passed around as tightly packed BGRA8 bytes (`width * 4` bytes per row), the
//! byte order of Metal's `BGRA8Unorm` and of GFX surfaces. PNG files store them as RGBA8.
//!
//! Set `DRIFT_UPDATE_GOLDENS=1` to (re)write a golden from the *reference* image passed to
//! [`assert_golden`] instead of comparing against it.

use std::fmt;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

/// Environment variable that switches [`assert_golden`] into "write" mode.
pub const UPDATE_ENV: &str = "DRIFT_UPDATE_GOLDENS";

/// A decoded golden image: tightly packed BGRA8.
#[derive(Clone, PartialEq, Eq)]
pub struct GoldenImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, B,G,R,A per pixel, rows top to bottom.
    pub bgra: Vec<u8>,
}

impl fmt::Debug for GoldenImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GoldenImage({}x{})", self.width, self.height)
    }
}

/// Why two images differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// Dimensions differ.
    Size {
        /// Expected `(width, height)`.
        expected: (u32, u32),
        /// Actual `(width, height)`.
        actual: (u32, u32),
    },
    /// At least one channel differs by more than the tolerance.
    Pixels {
        /// Number of pixels outside tolerance.
        count: usize,
        /// First offending pixel `(x, y)`.
        first: (u32, u32),
        /// Expected BGRA at `first`.
        expected: [u8; 4],
        /// Actual BGRA at `first`.
        actual: [u8; 4],
        /// Largest per-channel difference seen.
        max_diff: u8,
    },
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Size { expected, actual } => {
                write!(f, "size mismatch: expected {expected:?}, got {actual:?}")
            }
            Self::Pixels { count, first, expected, actual, max_diff } => write!(
                f,
                "{count} pixel(s) differ (max channel diff {max_diff}); first at {first:?}: expected BGRA {expected:?}, got {actual:?}"
            ),
        }
    }
}

/// Compares two BGRA8 images channel by channel.
pub fn compare(expected: &GoldenImage, actual: &GoldenImage, tolerance: u8) -> Result<(), Mismatch> {
    if (expected.width, expected.height) != (actual.width, actual.height)
        || expected.bgra.len() != actual.bgra.len()
    {
        return Err(Mismatch::Size {
            expected: (expected.width, expected.height),
            actual: (actual.width, actual.height),
        });
    }
    let mut count = 0usize;
    let mut first = None;
    let mut max_diff = 0u8;
    for (i, (e, a)) in expected.bgra.as_chunks::<4>().0.iter().zip(actual.bgra.as_chunks::<4>().0).enumerate()
    {
        let d = e.iter().zip(a).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        if d > tolerance {
            count += 1;
            max_diff = max_diff.max(d);
            if first.is_none() {
                let w = expected.width.max(1) as usize;
                let px = |s: &[u8]| [s[0], s[1], s[2], s[3]];
                first = Some((((i % w) as u32, (i / w) as u32), px(e), px(a)));
            }
        }
    }
    match first {
        None => Ok(()),
        Some((first, expected, actual)) => Err(Mismatch::Pixels { count, first, expected, actual, max_diff }),
    }
}

/// Loads a PNG (RGB8 or RGBA8) as BGRA8.
pub fn load_png(path: &Path) -> Result<GoldenImage, String> {
    let file = File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("png header {}: {e}", path.display()))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("png decode {}: {e}", path.display()))?;
    let src = &buf[..info.buffer_size()];
    let bgra = match info.color_type {
        png::ColorType::Rgba => {
            src.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect()
        }
        png::ColorType::Rgb => src.as_chunks::<3>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect(),
        other => return Err(format!("unsupported png colour type {other:?} in {}", path.display())),
    };
    Ok(GoldenImage { width: info.width, height: info.height, bgra })
}

/// Saves a BGRA8 image as an RGBA8 PNG, creating parent directories.
pub fn save_png(path: &Path, image: &GoldenImage) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    let file = File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), image.width, image.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| format!("png header: {e}"))?;
    let rgba: Vec<u8> = image.bgra.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    w.write_image_data(&rgba).map_err(|e| format!("png write: {e}"))?;
    w.finish().map_err(|e| format!("png finish: {e}"))
}

/// Asserts that `actual` matches the golden PNG at `path` within `tolerance`.
///
/// With `DRIFT_UPDATE_GOLDENS=1` the golden is first (re)written from `reference` — the
/// independently computed expectation (e.g. a CPU model), never from `actual` — and then
/// `actual` is still compared against it.
///
/// # Panics
/// On mismatch or I/O error (this is a test helper).
#[track_caller]
pub fn assert_golden(path: &Path, reference: &GoldenImage, actual: &GoldenImage, tolerance: u8) {
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        save_png(path, reference).unwrap_or_else(|e| panic!("writing golden: {e}"));
    }
    let golden = load_png(path)
        .unwrap_or_else(|e| panic!("golden {}: {e} (set {UPDATE_ENV}=1 to create)", path.display()));
    if let Err(m) = compare(&golden, reference, 0) {
        panic!("golden {} is stale versus its reference model: {m}", path.display());
    }
    if let Err(m) = compare(&golden, actual, tolerance) {
        panic!("golden {} mismatch (tolerance {tolerance}): {m}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(px: [u8; 4]) -> GoldenImage {
        GoldenImage { width: 2, height: 1, bgra: [px, px].concat() }
    }

    #[test]
    fn tolerance_is_per_channel() {
        assert!(compare(&img([10, 20, 30, 255]), &img([12, 18, 30, 255]), 2).is_ok());
        let err = compare(&img([10, 20, 30, 255]), &img([13, 20, 30, 255]), 2).unwrap_err();
        assert!(matches!(err, Mismatch::Pixels { count: 2, first: (0, 0), max_diff: 3, .. }), "{err}");
        let other = GoldenImage { width: 1, height: 2, bgra: vec![0; 8] };
        assert!(matches!(compare(&img([0; 4]), &other, 255), Err(Mismatch::Size { .. })));
    }

    #[test]
    fn png_round_trip_preserves_bgra() {
        let dir = std::env::temp_dir().join(format!("drift-golden-{}", std::process::id()));
        let path = dir.join("rt.png");
        let image = GoldenImage { width: 2, height: 2, bgra: (0u8..16).collect() };
        save_png(&path, &image).unwrap();
        assert_eq!(load_png(&path).unwrap(), image);
        assert_golden(&path, &image, &image, 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
