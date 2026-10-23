//! RFX Progressive ([MS-RDPEGFX] 2.2.4.2) decoded on the rayon [`TilePool`].
//!
//! The decoder mirrors `ironrdp_graphics::progressive::ProgressiveDecoder` step for step
//! (same validation order, same state transitions, same REGION clipping) but splits each
//! REGION into independent per-tile jobs that run in parallel. Tiles repeated within one
//! REGION are decoded in successive rounds so every tile still sees its own previous pass.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use drift_core::{Point, Rect, Size};
use ironrdp_graphics::progressive::{COEFFICIENTS_PER_COMPONENT, MAX_SURFACE_DIM, SurfaceTiles, TileState};
use ironrdp_graphics::rectangle_processing::Region;
use ironrdp_pdu::codecs::rfx::progressive::{
    ComponentCodecQuant, ProgressiveBlock, ProgressiveCodecQuant, ProgressiveRegion, ProgressiveTile,
    TILE_FLAG_DIFFERENCE, decode_progressive_stream,
};
use ironrdp_pdu::geometry::InclusiveRectangle;
use rayon::prelude::*;

use crate::error::{CodecError, CodecKind};
use crate::pool::TilePool;
use crate::tile::{BPP, BgraTile};

/// Edge length of an RFX Progressive tile in pixels.
pub const TILE_SIZE: u32 = 64;

/// Tile edge as used by the 16-bit wire coordinates.
const TILE_DIM: u16 = 64;

/// Bytes in one decoded 64×64 BGRA tile.
const TILE_BYTES: usize = 64 * 64 * BPP;

/// Same bound as IronRDP: aggregate rectangle visits while clipping one payload, which
/// caps the CPU a hostile REGION with many rectangles can burn.
const MAX_REGION_CLIPPING_WORK: usize = 1 << 20;

/// Full-quality progressive quality byte ([MS-RDPEGFX] 2.2.4.2.1.5.2).
const FULL_QUALITY: u8 = 0xFF;

/// Retained DWT coefficients of a tile (Y, Cb, Cr), the reference for difference tiles.
type Coeffs = [[i16; COEFFICIENTS_PER_COMPONENT]; 3];

/// Tile coordinates `(x_idx, y_idx)`.
type TileKey = (u16, u16);

/// Retained references keyed by `(surface_id, x_idx, y_idx)`.
type References = BTreeMap<(u16, u16, u16), Box<Coeffs>>;

fn malformed(detail: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(CodecKind::Progressive, detail)
}

/// A stateful RFX Progressive decoder for one session.
///
/// It mirrors `ironrdp_graphics::progressive::ProgressiveDecoder` (per
/// `(surface, codec context)` tile state, surface-scoped DWT references for difference
/// tiles, REGION clipping across the payloads of one RDPGFX frame) and produces
/// bit-identical pixels, but decodes the tiles of each REGION in parallel on a
/// [`TilePool`].
pub struct ProgressiveCodec {
    pool: TilePool,
    /// Tile grids per `(surface_id, codec_context_id)`.
    contexts: BTreeMap<(u16, u32), SurfaceTiles>,
    /// Latest coefficients per tile, for difference tiles.
    references: References,
    /// Tiles decoded so far in the current RDPGFX frame, per context.
    frame_tiles: BTreeMap<(u16, u32), BTreeSet<TileKey>>,
    frame_active: bool,
    /// Last reduce-extrapolate flag a CONTEXT block announced, per surface.
    surface_context_flags: BTreeMap<u16, bool>,
}

impl std::fmt::Debug for ProgressiveCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressiveCodec")
            .field("pool", &self.pool)
            .field("contexts", &self.contexts.len())
            .field("frame_active", &self.frame_active)
            .finish_non_exhaustive()
    }
}

impl ProgressiveCodec {
    /// Creates a decoder that runs tile work on `pool`.
    pub fn new(pool: TilePool) -> Self {
        Self {
            pool,
            contexts: BTreeMap::new(),
            references: BTreeMap::new(),
            frame_tiles: BTreeMap::new(),
            frame_active: false,
            surface_context_flags: BTreeMap::new(),
        }
    }

    /// Marks the start of an RDPGFX frame (`StartFrame`): REGION blocks in later payloads of
    /// the same frame may reference tiles decoded by earlier payloads.
    pub fn begin_frame(&mut self) {
        self.frame_tiles.clear();
        self.frame_active = true;
    }

