//! AVC444 (v1) for the Mac host (task **HOST-4**, MS-RDPEGFX 3.3.8.3.2; ADR
//! `HOST-4-realtime-encoder`).
//!
//! AVC444 carries a full-chroma picture as two ordinary YUV420 pictures, each encoded as its own
//! AVC420 stream:
//!
//! * the **main view** is the luma plane plus each 2×2 block's average U and V, so a client that
//!   only shows the main view still sees a correct 4:2:0 picture;
//! * the **auxiliary view** holds the chroma samples the main view dropped. Its luma plane takes
//!   the odd chroma rows in 16-row bands (8 rows of U, then 8 rows of V), and its chroma planes
//!   take U and V at the odd columns of the even rows.
//!
//! The auxiliary view is as wide as the picture and as tall as the picture rounded up to 16
//! rows, so every band is whole (the encoder pads to whole macroblocks anyway). [`split`] builds
//! both views from a [`Yuv444`] picture, [`combine`] puts them back (the even
//! row, even column sample is rebuilt from the average and its three neighbours, exact up to the
//! average's rounding). [`bgra_to_yuv444`] uses g-r-d's integer BT.709 full-range matrix, the
//! one Drift's decoder inverts (`drift-render::color`), so a Mac host and a GNOME host look the
//! same to the client. [`Avc444Encoder`] runs one [`VtEncoder`] per view.

use std::collections::VecDeque;
use std::time::Instant;

use drift_core::{Nv12Planes, Nv12Source as _, Size};
use rayon::prelude::*;

use crate::encode::{EncodeError, EncodedFrame, EncoderConfig, PixelBuffer, VtEncoder};

/// A planar 4:4:4 picture: Y, U and V at full resolution, row-major, no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Yuv444 {
    size: Size<u32>,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl Yuv444 {
    /// Builds a picture; every plane must hold `width × height` samples.
    pub fn new(size: Size<u32>, y: Vec<u8>, u: Vec<u8>, v: Vec<u8>) -> Option<Self> {
        let n = usize::try_from(size.width).ok()?.checked_mul(usize::try_from(size.height).ok()?)?;
        (y.len() == n && u.len() == n && v.len() == n).then_some(Self { size, y, u, v })
    }

    /// Size in pixels.
    pub fn size(&self) -> Size<u32> {
        self.size
    }

    /// The luma plane.
    pub fn y(&self) -> &[u8] {
        &self.y
    }

    /// The U (Cb) plane.
    pub fn u(&self) -> &[u8] {
        &self.u
    }

    /// The V (Cr) plane.
    pub fn v(&self) -> &[u8] {
        &self.v
    }
}

/// Neutral chroma, used for the auxiliary view's padding rows.
const GREY: u8 = 128;

fn dims(size: Size<u32>) -> Option<(usize, usize)> {
    Some((usize::try_from(size.width).ok()?, usize::try_from(size.height).ok()?))
}

/// Converts a BGRA picture (`stride` bytes per row) with g-r-d's integer matrix:
/// `Y = (54R + 183G + 18B) >> 8`, `U = ((-29R - 99G + 128B) >> 8) + 128`,
/// `V = ((128R - 116G - 12B) >> 8) + 128`. `None` if the buffer is too short.
pub fn bgra_to_yuv444(bgra: &[u8], size: Size<u32>, stride: usize) -> Option<Yuv444> {
    let (w, h) = dims(size)?;
    if h > 0 && (stride < w.checked_mul(4)? || bgra.len() < stride.checked_mul(h - 1)?.checked_add(w * 4)?) {
        return None;
    }
    let n = w.checked_mul(h)?;
    let (mut y, mut u, mut v) = (vec![0u8; n], vec![0u8; n], vec![0u8; n]);
    let clamp = |x: i32| x.clamp(0, 255) as u8;
    y.par_chunks_mut(w.max(1))
        .zip(u.par_chunks_mut(w.max(1)))
        .zip(v.par_chunks_mut(w.max(1)))
        .enumerate()
        .for_each(|(row, ((yr, ur), vr))| {
            let src = &bgra[row * stride..row * stride + w * 4];
            for (x, px) in src.chunks_exact(4).enumerate() {
                let (b, g, r) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
                yr[x] = clamp((54 * r + 183 * g + 18 * b) >> 8);
                ur[x] = clamp(((-29 * r - 99 * g + 128 * b) >> 8) + 128);
                vr[x] = clamp(((128 * r - 116 * g - 12 * b) >> 8) + 128);
            }
        });
    Some(Yuv444 { size, y, u, v })
}

