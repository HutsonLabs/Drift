# HOST-4 — Real-time encoder and AVC444 for the Mac host

- Status: accepted
- Date: 2026-10-18
- Task: HOST-4 (`TASKS.md`); answers ADR `IOS-0` open question 5

## Context

Drift Host must stream the Mac's screen as RDP GFX H.264 (ADR `IOS-0` decision 2):
AVC420 for every client, and AVC444 (two AVC420 streams) for full-chroma text. `drift-video`
already had a VideoToolbox encoder for session recording (M8-2), behind the `recording`
feature with the MP4 writer.

## Decisions

1. **One encoder, two profiles.** `encode` is no longer behind `recording` (it needs no
   AVFoundation); only `mp4` is. `EncoderConfig::new` stays the recording profile.
   `EncoderConfig::realtime(size, fps)` is the host's: real-time, no frame reordering,
   a keyframe at least every 4 s, `realtime_bitrate` (1/20 bit per pixel per frame, clamped to
   2–80 Mbit/s: 4K60 ≈ 25 Mbit/s, 1080p60 ≈ 6 Mbit/s) as a starting point for EXT-4.
   `VtEncoder::request_keyframe` forces the next frame to be an IDR (new client, lost frame).
   Every `EncodedFrame` records `completed_at`, the time of VideoToolbox's callback, for the
   host's latency statistics.
2. **Low-latency rate control up to 2560×1600.** With
   `kVTVideoEncoderSpecification_EnableLowLatencyRateControl` (Constrained High),
   `VTCompressionSessionEncodeFrame` returns only when the frame is done: 0.1 ms per call at
   1080p, 8.8 ms at 2560×1600, 18.6 ms at 4K, which caps 4K near 54 fps. Below that size it has
   the tightest latency (1080p: p95 5.9 ms against 12.0 ms). So `realtime` turns it on up to
   `LOW_LATENCY_MAX_PIXELS` (2560×1600) and uses the normal rate control above, which holds
   4K60 at p50 23 ms. If a VideoToolbox refuses the low-latency session, the encoder falls back to
   the normal one (`VtEncoder::is_low_latency` says which).
3. **Feed NV12, not BGRA.** Handing VideoToolbox BGRA makes it convert internally, which adds
   about 110 ms of pipelining at 4K (p50 110 ms against 16 ms for NV12). HOST-3's capture should
   ask ScreenCaptureKit for full-range 4:2:0 (`420f`) when the session is AVC420.
4. **AVC444 v1 layout, built from g-r-d's matrix.** `drift_video::avc444` converts BGRA with
   g-r-d's integer BT.709 full-range matrix (the one `drift-render::color` inverts, so a Mac host
   and a GNOME host look the same) and splits the 4:4:4 picture into the main view (luma +
   2×2-averaged chroma, a correct 4:2:0 picture on its own) and the auxiliary view (odd chroma
   rows in 16-row bands as its luma, odd-column even-row chroma as its chroma), per MS-RDPEGFX
   3.3.8.3.2. The auxiliary view is padded to whole 16-row bands; otherwise the last odd V rows
   of a picture whose height is not a multiple of 16 have nowhere to go. `split_bgra` does it in
   one pass; `split`/`combine` are the reference and the client's inverse.
5. **The client filters rebuilt chroma.** Rebuilding the even-row, even-column sample from the
   average multiplies coding error by four. `combine_filtered` keeps the average unless the
   rebuilt sample differs by more than 30 (`CHROMA_FILTER_THRESHOLD`, FreeRDP's choice). On a
   1080p desktop-like picture at the real-time bit rate: AVC420's chroma U 31.4 / V 26.5 dB,
   AVC444 unfiltered U 30.1 / V 28.8 dB, filtered U 32.3 / V 30.6 dB.
6. **Writers next to the parser.** `metablock` now writes `RFX_AVC420_BITMAP_STREAM` and writes
   and parses `RFX_AVC444_BITMAP_STREAM` (size in bits 0–29, `LC` in bits 30–31); the AVC420
   writer round-trips through the client's existing parser.
7. **`Avc444Encoder`** runs one real-time `VtEncoder` per view, encodes both views of a frame
   on two threads, and pairs their outputs in order.

## Measurements (open question 5)

Apple M4 Pro, macOS 27, `cargo run --release -p drift-video --example realtime_bench`. Frames
are submitted on a 60 Hz clock; latency is submission → output callback (both views for
AVC444). Three other agents were compiling on the same machine (load average 90–190 on 14
cores), so the CPU-side numbers are pessimistic; the encoder numbers repeated within a few
percent across runs.

| Stream | Size | fps | latency p50 / p95 | bit rate |
|---|---|---|---|---|
| AVC420, NV12 in, low-latency RC | 1920×1080 | 60.0 | 5.9 / 6.0 ms | 5.7 Mbit/s |
| AVC420, NV12 in, low-latency RC | 2560×1600 | 60.1 | 8.5 / 9.1 ms | 10.9 Mbit/s |
| AVC420, NV12 in, low-latency RC | 3840×2160 | 53.8–55.6 | 16.4 / 21.5 ms | 16 Mbit/s |
| AVC420, NV12 in, normal RC | 3840×2160 | 59.9 | 23.2 / 34.6 ms | 18.6 Mbit/s |
| AVC420, BGRA in, low-latency RC | 3840×2160 | 45.1 | 110 / 110 ms | 15.5 Mbit/s |
| AVC444, both views, encoders only | 1920×1080 | 59.5–59.8 | 9.4–10.7 / 13.8–35 ms | 12 Mbit/s |
| AVC444, both views, encoders only | 2560×1600 | 44.6–46.9 | 18.1 / 34–43 ms | 19–20 Mbit/s |
| AVC444, both views, encoders only | 3840×2160 | 24–26.5 | 318–331 / 409–610 ms | 30 Mbit/s |

The CPU split (`split_bgra`) costs 25 ms per 4K frame on one core and 13–33 ms with rayon on
this loaded machine; uploading both NV12 views to IOSurfaces costs 2–3 ms.

**Answer:** a single 4K60 H.264 stream is fine. AVC444's two streams hold 60 fps up to about
1080p on an M4 Pro, about 45 fps at 2560×1600 and about 25 fps at 4K: the media engine, not the
CPU, is the limit (the views were split up front). The question asked about M1, which has a
smaller media engine; it is not measured here (manual step in `docs/acceptance.md`).

## Consequences

- The host picks AVC444 only where it holds the frame rate (about 2 Mpx at 60 fps here) and
  AVC420 above it. Two cheaper ways to keep full chroma at 4K are left to HOST-2/EXT-4: the
  `LC` field lets the host send luma-only frames while the picture moves and a chroma-only
  refinement once it settles (what g-r-d's dual view does), and EXT-5's HEVC 4:4:4 needs one
  stream instead of two.
- The 4:4:4 split belongs on the GPU (a Metal compute pass over the captured IOSurface, as g-r-d
  does with a Vulkan shader); the CPU version is the reference and the fallback.
- Drift's own client does not decode AVC444 yet: `drift-gfx` treats it as unsupported and the
  capabilities avoid it (ADR `M1-2-gfx-client`). `combine_filtered` is the client half of the
  pixel math; wiring AVC444 into `drift-gfx` and the compositor is follow-up work before a Drift
  client benefits.
- ADR `IOS-0`'s context says the client "already has … AVC444"; it has the AVC420 path only.
