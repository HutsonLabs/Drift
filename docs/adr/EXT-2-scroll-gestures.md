# EXT-2 — Precise scroll and trackpad gestures on `Drift::Mac::v1`

- Status: accepted
- Date: 2026-10-18
- Task: EXT-2 (`TASKS.md`); builds on ADR `EXT-1-channel-framing` and ADR `HOST-6-input-injection`

## Context

RDP wheel events have no phase, so a Mac host cannot do momentum or rubber-banding, and RDP has
nothing for swipes or pinches (ADR `IOS-0` decision 3). Drift sends both on its own channel when
the host is a Drift Host, and plain RDP otherwise.

## Decisions

1. **New input events** (`drift-core`, a cross-stream change): `InputEvent::PreciseScroll`
   (`dx`, `dy` in hundredths of a point with AppKit's `scrollingDelta` signs, `ScrollPhase`,
   `MomentumPhase`), `Swipe` (direction, fingers), `Pinch` (magnification change in thousandths,
   phase) and `SmartZoom`.
2. **A precise scroll carries its own fallback.** The platform layer still runs today's wheel
   accumulator and stores the resulting wheel units in the event (`wheel_v`, `wheel_h`). The
   session actor sends the event on the channel when both ends agreed on `SCROLL`, and otherwise
   encodes exactly those wheel units. One event, so the two paths can never both reach a server
   and the GNOME behaviour is unchanged unit for unit.
3. **Gestures need `GESTURES`.** Without it they are dropped: plain RDP has no equivalent, and
   guessing a shortcut for a GNOME or Windows host would surprise more than it helps.
4. **Wire format** (`drift_ext::gestures`, types `0x0101`–`0x0104`): fixed little-endian
   payloads, strict lengths, unknown enum values are errors. A malformed message is dropped with
   a log line; it never ends the session. The EXT-1 fuzz target now also round-trips gestures
   and feeds every frame to a host end with the gesture responder.
5. **Several responders per end.** `with_responder` adds rather than replaces; the first that
   consumes a message answers it. The host's channel has pairing (HOST-8) and gestures.
6. **Host mapping** (`drift_inject::gesture`, pure): scrolls become pixel scroll events with the
   matching `kCGScrollWheelEventScrollPhase` / `MomentumPhase` (a momentum *begin* phase was
   added). Swipes become the default Mission Control shortcuts: up → Control-Up (Mission
   Control), down → Control-Down (App Exposé), left → Control-Right and right → Control-Left (the
   Space the fingers pull in, as on a trackpad). A pinch becomes Command-= or Command-- for every
   0.25 of magnification, counted from the start of each pinch. Smart zoom has no shortcut that
   works everywhere, so it does nothing. Public CGEvent cannot post real gesture events.
7. **Gated like other input.** Gestures reach the injector only while the connection is
   authorized (HOST-8).
8. **Sources.** macOS: `scrollWheel:` with precise deltas (phase and momentum from AppKit's
   `NSEventPhase` bits), `swipeWithEvent:`, `magnifyWithEvent:`, `smartMagnifyWithEvent:`. iPad:
   the trackpad scroll recognizer's state as the phase. iPhone and iPad touch: a two-finger pan
   is a phased precise scroll that ends when the fingers lift; on iPad (no local zoom) a pinch is
   a remote pinch. `GCMouse` scrolling under pointer lock stays a plain wheel.

## Consequences

- `drift-input`'s two-finger test now reads the wheel units from the precise scrolls; the
  amount is unchanged.
- The host's real behaviour (momentum in Safari, Mission Control from a swipe) needs a person at
  a Mac with Accessibility granted; `docs/acceptance.md` lists the steps.
- Users who changed the Mission Control shortcuts on the host get whatever their shortcuts are
  now bound to; the host does not read the shortcut settings.
