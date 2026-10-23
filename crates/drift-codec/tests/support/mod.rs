//! Test support: DRFTGFX1 capture reader, a CPU reference compositor driven by drift-codec,
//! PNG loading and PSNR.
#![allow(dead_code, clippy::unwrap_used, clippy::cast_possible_truncation, clippy::cast_precision_loss)]

pub mod synth;

use std::collections::HashMap;
use std::path::PathBuf;

use drift_codec::{BgraTile, ClearCodec, ProgressiveCodec, TilePool, UncompressedFormat};
use drift_core::{Point, Rect, Size};
use ironrdp_core::{ReadCursor, decode_cursor};
use ironrdp_egfx::pdu::{Codec1Type, GfxPdu, PixelFormat};
use ironrdp_graphics::zgfx;

/// Magic at the start of a DRFTGFX1 capture (see `docs/adr/M1-4-cpu-codecs.md`).
pub const MAGIC: &[u8; 8] = b"DRFTGFX1";

/// `fixtures/` at the repository root.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// Reads a fixture, failing with a helpful message (e.g. un-fetched git-lfs pointers).
pub fn read_fixture(rel: &str) -> Vec<u8> {
    let path = fixtures_dir().join(rel);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
    assert!(
        !data.starts_with(b"version https://git-lfs"),
        "fixture {} is a git-lfs pointer: run `git lfs pull`",
        path.display()
    );
    data
}

/// Splits a DRFTGFX1 capture into its raw (ZGFX-compressed) DVC payloads.
pub fn gfx_payloads(capture: &[u8]) -> Vec<&[u8]> {
    assert_eq!(capture.get(..8), Some(&MAGIC[..]), "not a DRFTGFX1 capture");
    let mut rest = &capture[8..];
    let mut out = Vec::new();
    while !rest.is_empty() {
        let len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
        out.push(&rest[4..4 + len]);
        rest = &rest[4 + len..];
    }
    out
}

/// Decompresses every payload of a capture and decodes the GFX PDUs.
pub fn gfx_pdus(capture: &[u8]) -> Vec<GfxPdu> {
    let mut z = zgfx::Decompressor::new();
    let mut pdus = Vec::new();
    for payload in gfx_payloads(capture) {
        let mut buf = Vec::new();
        z.decompress(payload, &mut buf).unwrap();
        let mut cursor = ReadCursor::new(&buf);
        while !cursor.is_empty() {
            pdus.push(decode_cursor::<GfxPdu>(&mut cursor).unwrap());
        }
    }
    pdus
}

/// The progressive payloads (`surface_id`, `codec_context_id`, `bitmap_data`) of a capture.
pub fn progressive_payloads(capture: &[u8]) -> Vec<(u16, u32, Vec<u8>)> {
    gfx_pdus(capture)
        .into_iter()
        .filter_map(|p| match p {
            GfxPdu::WireToSurface2(w) => Some((w.surface_id, w.codec_context_id, w.bitmap_data)),
            _ => None,
        })
        .collect()
}

struct Surface {
    size: Size<u32>,
    px: Vec<u8>,
}

/// A minimal CPU GFX compositor driving drift-codec, used to replay captures.
pub struct Replay {
    progressive: ProgressiveCodec,
    clear: ClearCodec,
    surfaces: HashMap<u16, Surface>,
    mapped: HashMap<u16, Point<u32>>,
    cache: HashMap<u16, (Size<u32>, Vec<u8>)>,
    output: Size<u32>,
    /// Number of progressive tiles decoded.
    pub tiles: usize,
    /// Number of frames completed.
    pub frames: usize,
}

impl Replay {
    /// A replay using a pool of `threads` workers.
    pub fn new(threads: usize) -> Self {
        Self {
            progressive: ProgressiveCodec::new(TilePool::new(threads).unwrap()),
            clear: ClearCodec::new(),
            surfaces: HashMap::new(),
            mapped: HashMap::new(),
            cache: HashMap::new(),
            output: Size::new(0, 0),
            tiles: 0,
            frames: 0,
        }
    }

    fn blit(&mut self, id: u16, tile: &BgraTile) {
        if let Some(s) = self.surfaces.get_mut(&id) {
            tile.blit_into(s.size, &mut s.px);
        }
    }

    fn copy_out(&self, id: u16, r: Rect) -> Option<Vec<u8>> {
        let s = self.surfaces.get(&id)?;
        if !r.fits_within(s.size) {
            return None;
        }
        let mut out = Vec::with_capacity((r.width * r.height * 4) as usize);
        for y in r.y..r.bottom() {
            let o = ((y * s.size.width + r.x) * 4) as usize;
            out.extend_from_slice(&s.px[o..o + (r.width * 4) as usize]);
        }
        Some(out)
    }

