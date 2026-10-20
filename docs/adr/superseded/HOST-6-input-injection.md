# HOST-6 — Input injection on Drift Host

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Task: HOST-6 (`TASKS.md`); follows ADR `IOS-0` decision 4

## Context

Drift Host receives RDP fast-path input (scancodes, Unicode keys, mouse, wheel) and must
replay it on the Mac. The client already maps `kVK_*` and HID usages to set-1 scancodes
(`drift-input`, ADRs `M2-1` and `IOS-6`).

## Decisions

1. **One table, both directions.** `drift_input::keymap::kvk_for` is built from
   `scancode_for` over `ALL_KVK` (Command as the Windows keys), cached per keyboard type. Every
   mapped key round-trips on ANSI, ISO and JIS, and HID → scancode → `kVK_*` matches Apple's
   HID ↔ `kVK_*` correspondence. The tests check both.
2. **Intentional asymmetries.** The Windows keys come back as Command and Control stays Control
   (a client that sends Command as Control types Control). On an ISO Mac the two ISO positions
   swap back, so the physical key is kept. PC-only keys (Num Lock, Print Screen, Scroll Lock,
   Pause) inject nothing.
3. **A small crate, `drift-inject`.** HOST-2 creates `drift-host` later; injection is a separate
   concern with its own tests, so it does not wait for it. `plan` is pure (any platform);
   `cg` is the macOS humble object.
4. **The plan carries the state CoreGraphics does not.** An injected modifier key does not set
   the flags of later injected events, so every event carries the modifiers held. Moves with a
   button held are drags. Presses count clicks within 500 ms and 4 points. Desktop pixels map
   to global display points through a `DisplayMap` (origin, desktop size, points per pixel) and
   are clamped to the display.
5. **Wheel as continuous pixels.** 120 RDP units (one notch) are 40 pixels of continuous
   scrolling (`kCGScrollEventUnitPixel`). Windows' horizontal wheel is positive to the right,
   macOS's to the left, so it is negated. `Injector::scroll_pixels` takes a phase, which the
   poster writes to `kCGScrollWheelEventScrollPhase` / `MomentumPhase`, ready for EXT-2.
6. **Lock keys are reported, not toggled.** CGEvent cannot set Caps Lock reliably; RDP
   synchronize events become `InjectEvent::Locks` for the host's logs.
7. **Posting** is `CGEventPost` at the HID tap. It needs the Accessibility permission
   (`AXIsProcessTrusted`). Tests build the events and read them back; the only test that posts
   (a move to where the pointer already is) is `#[ignore]` and a manual step.
8. **`release_all`** lets go of every held key and button when a session ends or is
   suspended, so nothing stays stuck on the Mac.

## Consequences

- HOST-2 wires `Injector` to the server's input handler and calls `release_all` on disconnect.
- On 2026-10-18 the dev Mac's terminal had Accessibility, and the ignored posting test passed.