/// Converts back to BGRA (opaque) with the BT.709 full-range inverse of [`bgra_to_yuv444`]'s
/// matrix: `R = Y + 1.5748 V'`, `G = Y - 0.1873 U' - 0.4681 V'`, `B = Y + 1.8556 U'` with
/// `U' = U - 128`, `V' = V - 128` (fixed point, 16 fractional bits).
pub fn yuv444_to_bgra(pic: &Yuv444) -> Vec<u8> {
    let Some((w, h)) = dims(pic.size) else { return Vec::new() };
    let mut out = vec![0u8; w * h * 4];
    if w == 0 {
        return out;
    }
    let clamp = |x: i32| x.clamp(0, 255) as u8;
    out.par_chunks_mut(w * 4).enumerate().for_each(|(row, dst)| {
        let (ys, us, vs) = (&pic.y[row * w..][..w], &pic.u[row * w..][..w], &pic.v[row * w..][..w]);
        for (x, px) in dst.chunks_exact_mut(4).enumerate() {
            let y = i32::from(ys[x]) << 16;
            let (u, v) = (i32::from(us[x]) - 128, i32::from(vs[x]) - 128);
            let r = (y + 103_206 * v + 32_768) >> 16;
            let g = (y - 12_275 * u - 30_677 * v + 32_768) >> 16;
            let b = (y + 121_609 * u + 32_768) >> 16;
            px.copy_from_slice(&[clamp(b), clamp(g), clamp(r), 255]);
        }
    });
    out
}

/// A full-chroma picture from the main view alone (each 4:2:0 chroma sample repeated over its
/// 2×2 block): what a client shows before the first auxiliary view arrives.
pub fn upsample_main(main: &Nv12Planes) -> Option<Yuv444> {
    let size = main.size();
    let (w, h) = dims(size)?;
    let (mut u, mut v) = (vec![GREY; w * h], vec![GREY; w * h]);
    let uv = main.uv();
    for row in 0..h {
        for x in 0..w {
            let i = (row / 2) * w + (x / 2) * 2;
            u[row * w + x] = *uv.get(i)?;
            v[row * w + x] = *uv.get(i + 1)?;
        }
    }
    Yuv444::new(size, main.y().to_vec(), u, v)
}

/// The source chroma row and plane of auxiliary-view luma row `row`: in each 16-row band, rows
/// 0–7 are U rows `2(8k + r) + 1`, rows 8–15 are V rows `2(8k + r) + 1`.
fn aux_luma_source(row: usize) -> (bool, usize) {
    let (band, r) = (row / 16, row % 16);
    let is_v = r >= 8;
    (is_v, 2 * (8 * band + r % 8) + 1)
}

/// The auxiliary view's height for a picture `h` rows tall: whole 16-row bands.
pub fn aux_height(h: u32) -> u32 {
    h.div_ceil(16).saturating_mul(16)
}

/// Splits a 4:4:4 picture into the AVC444 main view (the picture's size) and auxiliary view
/// (height rounded up by [`aux_height`]), both NV12. `None` for odd widths or heights.
pub fn split(pic: &Yuv444) -> Option<(Nv12Planes, Nv12Planes)> {
    let (w, h) = dims(pic.size)?;
    if !w.is_multiple_of(2) || !h.is_multiple_of(2) || w == 0 || h == 0 {
        return None;
    }
    let (u, v) = (&pic.u, &pic.v);
    // Main view: full luma, 2×2-averaged chroma (rounded), interleaved as NV12.
    let mut main_uv = vec![0u8; w * h / 2];
    main_uv.par_chunks_mut(w).enumerate().for_each(|(cy, out)| {
        let (r0, r1) = (2 * cy * w, (2 * cy + 1) * w);
        for cx in 0..w / 2 {
            let avg = |p: &[u8]| {
                let s = u32::from(p[r0 + 2 * cx])
                    + u32::from(p[r0 + 2 * cx + 1])
                    + u32::from(p[r1 + 2 * cx])
                    + u32::from(p[r1 + 2 * cx + 1]);
                ((s + 2) / 4) as u8
            };
            out[2 * cx] = avg(u);
            out[2 * cx + 1] = avg(v);
        }
    });
    // Auxiliary view luma: odd chroma rows in 16-row bands, grey past the picture.
    let ah = usize::try_from(aux_height(pic.size.height)).ok()?;
    let mut aux_y = vec![GREY; w * ah];
    aux_y.par_chunks_mut(w).enumerate().for_each(|(row, out)| {
        let (is_v, src) = aux_luma_source(row);
        if src < h {
            let plane = if is_v { v } else { u };
            out.copy_from_slice(&plane[src * w..(src + 1) * w]);
        }
    });
    // Auxiliary view chroma: odd columns of the even rows, grey past the picture.
    let mut aux_uv = vec![GREY; w * ah / 2];
    aux_uv[..w * h / 2].par_chunks_mut(w).enumerate().for_each(|(cy, out)| {
        let r0 = 2 * cy * w;
        for cx in 0..w / 2 {
            out[2 * cx] = u[r0 + 2 * cx + 1];
            out[2 * cx + 1] = v[r0 + 2 * cx + 1];
        }
    });
    let main = Nv12Planes::new(pic.size, pic.y.clone(), main_uv)?;
    let aux = Nv12Planes::new(Size::new(pic.size.width, aux_height(pic.size.height)), aux_y, aux_uv)?;
    Some((main, aux))
}

