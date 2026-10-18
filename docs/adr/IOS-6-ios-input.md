# IOS-6 — Hardware keyboard, pointer and Pencil input on iPad

- Status: accepted
- Date: 2026-10-18
- Task: IOS-6 (`TASKS.md`); amends ADR `IOS-0` decision 8 ("a Swift Tauri plugin")

## Context

ADR IOS-0 planned a Swift Tauri plugin (`tauri-plugin-drift-input`) that forwards normalized
events to `drift-input` through the command layer. IOS-5 put the `RemoteView` in the app as a
Rust `UIView` subclass, and the session's input already flows from the view to its
`SessionLink` without JavaScript.

## Decisions

1. **Rust in `drift-ios`, not a Swift plugin.** The input sources are UIKit and GameController
   objects that `objc2` reaches directly. Keeping them in Rust means no hop through the webview
   and the IPC bridge for every pointer move (latency), one language, and the same split as on
   macOS: plain values are extracted in a humble object and every decision is in `drift-input`.
2. **Pure parts in `drift-input`.**
   - `hid`: HID keyboard usages (`GCKeyCode`) → set-1 scancodes. The table agrees with the macOS
     `kVK_*` table on every key both know (checked key by key), adds the PC keys macOS lacks
     (Print Screen, Scroll Lock, Num Lock, Non-US `#`, International 2/4/5, F21–F24), and maps the
     GUI keys through the profile's `CmdAs`. `HidKeyboard` sends each press and release once and
     lifts every held key on `release_all`.
   - `pointer`: one pointer for absolute positions (iPad pointer, Pencil hover, later direct
     touch) and relative motion (pointer lock, later the trackpad). RDP fast-path only has absolute
     moves, so relative motion moves a fractional desktop position, clamped to the desktop and
     starting at its centre; buttons press where the pointer is; scroll reuses M2-3's
     accumulator.
3. **Keyboard: `GCKeyboard`.** `hardware::HardwareInput` hooks the coalesced keyboard and sends
   every key change to the *active* view — the session whose picture is in front. The view is
   first responder and swallows UIKit's `presses*` so keys do not reach the page twice.
4. **Pointer without lock: UIKit.** A `UIHoverGestureRecognizer` (iPad pointer *and* Apple
   Pencil hover) gives absolute moves; clicks arrive as indirect-pointer touches
   (`UIApplicationSupportsIndirectInputEvents`), with the secondary button as a right click;
   a scroll-only `UIPanGestureRecognizer` (no touch types) carries wheel and two-finger trackpad
   scrolling. A `UIPointerInteraction` keeps the system pointer over the picture.
5. **Pointer lock.** As the IOS-1 spike showed, `prefersPointerLocked` is added at runtime to
   tao's `TaoUIViewController` and answered per instance (only a view controller Drift asked to
   lock says YES). The live picture asks for the lock; the page, the reconnect overlay and HUDs
   do not. iPadOS grants it only to a full-screen foreground scene. While locked, `GCMouse`
   deltas (y flipped), buttons and scroll drive the pointer; while not, they are ignored and
   UIKit's events are used, so nothing is sent twice.
6. **Releasing.** The previous active view releases its keys and buttons when another session
   comes to the front; every view releases them when the app goes to the background.
7. **Apple ISO keyboards** swap two usages in hardware (left of `1` reports Non-US `\`, right of
   left Shift reports Grave). macOS undoes it from the keyboard type, which iPadOS does not
   expose, so Drift keeps the HID standard; on such a keyboard those two keys come out swapped
   on the remote. A per-profile switch can follow if users ask.

## Shortcuts iPadOS keeps

These never reach an app, so they never reach the remote: Globe/fn shortcuts (Globe-H Home,
Globe-A Dock, Globe-C Control Center, Globe-N notifications, Globe-arrows window tiling,
Globe-Q Quick Note, Globe-E emoji), Cmd-Tab and Cmd-Shift-Tab (app switcher), Cmd-Space
(Spotlight), Cmd-H (Home), Cmd-Shift-3/4/5 (screenshots), Cmd-Option-D (Dock), Cmd-` within
Stage Manager, and Ctrl-Space or Globe when they switch the input language. Everything else,
including Cmd-Q, Cmd-W and Ctrl-Alt-Del's keys, is sent to the remote while its picture is in
front. With `CmdAs::Super` (default) Cmd sends the Windows/Super key, as on macOS.

## Consequences

- Smoke tests (`crates/drift-ios/tests/input.rs`) drive the view's entry points and the pointer
  lock patch in a simulator; the simulator cannot synthesize a hardware keyboard, mouse or
  Pencil, so those are manual steps in `docs/acceptance.md`.
- The project links `GameController.framework`.
