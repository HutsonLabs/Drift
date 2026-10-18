# HOST-3 — Screen capture for Drift Host

- Status: accepted
- Date: 2026-10-18
- Task: HOST-3 (`TASKS.md`); follows ADR `IOS-0` decision 4

## Decisions

1. **New crate `drift-capture`.** Pure modules (`config`, `display`, `frame`, `rect`, `cursor`)
   build on every target and carry the logic, with unit tests. The ScreenCaptureKit and
   CoreGraphics glue (`sck`, `cursor_sample`) is `cfg(target_os = "macos")`, and its
   dependencies sit under `[target.'cfg(target_os = "macos")'.dependencies]`. The crate is not
   in the iOS gate: Drift Host only runs on a Mac.
2. **One `SCStream` per display**, at the display's native pixel size (the current mode's
   pixel width and height), with `showsCursor` off, a queue depth of 4 and a 60 fps cap by
   default. Frames are IOSurface-backed `CVPixelBuffer`s, which
   `drift_video::encode::PixelBuffer::from_cv` takes without a copy.
3. **Pixel format.** NV12 full range (`420f`) by default, which VideoToolbox's H.264 encoder
   takes directly (AVC420). `BGRA` is available for AVC444, which needs full chroma (HOST-4).
4. **Damage from ScreenCaptureKit.** `SCStreamFrameInfoStatus` decides whether a frame has new
   pixels (`Complete`, `Started`); idle, blank, suspended and stopped frames are dropped before
   they reach the encoder. `SCStreamFrameInfoDirtyRects` are taken in output-pixel coordinates,
   rounded outward, clipped, merged when they overlap or touch, and capped: more than 16 rects
   become their bounding box, and more than half the frame becomes a full update. A started
   stream, or a complete frame without dirty rects, is a full update.
5. **The pointer is reported separately.** RDP sends pointer position and shape as their own
   updates, so the picture never contains the pointer. The position is a null `CGEvent`'s
   location (global points, top-left origin) mapped to the display's pixels (the pixel that
   contains the point); off the display it is `Hidden`. The shape is
   `NSCursor.currentSystemCursor` drawn into a straight-alpha RGBA bitmap, capped at 384×384
   (RDP's largest pointer). `CursorTracker` reports only changes, and shapes carry a content
   hash so the host can cache them on the wire.
6. **`currentSystemCursor` is deprecated**, with the advice to let ScreenCaptureKit draw the
   pointer. That is exactly what an RDP host must not do, and there is no other public API for
   the system pointer's shape, so the call is allowed with a comment. If Apple removes it, the
   fallback is `showsCursor` on (pointer in the picture) with position-only pointer updates.
7. **Permission.** ScreenCaptureKit needs Screen Recording. `sck::has_permission` uses
   `CGPreflightScreenCaptureAccess` (never prompts); without it `displays` and
   `CaptureStream::start` fail with `CaptureError::NoPermission` and the smoke tests skip with a
   message. The pointer sampler and `sck::main_display` (CoreGraphics only) need no permission.

## Consequences

- The development Mac running the gate has no Screen Recording grant, so the stream tests
  skip there; the permissioned run is a manual step in `docs/acceptance.md`. The pointer
  sampler ran for real (a 56×80 Retina arrow with its hotspot).
- Whether dirty rects are in output pixels or points is checked in that manual run; if they
  are points, `FrameMeta::scale` becomes the display scale (one line in `sck::frame_meta`).
