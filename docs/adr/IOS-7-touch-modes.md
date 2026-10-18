# IOS-7 — Touch modes and pinch zoom

- Status: accepted
- Date: 2026-10-18
- Task: IOS-7 (`TASKS.md`); builds on ADR `IOS-6-ios-input`

## Decisions

1. **One pure state machine** (`drift_input::touch::Touches`) for both modes, fed with plain
   finger ids, screen points and timestamps by the `RemoteView`, driving IOS-6's `Pointer`:

   | Gesture | Trackpad | Direct |
   |---|---|---|
   | one finger, drag | relative pointer, ×1.5 | left-drag from where the finger went down |
   | one finger, tap (≤ 0.3 s, ≤ 10 pt) | left click where the pointer is | left click where you touch |
   | tap, touch again within 0.35 s and drag | left-drag | — |
   | one finger held ≥ 0.6 s without moving | — | right click |
   | two-finger tap | right click | right click |
   | two-finger drag | scroll, natural direction, through M2-3's accumulator | the same |
   | pinch (spread changes > 20 pt) | local zoom, iPhone only | the same |

   Once two fingers were down, nothing more happens until all fingers lift, so the finger left
   after a scroll does not jerk the pointer. Three fingers abandon the gesture (iPadOS uses
   them). A UIKit cancel releases a held button.
2. **The mode is a profile preference with a device default.** `ConnectionProfile::touch`
   (`auto` | `trackpad` | `direct`, kebab-case, not written while `auto`, so existing
   `profiles.toml` files do not change). `auto` means trackpad on iPhone (the picture is small and
   a finger hides what it touches) and direct on iPad. The form shows it as "Touch on iPhone and
   iPad"; the Mac ignores it.
3. **Zoom is a view transform, not a compositor change.** On iPhone a pinch magnifies the
   picture locally (1×–4×) about the pinch centre and follows the centre as it moves
   (`drift_input::zoom::Zoom`); the remote desktop size does not change. The `RemoteView` gets a
   centre-anchored `UIView.transform` computed from the zoom, so the Metal drawable keeps its
   size and only the presentation scales. UIKit reports touches in the view's own,
   untransformed space; the state machine instead takes **screen points** (the superview's
   space) and direct mode maps them through `Zoom::to_view`, so a tap lands on the pixel under
   the finger at any zoom. The iPad has no local zoom: the picture is already large and the
   desktop follows the window (Display Control).
4. **Pointer and Pencil.** Indirect-pointer touches stay IOS-6's clicks; finger and Apple Pencil
   touches go through the state machine. The view enables multiple touch.

## Consequences

- All gestures are unit tested in `drift-input`; the view's wiring and the zoom transform are
  smoke tested in the simulator (`crates/drift-ios/tests/input.rs`). Real fingers are a manual
  check in `docs/acceptance.md`.
- A rotation while zoomed keeps the zoom; IOS-8's layout may reset it.
