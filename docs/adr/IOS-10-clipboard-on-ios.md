# IOS-10 — Clipboard on iOS

- Status: accepted
- Date: 2026-10-18
- Task: IOS-10 (`TASKS.md`); follows ADR `M5-1`, `M5-3` and ADR `IOS-5` decision 5

## Context

On macOS Drift polls `NSPasteboard.changeCount` and offers every local copy to the focused
session (M5-3). iOS shows "Drift would like to paste from …" whenever an app reads another app's
pasteboard contents without the user asking, so polling would prompt all the time.

## Decisions

1. **`drift_clipboard::uipasteboard::UiPasteboard`** (iOS only) implements `PasteboardPort`:
   text as `public.utf8-plain-text`, images as `public.png` (other images go through `UIImage`).
   `has_contents` asks only `hasStrings`/`hasImages`, which never prompt. Its dependencies sit
   under `[target.'cfg(target_os = "ios")'.dependencies]`, and `drift-clipboard` joins the iOS
   crates in the gate so its simulator smoke test is linked.
2. **Remote → local is a write, which needs no consent.** `windows_ios::write_pasteboard`
   writes the general pasteboard when the session's window is in front
   (`drift_ios::webview::is_front`: its scene is foreground-active and the window is key, the
   scene's key window, or the scene's only window — iPad session windows are scenes of their own
   and need not be "key").
3. **Local → remote only on an explicit paste.** A `UIPasteControl` (the system paste button,
   icon only) sits in the bottom-trailing corner of the picture while the picture is shown and
   the profile's clipboard is not off. Tapping it is the user's consent, so there is no prompt.
   Its target, `drift_ios::paste::PasteTarget`, loads the pasted text (`loadObjectOfClass:
   NSString`) and image (data, re-encoded to PNG) and calls the app once.
4. **A paste pastes.** `drift_app::paste::paste_commands` turns one paste into the session's
   `ClipboardLocalChanged` (filtered by the profile through `ClipboardContents::from_paste`, CRLF
   to LF) followed by Ctrl+V on the remote. Both go down the same connection in order, so the
   remote has the format list before the key arrives.
5. **No seeding.** Unlike macOS, a new iOS session is not seeded with the current pasteboard;
   that would be a read without consent.

## Consequences

- In the iPad simulator against `fake_host` (`FAKE_HOST_COPY_TEXT`): a remote copy lands on the
  iPad pasteboard (`xcrun simctl pbpaste`), and the client's initial format list stays empty
  although the iPad pasteboard held text — Drift did not read it.
- The paste button's tap could not be driven from the command line; the target's loading is
  covered by `crates/drift-ios/tests/paste.rs` in a simulator, and the tap is a manual step in
  `docs/acceptance.md`.
- A hardware keyboard's Cmd+V goes to the remote as keys (IOS-6), not through this path.