    /// Marks the end of an RDPGFX frame (`EndFrame`).
    pub fn end_frame(&mut self) {
        self.frame_tiles.clear();
        self.frame_active = false;
    }

    /// Decodes one `WireToSurface2` progressive payload for `surface_id` /
    /// `codec_context_id`, on a surface of `surface` pixels.
    ///
    /// Returns one 64×64 [`BgraTile`] per tile that has visible pixels in this payload's
    /// REGION rectangles; `update_rects` are clipped to the REGION and the surface.
    ///
    /// # Errors
    /// [`CodecError::Malformed`] for any stream, tile or entropy-coding error, and
    /// [`CodecError::SurfaceTooLarge`] for surfaces beyond 32768 px per axis.
    pub fn decode(
        &mut self,
        surface_id: u16,
        codec_context_id: u32,
        surface: Size<u32>,
        data: &[u8],
    ) -> Result<Vec<BgraTile>, CodecError> {
        let too_large = || CodecError::SurfaceTooLarge {
            codec: CodecKind::Progressive,
            width: surface.width,
            height: surface.height,
        };
        let width =
            u16::try_from(surface.width).ok().filter(|w| *w <= MAX_SURFACE_DIM).ok_or_else(too_large)?;
        let height =
            u16::try_from(surface.height).ok().filter(|h| *h <= MAX_SURFACE_DIM).ok_or_else(too_large)?;

        let blocks = decode_progressive_stream(data).map_err(malformed)?;

        // SYNC + CONTEXT are sent once per context; g-r-d omits CONTEXT afterwards and
        // Windows opens new context ids without repeating it (see IronRDP's decoder).
        let signalled = blocks.iter().find_map(|block| match block {
            ProgressiveBlock::Context(ctx) => Some(ctx.uses_reduce_extrapolate()),
            _ => None,
        });
        if let Some(flag) = signalled {
            self.surface_context_flags.insert(surface_id, flag);
        }
        let use_reduce_extrapolate = signalled
            .or_else(|| self.contexts.get(&(surface_id, codec_context_id)).map(|c| c.use_reduce_extrapolate))
            .or_else(|| self.surface_context_flags.get(&surface_id).copied())
            .ok_or_else(|| malformed("progressive stream missing CONTEXT block"))?;

        if !self.frame_active {
            self.frame_tiles.clear();
        }

        let Self { pool, contexts, references, frame_tiles, .. } = self;
        let grid = match contexts.entry((surface_id, codec_context_id)) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(new_grid(width, height, use_reduce_extrapolate)?),
        };
        let resized =
            grid.tiles_wide != width.div_ceil(TILE_DIM) || grid.tiles_high != height.div_ceil(TILE_DIM);
        if resized {
            *grid = new_grid(width, height, use_reduce_extrapolate)?;
        }
        grid.use_reduce_extrapolate = use_reduce_extrapolate;

        let frame_tiles = frame_tiles.entry((surface_id, codec_context_id)).or_default();
        if resized {
            frame_tiles.clear();
        }

        let mut out = Vec::new();
        let mut clipping_work = 0usize;
        let mut in_frame = false;
        let mut frame_ended = false;
        for block in &blocks {
            let region = match block {
                ProgressiveBlock::FrameBegin(_) if !frame_ended => {
                    in_frame = true;
                    continue;
                }
                ProgressiveBlock::FrameEnd(_) => {
                    in_frame = false;
                    frame_ended = true;
                    continue;
                }
                ProgressiveBlock::Region(r) if in_frame => r,
                _ => continue,
            };

            let ctx = RegionCtx { surface_id, use_reduce_extrapolate, region };
            let mut region_tiles = decode_region_tiles(pool, grid, references, &ctx)?;
            frame_tiles.extend(region_tiles.keys().copied());

            let clipping = clipping_region(region, width, height, &mut clipping_work)?;
            let mut pending = Vec::new();
            for &(x_idx, y_idx) in frame_tiles.iter() {
                charge(&mut clipping_work, clipping.rectangles.len().max(1))?;
                let rects = visible_rects(&clipping, x_idx, y_idx, width, height);
                if rects.is_empty() {
                    continue;
                }
                let origin = Point::new(u32::from(x_idx) * TILE_SIZE, u32::from(y_idx) * TILE_SIZE);
                let data = match region_tiles.remove(&(x_idx, y_idx)) {
                    Some(px) => px,
                    None if grid_get(grid, x_idx, y_idx).is_some() => {
                        pending.push((out.len(), (x_idx, y_idx)));
                        Vec::new()
                    }
                    None => continue,
                };
                out.push(BgraTile {
                    origin,
                    size: Size::new(TILE_SIZE, TILE_SIZE),
                    data,
                    update_rects: rects,
                });
            }
            // Tiles visible in this REGION but decoded by an earlier payload of the frame are
            // re-rendered from their retained coefficients.
            let grid_ref: &SurfaceTiles = grid;
            let rendered = run_parallel(pool, &mut pending, |(_, (x, y))| {
                grid_get(grid_ref, *x, *y).map(render_bgra).unwrap_or_default()
            });
            for ((i, _), px) in pending.iter().zip(rendered) {
                if let Some(tile) = out.get_mut(*i) {
                    tile.data = px;
                }
            }
        }