/// The chroma filter threshold a client uses after lossy coding: the even row, even column
/// sample is rebuilt only where that changes it by more than this much; elsewhere the average
/// is kept, so smooth areas don't get the coding error of four samples (FreeRDP's choice).
pub const CHROMA_FILTER_THRESHOLD: u8 = 30;

/// [`split`] of [`bgra_to_yuv444`] in one pass over the BGRA rows, without the full 4:4:4
/// intermediate (what [`Avc444Encoder`] runs per frame). Same output; `None` for odd sizes or a
/// short buffer.
pub fn split_bgra(bgra: &[u8], size: Size<u32>, stride: usize) -> Option<(Nv12Planes, Nv12Planes)> {
    let (w, h) = dims(size)?;
    if !w.is_multiple_of(2) || !h.is_multiple_of(2) || w == 0 || h == 0 {
        return None;
    }
    if stride < w.checked_mul(4)? || bgra.len() < stride.checked_mul(h - 1)?.checked_add(w * 4)? {
        return None;
    }
    let ah = usize::try_from(aux_height(size.height)).ok()?;
    let mut main_y = vec![0u8; w * h];
    let mut main_uv = vec![0u8; w * h / 2];
    let mut aux_uv = vec![GREY; w * ah / 2];
    // Odd-row chroma (U row, V row) per row pair, scattered into the auxiliary luma afterwards.
    let mut odd_u = vec![0u8; w * h / 2];
    let mut odd_v = vec![0u8; w * h / 2];
    main_y
        .par_chunks_mut(2 * w)
        .zip(main_uv.par_chunks_mut(w))
        .zip(aux_uv[..w * h / 2].par_chunks_mut(w))
        .zip(odd_u.par_chunks_mut(w))
        .zip(odd_v.par_chunks_mut(w))
        .enumerate()
        .for_each(|(cy, ((((y2, muv), auv), ou), ov))| {
            let row0 = &bgra[2 * cy * stride..2 * cy * stride + w * 4];
            let row1 = &bgra[(2 * cy + 1) * stride..(2 * cy + 1) * stride + w * 4];
            let (y_even, y_odd) = y2.split_at_mut(w);
            for cx in 0..w / 2 {
                let mut su = 0u32;
                let mut sv = 0u32;
                for (dy, row) in [(0usize, row0), (1, row1)] {
                    for dx in 0..2 {
                        let x = 2 * cx + dx;
                        let px = &row[x * 4..x * 4 + 4];
                        let (b, g, r) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
                        let yy = ((54 * r + 183 * g + 18 * b) >> 8).clamp(0, 255) as u8;
                        let uu = (((-29 * r - 99 * g + 128 * b) >> 8) + 128).clamp(0, 255) as u8;
                        let vv = (((128 * r - 116 * g - 12 * b) >> 8) + 128).clamp(0, 255) as u8;
                        su += u32::from(uu);
                        sv += u32::from(vv);
                        if dy == 0 {
                            y_even[x] = yy;
                            if dx == 1 {
                                auv[2 * cx] = uu;
                                auv[2 * cx + 1] = vv;
                            }
                        } else {
                            y_odd[x] = yy;
                            ou[x] = uu;
                            ov[x] = vv;
                        }
                    }
                }
                muv[2 * cx] = ((su + 2) / 4) as u8;
                muv[2 * cx + 1] = ((sv + 2) / 4) as u8;
            }
        });
    let mut aux_y = vec![GREY; w * ah];
    aux_y.par_chunks_mut(w).enumerate().for_each(|(row, out)| {
        let (is_v, src) = aux_luma_source(row);
        if src < h {
            let pair = src / 2;
            let plane = if is_v { &odd_v } else { &odd_u };
            out.copy_from_slice(&plane[pair * w..(pair + 1) * w]);
        }
    });
    let main = Nv12Planes::new(size, main_y, main_uv)?;
    let aux = Nv12Planes::new(Size::new(size.width, aux_height(size.height)), aux_y, aux_uv)?;
    Some((main, aux))
}

