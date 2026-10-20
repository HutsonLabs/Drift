# HOST-pipeline — The live picture, the pointer and injection in Drift Host

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Tasks: the glue between HOST-2 (host skeleton), HOST-3 (capture), HOST-4 (encoder), HOST-5
  (mailbox) and HOST-6 (injection); no task of its own in `TASKS.md`

## Context

HOST-2's host showed a test pattern and only logged input. The pieces for the real thing
existed separately: `drift-capture` streams NV12 frames with damage into a one-slot mailbox,
`drift-video` has a real-time H.264 encoder, `drift-inject` plans and posts CoreGraphics events.

## Decisions

1. **A live source per connection.** `HostOptions::live` makes one when a connection starts
   (`RdpServerDisplay::size`). The desktop is then the source's size. Without one, or when it
   fails (no Screen Recording), the host shows the test pattern as before and logs why.
2. **A pump thread per session** (`live.rs`) moves pictures to EGFX and pointer changes to the
   display updates, behind a small `GfxTarget` trait so the pump holds no server state. It stops
   when the session ends, which also stops the capture.
3. **Never encode under backpressure.** The pump asks the EGFX server first; while the client is
   behind, nothing is taken from the mailbox, so changes merge there (HOST-5). A frame the server
   still refuses makes the next one a keyframe: a dropped P-frame would leave later pictures
   referencing nothing.
4. **Damage becomes AVC420 regions.** Each picture's regions are what changed since the
   previous one (`regions_for`); keyframes cover the whole surface. Idle frames (no damage) are
   not encoded.
5. **A layout keeps the screen's size.** On a live desktop, a Display Control layout gets a new
   output and surface at the captured size and a keyframe. HOST-10's virtual display will size
   the desktop to the client instead.
6. **The pointer goes as RDP pointer updates**: positions, hide, and shapes (RGBA up to 96 px,
   larger ones as large pointers up to 384 px, converted to BGRA bottom-up), each shape once and
   then by cache slot (25 slots, round robin). It is sampled every 8 ms on the pump thread.
7. **Injection.** `ironrdp-server` delivers decoded keyboard and mouse events; `inject.rs` turns
   them back into fast-path events for `drift-inject`'s planner and posts the plan. Posting is a
   function, so tests record it. `inject_input` in `host.toml` is now optional: unset means on
   when Drift Host has Accessibility and logged only when it has not. Every session end
   releases held keys and buttons.
8. **`MovingSquare`** is a synthetic NV12 screen with exact damage. It drives the loopback test
   (no Screen Recording needed) and keeps the encoder, regions, acks and injection covered by CI.

## Consequences

- The loopback test streams 20+ pictures to `drift-rdp`, sees them acknowledged, and sees the
  client's key and pointer reach the injector (recorded, never posted).
- A real screen needs Screen Recording and a real injection needs Accessibility; both are manual
  steps in `docs/acceptance.md`.
- `NSCursor.currentSystemCursor` is read from the pump thread, not the main thread. It worked in
  HOST-3's sampler test; if it ever misbehaves off the main thread, sampling moves to a main-queue
  timer.
- The screen's size changing mid-session (a display mode change) rebuilds the encoder but not
  the surface; HOST-10 owns display sizes.

## Virtual displays and AVC444 (added when HOST-10 and AVC444-client merged)

- The live factory takes a `LiveRequest`: which display to capture (`None` = main, or HOST-10's
  virtual display id) and whether to encode AVC444.
- A client layout that gets a virtual display stops the pump, reopens capture on that display,
  and starts again on a new output and surface at the mode's size. The physical display keeps
  its size, as before. The next session starts on the main display again.
- When the graphics channel is ready the client's capabilities are known: `codec::choose`
  picks AVC444 if the client confirmed it and the picture is at most 1920×1080, and the source
  is reopened as `LiveAvc444Encoder` (BGRA capture, HOST-4's two views) if needed. Both views
  carry the same regions; `HostStats::avc444_frames` counts them.
- Tested on loopback with synthetic sources (`tests/live_glue.rs`): capture moves to the virtual
  display at 1024×640, and a client with Drift's V10 caps gets AVC444 frames while a V8.1 client
  never does.