    fn paste(&mut self, id: u16, at: Point<u32>, size: Size<u32>, data: Vec<u8>) {
        let tile = BgraTile {
            origin: at,
            size,
            data,
            update_rects: vec![Rect::new(at.x, at.y, size.width, size.height)],
        };
        self.blit(id, &tile);
    }

    /// Applies one GFX PDU.
    pub fn apply(&mut self, pdu: GfxPdu) {
        match pdu {
            GfxPdu::ResetGraphics(r) => {
                self.output = Size::new(r.width, r.height);
                self.surfaces.clear();
                self.mapped.clear();
                self.progressive.reset();
            }
            GfxPdu::CreateSurface(c) => {
                let size = Size::new(u32::from(c.width), u32::from(c.height));
                self.surfaces.insert(
                    c.surface_id,
                    Surface { size, px: vec![0; (size.width * size.height * 4) as usize] },
                );
            }
            GfxPdu::DeleteSurface(d) => {
                self.surfaces.remove(&d.surface_id);
                self.mapped.remove(&d.surface_id);
                self.progressive.delete_surface(d.surface_id);
            }
            GfxPdu::MapSurfaceToOutput(m) => {
                self.mapped.insert(m.surface_id, Point::new(m.output_origin_x, m.output_origin_y));
            }
            GfxPdu::StartFrame(_) => self.progressive.begin_frame(),
            GfxPdu::EndFrame(_) => {
                self.progressive.end_frame();
                self.frames += 1;
            }
            GfxPdu::DeleteEncodingContext(d) => {
                self.progressive.delete_context(d.surface_id, d.codec_context_id)
            }
            GfxPdu::SolidFill(f) => {
                for r in &f.rectangles {
                    let Some(r) = rect(r) else { continue };
                    let px = [f.fill_pixel.b, f.fill_pixel.g, f.fill_pixel.r, 0xFF];
                    let data = px.repeat((r.width * r.height) as usize);
                    self.paste(f.surface_id, Point::new(r.x, r.y), r.size(), data);
                }
            }
            GfxPdu::SurfaceToSurface(s) => {
                let Some(src) = rect(&s.source_rectangle) else { return };
                let Some(data) = self.copy_out(s.source_surface_id, src) else { return };
                for p in &s.destination_points {
                    let at = Point::new(u32::from(p.x), u32::from(p.y));
                    self.paste(s.destination_surface_id, at, src.size(), data.clone());
                }
            }
            GfxPdu::SurfaceToCache(s) => {
                let Some(src) = rect(&s.source_rectangle) else { return };
                if let Some(data) = self.copy_out(s.surface_id, src) {
                    self.cache.insert(s.cache_slot, (src.size(), data));
                }
            }
            GfxPdu::CacheToSurface(c) => {
                let Some((size, data)) = self.cache.get(&c.cache_slot).cloned() else { return };
                for p in &c.destination_points {
                    self.paste(c.surface_id, Point::new(u32::from(p.x), u32::from(p.y)), size, data.clone());
                }
            }
            GfxPdu::EvictCacheEntry(e) => {
                self.cache.remove(&e.cache_slot);
            }
            GfxPdu::WireToSurface1(w) => {
                let Some(dest) = rect(&w.destination_rectangle) else { return };
                let tile = match w.codec_id {
                    Codec1Type::Uncompressed => {
                        let fmt = match w.pixel_format {
                            PixelFormat::XRgb => UncompressedFormat::Xrgb,
                            PixelFormat::ARgb => UncompressedFormat::Argb,
                        };
                        drift_codec::decode_uncompressed(dest, fmt, &w.bitmap_data)
                    }
                    Codec1Type::Planar => drift_codec::decode_planar(dest, &w.bitmap_data),
                    Codec1Type::ClearCodec => self.clear.decode(dest, &w.bitmap_data),
                    other => panic!("unexpected codec {other:?} in a v81noavc capture"),
                };
                self.blit(w.surface_id, &tile.unwrap());
            }
            GfxPdu::WireToSurface2(w) => {
                let size = self.surfaces.get(&w.surface_id).map(|s| s.size).unwrap();
                let tiles =
                    self.progressive.decode(w.surface_id, w.codec_context_id, size, &w.bitmap_data).unwrap();
                self.tiles += tiles.len();
                for t in &tiles {
                    self.blit(w.surface_id, t);
                }
            }
            _ => {}
        }
    }

