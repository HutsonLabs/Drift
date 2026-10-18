# HOST-10 — A virtual display sized to the device

- Status: accepted
- Date: 2026-10-18
- Task: HOST-10 (`TASKS.md`); implements ADR `IOS-0` decision 5

## Context

A client sends its monitor layout over Display Control. Drift Host should answer with a display
of exactly that size and scale, so an iPad gets its own native resolution instead of a scaled
Mac desktop. macOS has no public API for that; `CGVirtualDisplay` is private CoreGraphics.

## Decisions

1. **Pure planning.** `drift_host::vdisplay_plan::plan` turns a layout into a `ModePlan`:
   pixels clamped to 640×480 … 5120×2880 (larger layouts shrink keeping their shape; the
   encoder holds 60 fps up to 4K, HOST-4), even sides for H.264, HiDPI from a desktop scale of
   150 % (points are half the pixels), 60 Hz, and a physical size from the client or from a
   typical density (220 ppi Retina, 110 ppi otherwise). Only a different plan changes the
   display.
2. **One private module.** `drift_host::cg_virtual` is the only code that touches the private
   classes (`CGVirtualDisplayDescriptor`, `CGVirtualDisplay`, `CGVirtualDisplaySettings`,
   `CGVirtualDisplayMode`), through the Objective-C runtime. It is reimplemented from the
   API's shape; no third-party code is copied. Argument types follow the private headers
   (`unsigned int` sizes); objc2 checks encodings in debug builds, which caught a wrong
   `NSUInteger` during development.
3. **Self-test at launch.** `cg_virtual::self_test` checks every class and selector the module
   uses without creating anything. If it fails (a macOS release changed the API) or creation
   fails at run time, the host captures the physical display and the client letterboxes.
4. **A display manager seam.** `HostOptions::displays` is a `DisplayManager`: `apply` a layout
   → a `DisplayTarget` (`Virtual { display_id, plan }` or `Physical`), `release` when the session
   ends. `VirtualDisplays` owns the (non-`Send`) objects on a thread of its own and resizes the
   same display when the plan changes. The host reports the current target in `HostStats::display`
   and sizes the desktop to the virtual mode; the capture side (HOST-3 / the pipeline) captures
   `DisplayChoice::Id(display_id)`.
5. **On by default.** `host.toml`'s `virtual_display` (default `true`) can switch it off.
6. **The display lives with the session.** It is released when the session ends; the next
   client gets one at its own size.

## Consequences

- Planning and the manager seam are unit tested, and a loopback test shows a client's layout
  reaching the manager. The self-test runs in the gate.
- Creating a real display changes the user's arrangement, so that test is `#[ignore]`d; it was
  run once on the dev Mac (macOS 27, 2026-10-18): the display appeared in
  `CGGetActiveDisplayList` and was gone a second after drop.
- Not yet checked: that a HiDPI virtual display captures at the planned pixel size, and what
  macOS does with windows on it when the session ends. Both are manual steps.
