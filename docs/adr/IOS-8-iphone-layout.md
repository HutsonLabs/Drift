# IOS-8 — iPhone layout and key row

- Status: accepted
- Date: 2026-10-18
- Task: IOS-8 (`TASKS.md`); builds on ADRs `IOS-5-ios-app-shell`, `IOS-6-ios-input`, `IOS-7-touch-modes`

## Decisions

1. **Pure layout, native placement.** `phone::phone_layout(container, safe area, desktop,
   controls)` decides the frames; `windows_ios_phone.rs` (a part of the iOS shell) applies them
   while a live picture is on screen, on iPhone only (`UIUserInterfaceIdiom::Phone`). Every other
   surface (forms, overlays, HUDs) and the iPad keep IOS-5's layout.
2. **Portrait:** the picture is full width at the desktop's aspect, just below the status area,
   at most 60 % of the usable height. Below it a native `TrackpadView` fills the middle and the
   page becomes the key-row strip above the home indicator. The trackpad is always a trackpad
   (IOS-7's machine, own instance) whatever the picture's touch mode, so the picture itself can be
   in direct mode. With an adaptive profile the remote desktop follows the picture's size
   (Display Control), so text stays sharp.
3. **Landscape:** the picture fills the screen. The key row floats over its bottom edge inside
   the safe area when the phone turns to landscape and when the bottom 44-point strip of the
   picture is touched (that touch is not sent to the remote); it hides after 4 s without use
   (`phone::AutoHide`). Using a key keeps it up.
4. **The key row is web UI** in the session page (`ui/src/views/keyRow.ts`): Esc, Tab, latching
   Control / Alt / Command, arrows, a keyboard button that focuses a hidden field to bring up
   the iOS keyboard, and a touch-mode switch. It talks to Rust through three commands:
   `send_keys` (a chord of `phone::SoftKey` names plus single letters and digits — a latched
   modifier and a typed letter become one chord), `type_text` (Unicode key events; newline is
   Return) and `set_touch_mode` (this session only). Web UI because the key row is ordinary
   buttons and text entry, which the page already styles and tests, and the latency of a key tap
   through IPC does not matter the way pointer motion does.
5. **Page ↔ shell.** `PhoneControlsChanged { visible, touch }` tells the page to show the key row;
   the page also asks once with `phone_controls` when it loads, because the first event can come
   before it listens. The Mac never sends it; the Mac's shell has no-op `controls_used`,
   `set_touch_mode` and `phone_controls`.
6. **Rotation.** An invisible, touch-transparent `ResizeWatcher` at the back of the window's
   root view reports each new size and re-runs the layout. Views that may carry IOS-7's zoom
   transform are placed by bounds and centre, never by frame.
7. **No window behind shows through.** iPhone windows stack in one scene (IOS-5), so a phone
   session window's root view gets an opaque system background.
8. **Smoke runs can turn the scene.** The simulator cannot be rotated without its GUI; with the
   `smoke-env` feature, `DRIFT_SMOKE_ORIENTATION=landscape` asks the scene for landscape
   (`requestGeometryUpdate`) 8 s after the window opens.

9. **Amended by RESP-14 (2026-10-22).** The portrait key row is two rows of 44-point keys
   (`phone::KEY_ROWS_HEIGHT_PORTRAIT`, 106 points; the trackpad gives up 50 points), because the
   13 keys need about 717 points and a hidden sideways swipe hid the arrows. Landscape keeps one
   56-point row (`KEY_ROW_HEIGHT`). The keyboard and touch-mode keys are SVG icons
   (`keyboard`, `trackpad`, `hand`) instead of ⌨ ▭ ☝.

## Consequences

- Layout, key events and auto-hide are unit tested (`src-tauri/tests/phone_layout.rs`), the key
  row in bun (`ui/test/keyRow.test.ts`), the trackpad view, reveal strip and resize watcher in
  the simulator (`crates/drift-ios/tests/input.rs`). Portrait and landscape were checked by
  screenshot in an iPhone 17 Pro simulator against `fake_host`.