/// Puts the main and auxiliary views back into a 4:4:4 picture (the client side of [`split`]),
/// rebuilding every even row, even column sample: exact up to the average's rounding.
/// `None` if the views do not fit together or are odd-sized.
pub fn combine(main: &Nv12Planes, aux: &Nv12Planes) -> Option<Yuv444> {
    combine_filtered(main, aux, 0)
}

/// [`combine`] for decoded (lossy) views: the even row, even column sample is rebuilt only
/// where it differs from the 2×2 average by more than `threshold`
/// ([`CHROMA_FILTER_THRESHOLD`]).
pub fn combine_filtered(main: &Nv12Planes, aux: &Nv12Planes, threshold: u8) -> Option<Yuv444> {
    let size = main.size();
    if aux.size() != Size::new(size.width, aux_height(size.height)) {
        return None;
    }
    let (w, h) = dims(size)?;
    if !w.is_multiple_of(2) || !h.is_multiple_of(2) {
        return None;
    }
    let (mut u, mut v) = (vec![0u8; w * h], vec![0u8; w * h]);
    // Odd rows: from the auxiliary luma.
    let ah = usize::try_from(aux_height(size.height)).ok()?;
    for row in 0..ah {
        let (is_v, src) = aux_luma_source(row);
        if src < h {
            let plane = if is_v { &mut v } else { &mut u };
            plane[src * w..(src + 1) * w].copy_from_slice(&aux.y()[row * w..(row + 1) * w]);
        }
    }
    // Even rows: odd columns from the auxiliary chroma, even columns rebuilt from the average.
    for cy in 0..h / 2 {
        let (r0, r1) = (2 * cy * w, (2 * cy + 1) * w);
        for cx in 0..w / 2 {
            for (plane, k) in [(&mut u, 0usize), (&mut v, 1)] {
                plane[r0 + 2 * cx + 1] = aux.uv()[cy * w + 2 * cx + k];
                let avg = i32::from(main.uv()[cy * w + 2 * cx + k]);
                let others = i32::from(plane[r0 + 2 * cx + 1])
                    + i32::from(plane[r1 + 2 * cx])
                    + i32::from(plane[r1 + 2 * cx + 1]);
                let rebuilt = (4 * avg - others).clamp(0, 255);
                let keep = (rebuilt - avg).abs() <= i32::from(threshold);
                plane[r0 + 2 * cx] = if keep { avg as u8 } else { rebuilt as u8 };
            }
        }
    }
    Yuv444::new(size, main.y().to_vec(), u, v)
}

/// One AVC444 picture: the main and auxiliary views of input frame `index`.
#[derive(Debug, Clone)]
pub struct Avc444Picture {
    /// Number of the input frame (0-based, in submission order).
    pub index: u64,
    /// The main (luma) view, `avc420EncodedBitstream1`.
    pub main: EncodedFrame,
    /// The auxiliary (chroma) view, `avc420EncodedBitstream2`.
    pub aux: EncodedFrame,
}

/// Two real-time encoders, one per view, fed from BGRA frames.
#[derive(Debug)]
pub struct Avc444Encoder {
    size: Size<u32>,
    main: VtEncoder,
    aux: VtEncoder,
    submitted: u64,
    emitted: u64,
    main_out: VecDeque<EncodedFrame>,
    aux_out: VecDeque<EncodedFrame>,
}