        if !self.frame_active {
            self.frame_tiles.clear();
        }
        Ok(out)
    }

    /// Drops a codec context (`DeleteEncodingContext`).
    pub fn delete_context(&mut self, surface_id: u16, codec_context_id: u32) {
        self.contexts.remove(&(surface_id, codec_context_id));
        self.frame_tiles.remove(&(surface_id, codec_context_id));
    }

    /// Drops every context and reference of a surface (`DeleteSurface`), so a new surface
    /// reusing the id cannot inherit stale tiles.
    pub fn delete_surface(&mut self, surface_id: u16) {
        self.contexts.retain(|(s, _), _| *s != surface_id);
        self.references.retain(|(s, _, _), _| *s != surface_id);
        self.frame_tiles.retain(|(s, _), _| *s != surface_id);
        self.surface_context_flags.remove(&surface_id);
    }

    /// Drops all codec contexts (`ResetGraphics`), keeping surface references like IronRDP.
    pub fn reset(&mut self) {
        self.contexts.clear();
        self.frame_tiles.clear();
        self.frame_active = false;
    }
}

fn new_grid(width: u16, height: u16, use_reduce_extrapolate: bool) -> Result<SurfaceTiles, CodecError> {
    SurfaceTiles::new(width, height, use_reduce_extrapolate).map_err(|_| CodecError::SurfaceTooLarge {
        codec: CodecKind::Progressive,
        width: u32::from(width),
        height: u32::from(height),
    })
}

/// Index of a tile in the grid, or `None` when out of bounds.
fn grid_index(grid: &SurfaceTiles, x_idx: u16, y_idx: u16) -> Option<usize> {
    (x_idx < grid.tiles_wide && y_idx < grid.tiles_high)
        .then(|| usize::from(y_idx) * usize::from(grid.tiles_wide) + usize::from(x_idx))
}

fn grid_get(grid: &SurfaceTiles, x_idx: u16, y_idx: u16) -> Option<&TileState> {
    grid.tiles.get(grid_index(grid, x_idx, y_idx)?)?.as_deref()
}

fn charge(used: &mut usize, units: usize) -> Result<(), CodecError> {
    match used.checked_add(units) {
        Some(total) if total <= MAX_REGION_CLIPPING_WORK => {
            *used = total;
            Ok(())
        }
        _ => Err(malformed("progressive REGION clipping work limit exceeded")),
    }
}

/// Union of a REGION's rectangles clipped to the surface.
fn clipping_region(
    region: &ProgressiveRegion<'_>,
    width: u16,
    height: u16,
    work: &mut usize,
) -> Result<Region, CodecError> {
    let mut clipping = Region::new();
    for r in &region.rects {
        let left = r.x.min(width);
        let top = r.y.min(height);
        let right = r.x.saturating_add(r.width).min(width);
        let bottom = r.y.saturating_add(r.height).min(height);
        if left < right && top < bottom {
            charge(work, clipping.rectangles.len().saturating_add(1))?;
            clipping.union_rectangle(InclusiveRectangle { left, top, right: right - 1, bottom: bottom - 1 });
        }
    }
    Ok(clipping)
}

