# M1-6 / M2-2 / M2-4 / M2-5 / M3-2 / M6-2 / M7-2 — drift-macos platform layer

- Status: accepted
- Date: 2026-09-22
- Tasks: M1-6 (RemoteView), M2-2 (NSTextInputClient), M2-4 (performKeyEquivalent), M2-5
  (remote cursor), M3-2 (Keychain), M6-2 (native tab helpers), M7-2 (NWPathMonitor / wake),
  Local Network Privacy helper (stream C)

## Context

Plan §1.8 fixes the AppKit approach (layer-hosting `NSView` with a `CAMetalLayer` below the
`WKWebView`, `performKeyEquivalent:` claiming combos, native tabs via `addTabbedWindow:ordered:`
and `newWindowForTab:` on `TaoWindow`, occlusion notifications, errno 65). This ADR records the
decisions made while turning the spike (`~/code/drift-spikes/tauri-spike`) into `drift-macos`.

## Decisions

1. **Humble object + pure brain.** `RemoteView` (`define_class!`) only extracts plain values
   from `NSEvent`s. All routing lives in the pure `drift_macos::input::InputController` (over
   `drift-input`'s `Keyboard`, `ScrollAccumulator`, `Viewport`), unit-tested without AppKit.
2. **`performKeyEquivalent:` only claims for the first responder.** AppKit offers key
   equivalents to every view in the window; when the web view has focus (connect form),
   `RemoteView` returns `NO` so Cmd+C/Cmd+V work in the form. Allow-listed combos return `NO`
   *and* cancel the deferred Command press (`Keyboard::menu_shortcut_taken`), so the remote
   never sees them. Everything else with Command or Control is claimed and sent as scancodes.
3. **Command key-ups.** `NSApplication` does not dispatch `keyUp:` while Command is held, so a
   claimed Cmd+K would leave K pressed on the remote. `view::install_key_up_monitor` (a local
   `NSEvent` monitor for key-up with Command) forwards those key-ups to the key window's
   `RemoteView`; `Keyboard::key_up` is idempotent, so a duplicate is harmless. The app installs
   it once at startup.
4. **Focus.** Losing first responder *or* the window resigning key (observed with selector
   observers on `NSWindowDidResignKeyNotification`) releases every held key and button on the
   remote; gaining it re-syncs lock state and held modifiers.
5. **IME / dead keys.** `NSTextInputClient` is implemented on the view. Marked text is kept
   local (only its length, for `hasMarkedText`); committed text goes through
   `Keyboard::insert_text`. Candidate windows are anchored at the view's bottom-left.
6. **Remote cursor decode lives in `drift-macos::cursor` (pure).** It parses whole fast-path
   output PDUs with `ironrdp-pdu`, reassembles `FIRST`/`NEXT`/`LAST` fragments (the 86×86
   pointer at scale 200 is split), keeps a slot cache (a new pointer in a slot evicts the old
   one; `reset` clears it on reconnect) and outputs **premultiplied BGRA** like
   `drift_rdp::CursorBitmap`. It decodes with IronRDP's `Accelerated` target (straight alpha)
   and premultiplies itself: IronRDP's `Software` target premultiplies with `c*c/256` instead
   of `c*a/255`. Zero-sized pointers are rejected by IronRDP's attribute decoder and surface as
   `PointerError::Malformed` (never a panic). If `drift-rdp` keeps IronRDP's own pointer
   handling (`ActiveStageOutput::PointerBitmap`), it can use `CursorImage::from_decoded`.
   `NSCursor` size is `bitmap / (scale / 100)` points (plan M2-5); server pointer-position
   updates do not warp the Mac pointer. A hidden pointer is a transparent 1×1 cursor (not
   `+[NSCursor hide]`, which is global).
7. **Keychain.** Items are generic passwords: service `com.example.drift` (the bundle id),
   account `SecretRole::account(profile_id)` = `<uuid>/<role>`. One code path (Security's
   keychain-item API on an explicit `SecKeychain`) serves both production (the default/login
   keychain) and tests. Tests create a **throw-away keychain file** in a temp dir with a
   unique test-only service name: the login keychain is not reachable from SSH/CI sessions
   (`errSecInteractionNotAllowed`), and tests must never touch the user's items.
   `drift-app`'s `KeychainSecretStore` wraps it and replaces the in-memory store.
8. **Reconnect triggers.** `nw_path_monitor` is bound directly (Network.framework C API; no
   objc2 crate exists for it) on a private serial dispatch queue. `satisfiable` counts as
   online (a connection attempt brings up VPN-on-demand/cellular). Wake uses
   `NSWorkspaceDidWakeNotification`. Both feed `TriggerFeed`, a thread-safe wrapper around the
   pure `TriggerMerger`; the sink is called outside the lock.
9. **Tabs.** `tab_title` = state glyph + profile name (● connected, ◐ greeter, ◌ connecting,
   ↻ reconnecting, ○ idle/disconnected, ⚠ failed; control characters stripped, blank →
   "Untitled"). `install_new_window_for_tab` adds the method to the real class (skipping an
   `NSKVONotifying_` isa); the handler is process-wide and the first one installed is kept.
   `add_tab` does not select/show the new tab; the caller does.
10. **AppKit tests run on the main thread** via a ~80-line libtest-compatible harness
    (`tests/support/harness.rs`, `harness = false`) that nextest drives (`--list --format
    terse`, `--exact`). Windows are created off-screen and never shown, so `appkit` runs in
    `cargo xtask ci`. The M6-2 window-group test orders a window front and needs tabbing, so it
    is behind `--features macos-ui-tests` and lives in `drift-macos/tests/tabs_ui.rs` with an
    `NSWindow` subclass standing in for `TaoWindow` (a real tao window needs a running Tauri
    event loop on the main thread, which a test cannot own). Run it with
    `cargo nextest run -p drift-macos --features macos-ui-tests`.
11. **Tauri glue behind a feature.** `drift-macos` stays usable without Tauri; the `tauri`
    feature adds `tauri_glue::{attach, show_remote, show_webview}` (hide the *webview*, not the
    window, then make the `RemoteView` first responder). `drift-app` enables it.

## Consequences

- `drift-app` owns wiring: create the `RemoteView` with `tauri_glue::attach`, give its
  `metal_layer()` to `drift_render::LayerTarget`, forward `RemoteViewHandler::input` to
  `SessionHandle::send(SessionCommand::Input)`, `geometry_changed` to `Resize`,
  `focus_changed` to `Focus`, `WindowObserver` events to `SetVisible`/`Focus`,
  `SessionEvent::Cursor` to `RemoteView::set_cursor_shape`, `ReconnectTriggers` actions to
  `NetworkReachable`/`ReconnectNow`, and call `install_key_up_monitor` once.
- Manual checks that need a person (real IME, Wi-Fi toggling, sleep, the tab bar "+" button,
  the Keychain item in Keychain Access) are in `docs/acceptance.md`.