impl Avc444Encoder {
    /// Real-time encoders for `size` (even width and height) at `fps`. The auxiliary view gets
    /// the same bit rate as the main one: its luma holds chroma detail, which costs as much.
    pub fn new(size: Size<u32>, fps: u32) -> Result<Self, EncodeError> {
        if !size.width.is_multiple_of(2) || !size.height.is_multiple_of(2) {
            return Err(EncodeError::InvalidFrame(format!("odd size {}x{}", size.width, size.height)));
        }
        let config = EncoderConfig::realtime(size, fps);
        let aux_config = EncoderConfig { size: Size::new(size.width, aux_height(size.height)), ..config };
        Ok(Self {
            size,
            main: VtEncoder::new(config)?,
            aux: VtEncoder::new(aux_config)?,
            submitted: 0,
            emitted: 0,
            main_out: VecDeque::new(),
            aux_out: VecDeque::new(),
        })
    }

    /// Changes both views' average bit rate (EXT-4).
    pub fn set_average_bitrate(&mut self, bitrate: u32) -> Result<(), EncodeError> {
        self.main.set_average_bitrate(bitrate)?;
        self.aux.set_average_bitrate(bitrate)
    }

    /// Asks both views for a keyframe on the next frame.
    pub fn request_keyframe(&mut self) {
        self.main.request_keyframe();
        self.aux.request_keyframe();
    }

    /// Converts, splits and encodes one BGRA frame (`stride` bytes per row) captured at `at`.
    /// Returns every picture whose two views are both done.
    pub fn encode_bgra(
        &mut self,
        bgra: &[u8],
        stride: usize,
        at: Instant,
    ) -> Result<Vec<Avc444Picture>, EncodeError> {
        let (main, aux) = split_bgra(bgra, self.size, stride)
            .ok_or_else(|| EncodeError::InvalidFrame("BGRA buffer too short".into()))?;
        let main_buf = PixelBuffer::new_nv12(self.size)?;
        main_buf.write_nv12(&main)?;
        let aux_buf = PixelBuffer::new_nv12(aux.size())?;
        aux_buf.write_nv12(&aux)?;
        // Both views at once: with low-latency rate control an encode call returns only when
        // the frame is done, so one after the other would double the time per frame.
        let (main_enc, aux_enc) = (&mut self.main, &mut self.aux);
        let (main_frames, aux_frames) = std::thread::scope(|s| {
            let aux = s.spawn(|| aux_enc.encode(&aux_buf, at));
            let main = main_enc.encode(&main_buf, at);
            (main, aux.join())
        });
        let aux_frames =
            aux_frames.map_err(|_| EncodeError::InvalidFrame("auxiliary encoder panicked".into()))?;
        self.main_out.extend(main_frames?);
        self.aux_out.extend(aux_frames?);
        self.submitted += 1;
        Ok(self.pairs())
    }

    /// Emits every pending picture.
    pub fn flush(&mut self) -> Result<Vec<Avc444Picture>, EncodeError> {
        self.main_out.extend(self.main.flush()?);
        self.aux_out.extend(self.aux.flush()?);
        Ok(self.pairs())
    }

    /// Pictures submitted but not yet emitted.
    pub fn in_flight(&self) -> u64 {
        self.submitted - self.emitted
    }

    fn pairs(&mut self) -> Vec<Avc444Picture> {
        let mut out = Vec::new();
        while !self.main_out.is_empty() && !self.aux_out.is_empty() {
            let (Some(main), Some(aux)) = (self.main_out.pop_front(), self.aux_out.pop_front()) else {
                break;
            };
            out.push(Avc444Picture { index: self.emitted, main, aux });
            self.emitted += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aux_luma_rows_follow_the_band_layout() {
        assert_eq!(aux_luma_source(0), (false, 1));
        assert_eq!(aux_luma_source(7), (false, 15));
        assert_eq!(aux_luma_source(8), (true, 1));
        assert_eq!(aux_luma_source(15), (true, 15));
        assert_eq!(aux_luma_source(16), (false, 17));
        assert_eq!(aux_luma_source(24), (true, 17));
        assert_eq!(
            (aux_height(16), aux_height(20), aux_height(1080), aux_height(2160)),
            (16, 32, 1088, 2160)
        );
    }

    #[test]
    fn mismatched_views_do_not_combine() {
        let a = Nv12Planes::new(Size::new(4, 4), vec![0; 16], vec![0; 8]).unwrap();
        let b = Nv12Planes::new(Size::new(4, 2), vec![0; 8], vec![0; 4]).unwrap();
        assert!(combine(&a, &b).is_none());
    }
}
