# AVC444-client — Full chroma between Drift and Drift Host

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Fills a gap in ADR `IOS-0` decision 2, which assumed Drift's client already decoded AVC444.
  HOST-1 and HOST-4 found it did not: the client offered only V8.1 and rejected AVC444 frames.

## Context

MS-RDPEGFX's AVC444 sends each picture as two AVC420 streams: the main view (luma plus averaged
chroma) and the auxiliary view (the chroma samples 4:2:0 drops). A client recombines them into a
4:4:4 picture. HOST-4 built the host half: `Avc444Encoder`, the split and the filtered
recombination (`drift_video::avc444`).

g-r-d (GNOME) reacts to V10 capability sets by sending AVC444v2, whose auxiliary layout differs,
or Progressive with `AVC_DISABLED` (ADR `M1-2-gfx-client`). Drift does not decode AVC444v2.

## Decisions

1. **V10 only for paired Drift Hosts.** `drift_gfx::GfxCaps` has two lists. `Gnome` (the
   default) is the verified `[V8_1{AVC420_ENABLED}, V8{}]`, byte for byte as before.
   `DriftHost` is `[V10{}, V8_1{AVC420_ENABLED}, V8{}]`. The app uses `DriftHost` when the
   profile has `paired_host` (IOS-9), so GNOME sessions cannot regress.
2. **AVC444 v1 only.** The client decodes codec id `Avc444`; `Avc444v2` stays unsupported. Drift
   Host only sends v1, and no other server is offered V10.
3. **Decoder.** `drift_core::video::Avc444Decoder` is the seam; `drift_video::decode::
   VtAvc444Decoder` implements it with one VideoToolbox session per view, keeps the last picture
   of each (for `LC` = luma only or chroma only), recombines with
   `combine_filtered(…, CHROMA_FILTER_THRESHOLD)` and converts to BGRA with the BT.709
   full-range inverse of g-r-d's matrix (`yuv444_to_bgra`, within 4 per channel). Before the
   first auxiliary view, the main view is shown alone with its 4:2:0 chroma.
4. **CPU path first.** `drift-gfx` blits the regions of the first stream's metablock from the
   BGRA picture (`FrameSink::blit_bgra`). A GPU path (two NV12 textures recombined in a Metal
   shader) is a follow-up; at 1080p the CPU path is fine for the host's AVC444 range.
5. **Host choice.** `drift_host::codec::choose(avc420, avc444, size)` picks AVC444 when the
   client confirmed it and the picture is at most 1920×1080 (HOST-4: both views hold 60 fps up
   to about that size on an M4 Pro), AVC420 otherwise. The capture pipeline calls it per output
   size.

## Consequences

- Measured on coloured one-pixel strokes on white (640×368): chroma PSNR 33.2 dB with both
  views against 19.3 dB from the main view alone (what AVC420 shows); 34.7 dB end to end over
  loopback through `ironrdp-server`.
- `GfxAckOnly` (no tile pool) still advertises V8.1 only.
- Supporting AVC444v2 (for GNOME or Windows servers) needs its own decoder; until then those
  servers keep getting V8.1.