/// Parts of tile `(x_idx, y_idx)` inside the clipping region, as surface rectangles.
fn visible_rects(clipping: &Region, x_idx: u16, y_idx: u16, width: u16, height: u16) -> Vec<Rect> {
    let left = x_idx.saturating_mul(TILE_DIM);
    let top = y_idx.saturating_mul(TILE_DIM);
    let right = left.saturating_add(TILE_DIM).min(width);
    let bottom = top.saturating_add(TILE_DIM).min(height);
    if left >= right || top >= bottom {
        return Vec::new();
    }
    clipping
        .intersect_rectangle(&InclusiveRectangle { left, top, right: right - 1, bottom: bottom - 1 })
        .rectangles
        .into_iter()
        .map(|r| {
            Rect::new(
                u32::from(r.left),
                u32::from(r.top),
                u32::from(r.right) + 1 - u32::from(r.left),
                u32::from(r.bottom) + 1 - u32::from(r.top),
            )
        })
        .collect()
}

/// Reconstructs a tile's pixels and swizzles IronRDP's RGBA to BGRA in place.
fn render_bgra(state: &TileState) -> Vec<u8> {
    let mut px = vec![0u8; TILE_BYTES];
    state.reconstruct_to_rgba(&mut px);
    for p in px.as_chunks_mut::<BPP>().0 {
        p.swap(0, 2);
    }
    px
}

/// Runs `f` over `items`, in parallel on `pool` when there is more than one item.
fn run_parallel<T: Send, R: Send>(
    pool: &TilePool,
    items: &mut [T],
    f: impl Fn(&mut T) -> R + Sync + Send,
) -> Vec<R> {
    match items {
        [] => Vec::new(),
        [one] => vec![f(one)],
        many => pool.install(|| many.par_iter_mut().map(&f).collect()),
    }
}

/// The REGION being decoded plus what its tiles need to know.
struct RegionCtx<'r, 'a> {
    surface_id: u16,
    use_reduce_extrapolate: bool,
    region: &'r ProgressiveRegion<'a>,
}