    /// The output (all mapped surfaces composited at their origins) as BGRA.
    pub fn output_bgra(&self) -> (Size<u32>, Vec<u8>) {
        let mut out = vec![0u8; (self.output.width * self.output.height * 4) as usize];
        for (id, origin) in &self.mapped {
            let Some(s) = self.surfaces.get(id) else { continue };
            let tile = BgraTile {
                origin: *origin,
                size: s.size,
                data: s.px.clone(),
                update_rects: vec![Rect::new(origin.x, origin.y, s.size.width, s.size.height)],
            };
            tile.blit_into(self.output, &mut out);
        }
        (self.output, out)
    }
}

fn rect(r: &ironrdp_pdu::geometry::ExclusiveRectangle) -> Option<Rect> {
    Rect::from_ltrb(u32::from(r.left), u32::from(r.top), u32::from(r.right), u32::from(r.bottom))
        .filter(|r| !r.is_empty())
}

/// Replays a DRFTGFX1 capture and returns the final output image (BGRA).
pub fn replay_capture(capture: &[u8], threads: usize) -> (Replay, Size<u32>, Vec<u8>) {
    let mut replay = Replay::new(threads);
    for pdu in gfx_pdus(capture) {
        replay.apply(pdu);
    }
    let (size, px) = replay.output_bgra();
    (replay, size, px)
}

/// Loads a PNG as RGBA8.
pub fn load_png_rgba(bytes: &[u8]) -> (Size<u32>, Vec<u8>) {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    assert_eq!(info.color_type, png::ColorType::Rgba, "golden must decode to RGBA");
    (Size::new(info.width, info.height), buf)
}

/// PSNR (dB) over the R, G and B channels of a BGRA image against an RGBA image.
/// Identical images return `f64::INFINITY`.
pub fn psnr_bgra_vs_rgba(bgra: &[u8], rgba: &[u8]) -> f64 {
    assert_eq!(bgra.len(), rgba.len(), "image sizes differ");
    let mut sse = 0f64;
    for (a, b) in bgra.as_chunks::<4>().0.iter().zip(rgba.as_chunks::<4>().0) {
        for (x, y) in [(a[2], b[0]), (a[1], b[1]), (a[0], b[2])] {
            let d = f64::from(x) - f64::from(y);
            sse += d * d;
        }
    }
    let n = (bgra.len() / 4 * 3) as f64;
    if sse == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 / (sse / n)).log10() }
}

/// A g-r-d-shaped one-tile stream built from the most expensive (largest) real tile of the
/// greeter capture, placed at tile (0, 0): the input of the 64×64 tile budget.
pub fn real_tile_stream() -> Vec<u8> {
    use ironrdp_pdu::codecs::rfx::RfxRectangle;
    use ironrdp_pdu::codecs::rfx::progressive::{
        ProgressiveBlock, ProgressiveContextPdu, ProgressiveFrameBeginPdu, ProgressiveFrameEndPdu,
        ProgressiveRegion, ProgressiveSyncPdu, ProgressiveTile, decode_progressive_stream,
        encode_progressive_stream,
    };

    let capture = read_fixture("gfx/greeter_v81noavc.gfx");
    let payloads = progressive_payloads(&capture);
    let mut best: Option<(usize, ProgressiveTile<'_>, Vec<_>)> = None;
    for (_, _, data) in &payloads {
        for block in decode_progressive_stream(data).unwrap() {
            let ProgressiveBlock::Region(region) = block else { continue };
            for tile in region.tiles {
                let ProgressiveTile::Simple(s) = &tile else { continue };
                let size = s.y_data.len() + s.cb_data.len() + s.cr_data.len();
                if best.as_ref().is_none_or(|(b, _, _)| size > *b) {
                    best = Some((size, tile.clone(), region.quant_vals.clone()));
                }
            }
        }
    }
    let (_, mut tile, quant_vals) = best.expect("greeter capture has simple tiles");
    if let ProgressiveTile::Simple(s) = &mut tile {
        s.x_idx = 0;
        s.y_idx = 0;
    }
    let region = ProgressiveRegion {
        tile_size: 0x40,
        rects: vec![RfxRectangle { x: 0, y: 0, width: 64, height: 64 }],
        quant_vals,
        quant_prog_vals: vec![],
        flags: 0,
        tiles: vec![tile],
    };
    encode_progressive_stream(&[
        ProgressiveBlock::Sync(ProgressiveSyncPdu),
        ProgressiveBlock::Context(ProgressiveContextPdu { context_id: 0, tile_size: 0x40, flags: 0 }),
        ProgressiveBlock::FrameBegin(ProgressiveFrameBeginPdu { frame_index: 0, region_count: 1 }),
        ProgressiveBlock::Region(region),
        ProgressiveBlock::FrameEnd(ProgressiveFrameEndPdu),
    ])
    .unwrap()
}