/// The entropy-coded work for one tile.
enum Pass<'a> {
    First {
        data: [&'a [u8]; 3],
        base: [ComponentCodecQuant; 3],
        prog: [ComponentCodecQuant; 3],
        quant_idx: [u8; 3],
        quality: u8,
        difference: bool,
    },
    Upgrade {
        srl: [&'a [u8]; 3],
        raw: [&'a [u8]; 3],
        prog: [ComponentCodecQuant; 3],
        quality: u8,
    },
}

/// One tile decode, owning the tile state (taken out of the grid) while it runs.
struct Job<'a> {
    key: TileKey,
    index: usize,
    state: Box<TileState>,
    reference: Option<Box<Coeffs>>,
    pass: Pass<'a>,
}

/// Decodes every tile of a REGION. Returns the BGRA pixels of each decoded tile.
fn decode_region_tiles(
    pool: &TilePool,
    grid: &mut SurfaceTiles,
    references: &mut References,
    ctx: &RegionCtx<'_, '_>,
) -> Result<BTreeMap<TileKey, Vec<u8>>, CodecError> {
    // Round k holds the k-th occurrence of each tile, so repeated tiles keep their order.
    let mut rounds: Vec<Vec<&ProgressiveTile<'_>>> = Vec::new();
    let mut seen: HashMap<TileKey, usize> = HashMap::new();
    for tile in &ctx.region.tiles {
        let n = seen.entry((tile.x_idx(), tile.y_idx())).or_insert(0);
        if rounds.len() <= *n {
            rounds.push(Vec::new());
        }
        if let Some(round) = rounds.get_mut(*n) {
            round.push(tile);
        }
        *n += 1;
    }

    let mut decoded = BTreeMap::new();
    for round in rounds {
        let mut jobs = Vec::with_capacity(round.len());
        let mut failure = None;
        for tile in round {
            match prepare(grid, references, ctx, tile) {
                Ok(Some(job)) => jobs.push(job),
                Ok(None) => {}
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        let results = if failure.is_none() {
            run_parallel(pool, &mut jobs, |job| run(job, ctx.use_reduce_extrapolate))
        } else {
            Vec::new()
        };
        // Return every tile (and its reference) to the shared state, even on failure.
        for job in jobs {
            if let Some(slot) = grid.tiles.get_mut(job.index) {
                *slot = Some(job.state);
            }
            if let Some(reference) = job.reference {
                references.insert((ctx.surface_id, job.key.0, job.key.1), reference);
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        for result in results {
            let (key, px) = result?;
            decoded.insert(key, px);
        }
    }
    Ok(decoded)
}

/// Validates a tile block exactly like IronRDP (same checks, same order, same side effects)
/// and turns it into a job. `Ok(None)` means the block is legitimately skipped.
fn prepare<'a>(
    grid: &mut SurfaceTiles,
    references: &mut References,
    ctx: &RegionCtx<'_, 'a>,
    tile: &ProgressiveTile<'a>,
) -> Result<Option<Job<'a>>, CodecError> {
    let key = (tile.x_idx(), tile.y_idx());
    let out_of_bounds = || malformed(format!("tile ({}, {}) out of surface bounds", key.0, key.1));
    let index = grid_index(grid, key.0, key.1).ok_or_else(out_of_bounds)?;
    let ref_key = (ctx.surface_id, key.0, key.1);

    let pass = match tile {
        ProgressiveTile::Simple(t) => first_pass(
            grid,
            references,
            ctx,
            index,
            FirstBlock {
                flags: t.flags,
                quant_idx: [t.quant_idx_y, t.quant_idx_cb, t.quant_idx_cr],
                data: [t.y_data, t.cb_data, t.cr_data],
                quality: None,
            },
        )?,
        ProgressiveTile::First(t) => first_pass(
            grid,
            references,
            ctx,
            index,
            FirstBlock {
                flags: t.flags,
                quant_idx: [t.quant_idx_y, t.quant_idx_cb, t.quant_idx_cr],
                data: [t.y_data, t.cb_data, t.cr_data],
                quality: Some(t.quality),
            },
        )?,
        ProgressiveTile::Upgrade(t) => {
            ensure_tile(grid, index);
            // An upgrade for a tile that never had a first pass is skipped.
            if grid_get(grid, key.0, key.1).is_none_or(|s| s.pass == 0) {
                return Ok(None);
            }
            let pq = progressive_quant(t.quality, &ctx.region.quant_prog_vals)?;
            Pass::Upgrade {
                srl: [t.y_srl_data, t.cb_srl_data, t.cr_srl_data],
                raw: [t.y_raw_data, t.cb_raw_data, t.cr_raw_data],
                prog: [pq.y_quant, pq.cb_quant, pq.cr_quant],
                quality: t.quality,
            }
        }
    };

    let state = grid.tiles.get_mut(index).and_then(Option::take).ok_or_else(out_of_bounds)?;
    let reference = references.remove(&ref_key);
    Ok(Some(Job { key, index, state, reference, pass }))
}

/// The fields TILE_SIMPLE and TILE_FIRST share.
struct FirstBlock<'a> {
    flags: u8,
    quant_idx: [u8; 3],
    data: [&'a [u8]; 3],
    /// `None` for TILE_SIMPLE (lossless progressive quant, full quality).
    quality: Option<u8>,
}

fn first_pass<'a>(
    grid: &mut SurfaceTiles,
    references: &References,
    ctx: &RegionCtx<'_, 'a>,
    index: usize,
    block: FirstBlock<'a>,
) -> Result<Pass<'a>, CodecError> {
    let (x_idx, y_idx) = (index % usize::from(grid.tiles_wide), index / usize::from(grid.tiles_wide));
    let difference = block.flags & TILE_FLAG_DIFFERENCE != 0;
    let has_reference = u16::try_from(x_idx)
        .ok()
        .zip(u16::try_from(y_idx).ok())
        .is_some_and(|(x, y)| references.contains_key(&(ctx.surface_id, x, y)));
    if difference && !has_reference {
        return Err(malformed(format!("difference tile ({x_idx}, {y_idx}) has no retained reference")));
    }
    ensure_tile(grid, index);
    let quant_vals = &ctx.region.quant_vals;
    let [Some(y), Some(cb), Some(cr)] = block.quant_idx.map(|i| quant_vals.get(usize::from(i)).copied())
    else {
        let worst = block.quant_idx.iter().max().copied().unwrap_or_default();
        return Err(malformed(format!("quant index {worst} exceeds table length {}", quant_vals.len())));
    };
    let (prog, quality) = match block.quality {
        None => ([ComponentCodecQuant::LOSSLESS; 3], FULL_QUALITY),
        Some(q) => {
            let pq = progressive_quant(q, &ctx.region.quant_prog_vals)?;
            ([pq.y_quant, pq.cb_quant, pq.cr_quant], q)
        }
    };
    Ok(Pass::First {
        data: block.data,
        base: [y, cb, cr],
        prog,
        quant_idx: block.quant_idx,
        quality,
        difference,
    })
}

/// Allocates the tile at `index` if needed (IronRDP's `get_or_create`).
fn ensure_tile(grid: &mut SurfaceTiles, index: usize) {
    let use_reduce_extrapolate = grid.use_reduce_extrapolate;
    if let Some(slot) = grid.tiles.get_mut(index) {
        slot.get_or_insert_with(|| {
            let mut t = Box::new(TileState::new());
            t.use_reduce_extrapolate = use_reduce_extrapolate;
            t
        });
    }
}

/// The progressive quant for a `quality` byte: 0xFF is full quality, anything else indexes
/// the REGION's table ([MS-RDPEGFX] 2.2.4.2.1.5.2).
fn progressive_quant(
    quality: u8,
    table: &[ProgressiveCodecQuant],
) -> Result<ProgressiveCodecQuant, CodecError> {
    if quality == FULL_QUALITY {
        return Ok(ProgressiveCodecQuant {
            quality,
            y_quant: ComponentCodecQuant::LOSSLESS,
            cb_quant: ComponentCodecQuant::LOSSLESS,
            cr_quant: ComponentCodecQuant::LOSSLESS,
        });
    }
    table
        .get(usize::from(quality))
        .copied()
        .ok_or_else(|| malformed(format!("quant index {quality} exceeds table length {}", table.len())))
}

/// Runs one tile job: entropy decode, reference update, reconstruction to BGRA.
fn run(job: &mut Job<'_>, use_reduce_extrapolate: bool) -> Result<(TileKey, Vec<u8>), CodecError> {
    let state = &mut *job.state;
    match job.pass {
        Pass::First { data, base, prog, quant_idx, quality, difference } => {
            state
                .decode_first(
                    data,
                    [&base[0], &base[1], &base[2]],
                    prog,
                    quant_idx,
                    quality,
                    use_reduce_extrapolate,
                )
                .map_err(|e| malformed(format!("progressive RLGR decode: {e}")))?;
            if difference {
                let reference = job
                    .reference
                    .as_deref()
                    .ok_or_else(|| malformed("difference tile lost its reference"))?;
                for (component, reference) in state.coefficients.iter_mut().zip(reference) {
                    for (c, r) in component.iter_mut().zip(reference) {
                        *c = c.saturating_add(*r);
                    }
                }
                state.is_difference = true;
            }
        }
        Pass::Upgrade { srl, raw, prog, quality } => {
            state
                .decode_upgrade(srl, raw, prog, quality)
                .map_err(|e| malformed(format!("progressive srl decode: {e}")))?;
        }
    }
    match &mut job.reference {
        Some(r) => **r = state.coefficients,
        None => job.reference = Some(Box::new(state.coefficients)),
    }
    Ok((job.key, render_bgra(state)))
}

#[cfg(test)]
#[path = "../tests/support/synth.rs"]
mod synth;

#[cfg(test)]
mod tests {
    use drift_core::{Point, Rect};
    use ironrdp_graphics::progressive::ProgressiveDecoder;
    use ironrdp_pdu::codecs::rfx::progressive::{ComponentCodecQuant, TILE_FLAG_DIFFERENCE};

    use super::synth::*;
    use super::*;

    const SURFACE: Size<u32> = Size::new(200, 130);

    fn pool() -> TilePool {
        TilePool::new(4).unwrap_or_else(|e| panic!("pool: {e}"))
    }

    /// Runs IronRDP's sequential decoder and converts its output to BGRA tiles.
    fn reference(
        dec: &mut ProgressiveDecoder,
        surface_id: u16,
        ctx: u32,
        size: Size<u32>,
        data: &[u8],
    ) -> Option<Vec<BgraTile>> {
        let tiles = dec.decode_bitmap(surface_id, ctx, size.width as u16, size.height as u16, data).ok()?;
        Some(
            tiles
                .into_iter()
                .map(|t| BgraTile {
                    origin: Point::new(u32::from(t.x_idx) * 64, u32::from(t.y_idx) * 64),
                    size: Size::new(64, 64),
                    data: t.pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect(),
                    update_rects: t
                        .update_rectangles
                        .iter()
                        .filter_map(|r| {
                            Rect::from_ltrb(
                                u32::from(r.left),
                                u32::from(r.top),
                                u32::from(r.right),
                                u32::from(r.bottom),
                            )
                        })
                        .collect(),
                })
                .collect(),
        )
    }

    /// Feeds the same calls to both decoders and asserts identical results.
    fn assert_same(calls: &[(u16, u32, &[u8])]) -> Vec<Option<Vec<BgraTile>>> {
        let mut ours = ProgressiveCodec::new(pool());
        let mut theirs = ProgressiveDecoder::new();
        let mut outs = Vec::new();
        for (i, (surface, ctx, data)) in calls.iter().enumerate() {
            let a = ours.decode(*surface, *ctx, SURFACE, data).ok();
            let b = reference(&mut theirs, *surface, *ctx, SURFACE, data);
            assert_eq!(a.is_some(), b.is_some(), "call {i}: success differs");
            assert_eq!(a, b, "call {i}: output differs");
            outs.push(a);
        }
        outs
    }

    #[test]
    fn simple_tiles_match_ironrdp_and_clip_to_region_and_surface() {
        let tiles: Vec<_> = [(0, 0, 1), (1, 0, 2), (3, 2, 3)]
            .iter()
            .map(|&(x, y, s)| encode_tile(x, y, &pattern(s), &GRD_QUANT, &ComponentCodecQuant::LOSSLESS))
            .collect();
        let data = stream(
            true,
            vec![region(
                vec![rect(0, 0, 100, 64), rect(192, 128, 64, 64)],
                vec![],
                tiles.iter().map(|t| simple(t, 0)).collect(),
            )],
        );
        let out = assert_same(&[(1, 0, &data)]);
        let out = out[0].as_deref().unwrap_or_default();
        assert_eq!(out.len(), 3);
        assert_eq!(out[1].update_rects, vec![Rect::new(64, 0, 36, 64)], "clipped to the REGION rect");
        assert_eq!(out[2].update_rects, vec![Rect::new(192, 128, 8, 2)], "clipped to the 200x130 surface");
    }

    #[test]
    fn first_upgrade_and_difference_tiles_match_ironrdp() {
        let t0 = encode_tile(0, 0, &pattern(7), &GRD_QUANT, &COARSE);
        let t1 = encode_tile(1, 1, &pattern(8), &GRD_QUANT, &ComponentCodecQuant::LOSSLESS);
        let full = vec![rect(0, 0, 200, 130)];
        let first_pass = stream(
            true,
            vec![region(full.clone(), vec![prog_quant(0, COARSE)], vec![first(&t0, 0), simple(&t1, 0)])],
        );
        // Upgrade (0,0); difference-tile (1,1) against its retained reference; upgrade a
        // never-decoded tile (2,1), which IronRDP silently skips.
        let second = stream(
            false,
            vec![region(
                full.clone(),
                vec![prog_quant(0, COARSE)],
                vec![noop_upgrade(0, 0, 0), simple(&t1, TILE_FLAG_DIFFERENCE), noop_upgrade(2, 1, 0)],
            )],
        );
        // The same tile twice in one region: the second decode must see the first.
        let dup = stream(
            false,
            vec![region(full, vec![], vec![simple(&t1, 0), simple(&t1, TILE_FLAG_DIFFERENCE)])],
        );
        let outs = assert_same(&[(1, 0, &first_pass), (1, 0, &second), (1, 0, &dup)]);
        assert!(outs.iter().all(Option::is_some));
    }

    #[test]
    fn errors_match_ironrdp_and_never_panic() {
        let t = encode_tile(0, 0, &pattern(1), &GRD_QUANT, &ComponentCodecQuant::LOSSLESS);
        let out_of_bounds = encode_tile(9, 9, &pattern(1), &GRD_QUANT, &ComponentCodecQuant::LOSSLESS);
        let no_context = stream(false, vec![region(vec![rect(0, 0, 64, 64)], vec![], vec![simple(&t, 0)])]);
        let oob =
            stream(true, vec![region(vec![rect(0, 0, 64, 64)], vec![], vec![simple(&out_of_bounds, 0)])]);
        let no_reference = stream(true, vec![region(vec![], vec![], vec![simple(&t, TILE_FLAG_DIFFERENCE)])]);
        let bad_quality = stream(true, vec![region(vec![], vec![], vec![first(&t, 3)])]);
        let mut bad_quant = region(vec![], vec![], vec![simple(&t, 0)]);
        bad_quant.quant_vals.clear();
        let bad_quant = stream(true, vec![bad_quant]);
        let outs = assert_same(&[
            (1, 0, &no_context),
            (1, 0, &oob),
            (2, 0, &no_reference),
            (3, 0, &bad_quality),
            (4, 0, &bad_quant),
            (5, 0, &[0xC0, 0xCC, 0xFF]),
        ]);
        assert!(outs.iter().all(Option::is_none));
        let mut codec = ProgressiveCodec::new(pool());
        assert!(matches!(
            codec.decode(1, 0, Size::new(40_000, 10), &single_tile_stream(0, 0, 1)),
            Err(CodecError::SurfaceTooLarge { .. } | CodecError::Malformed { .. })
        ));
    }

    #[test]
    fn frame_bracketing_shares_tiles_between_payloads_like_ironrdp() {
        let t0 = encode_tile(0, 0, &pattern(1), &GRD_QUANT, &ComponentCodecQuant::LOSSLESS);
        let a = stream(true, vec![region(vec![rect(0, 0, 32, 32)], vec![], vec![simple(&t0, 0)])]);
        // A second payload with no tiles whose REGION rect is covered by the tile above.
        let b = stream(false, vec![region(vec![rect(10, 10, 20, 20)], vec![], vec![])]);

        let mut ours = ProgressiveCodec::new(pool());
        let mut theirs = ProgressiveDecoder::new();
        ours.begin_frame();
        theirs.begin_frame();
        for data in [&a, &b] {
            let x = ours.decode(1, 0, SURFACE, data).ok();
            let y = reference(&mut theirs, 1, 0, SURFACE, data);
            assert_eq!(x, y);
        }
        ours.end_frame();
        theirs.end_frame();
        // Outside a frame the second payload stands alone: nothing to show.
        let x = ours.decode(1, 0, SURFACE, &b).ok();
        assert_eq!(x, reference(&mut theirs, 1, 0, SURFACE, &b));
        assert_eq!(x.map(|v| v.len()), Some(0));
    }

    #[test]
    fn context_lifecycle_matches_ironrdp() {
        let with_ctx = single_tile_stream(0, 0, 3);
        let t = encode_tile(0, 0, &pattern(4), &GRD_QUANT, &ComponentCodecQuant::LOSSLESS);
        let without_ctx = stream(false, vec![region(vec![rect(0, 0, 64, 64)], vec![], vec![simple(&t, 0)])]);
        let diff = stream(
            false,
            vec![region(vec![rect(0, 0, 64, 64)], vec![], vec![simple(&t, TILE_FLAG_DIFFERENCE)])],
        );

        let mut ours = ProgressiveCodec::new(pool());
        let mut theirs = ProgressiveDecoder::new();
        let step = |ours: &mut ProgressiveCodec, theirs: &mut ProgressiveDecoder, ctx: u32, data: &[u8]| {
            let x = ours.decode(1, ctx, SURFACE, data).ok();
            assert_eq!(x, reference(theirs, 1, ctx, SURFACE, data));
            x.is_some()
        };
        assert!(step(&mut ours, &mut theirs, 0, &with_ctx));
        // A new context id without CONTEXT block inherits the surface's flag (Windows behaviour).
        assert!(step(&mut ours, &mut theirs, 7, &without_ctx));
        ours.delete_context(1, 7);
        theirs.delete_context(1, 7);
        assert!(step(&mut ours, &mut theirs, 7, &without_ctx));
        ours.reset();
        theirs.reset();
        // References survive reset: a difference tile still decodes.
        assert!(step(&mut ours, &mut theirs, 0, &diff));
        ours.delete_surface(1);
        theirs.delete_surface(1);
        // After deleting the surface there is no CONTEXT flag and no reference.
        assert!(!step(&mut ours, &mut theirs, 0, &without_ctx));
        assert!(!step(&mut ours, &mut theirs, 0, &diff));
        assert!(format!("{ours:?}").contains("ProgressiveCodec"));
    }

    #[test]
    fn surface_resize_reallocates_the_tile_grid() {
        let data = single_tile_stream(2, 1, 5);
        let mut ours = ProgressiveCodec::new(pool());
        let mut theirs = ProgressiveDecoder::new();
        for size in [SURFACE, Size::new(100, 64), SURFACE] {
            let x = ours.decode(1, 0, size, &data).ok();
            assert_eq!(x, reference(&mut theirs, 1, 0, size, &data), "size {size:?}");
        }
    }
}
