# IOS-5 — The Tauri app on iPhone and iPad

- Status: accepted
- Date: 2026-10-18
- Tasks: IOS-4, IOS-5 (`TASKS.md`); follows ADR `IOS-0` decision 8 and the IOS-1 spike notes

## Context

`drift-app` was macOS-only: `windows.rs` drives AppKit windows, the menu bar and the Dock, and
the app linked `drift-macos` unconditionally. IOS-4 gave `drift-ios` a `RemoteView`; IOS-5 has to
put it in the real app and connect.

## Decisions

1. **Same crate, two shells.** `src-tauri/src/windows.rs` stays the macOS shell;
   `windows_ios.rs` is compiled as `windows` on iOS and offers the same functions, so commands,
   the `SessionHost`, the `SessionManager` and the services are shared unchanged. The iOS shell
   has no menu bar, Dock menu, title-bar webview, saved frames or quit confirmation.
2. **One Tauri window per connection on iOS too.** The `session` template builds a window per
   session, as on macOS. On iPad each window becomes its own scene (IOS-1). On iPhone tao puts
   extra windows in the one scene, so the front window is the one shown; switching is IOS-11.
3. **Layering as on macOS (ADR M7-3).** wry adds the `WKWebView` as a subview of tao's root view;
   the `RemoteView` is inserted below it and the page is made transparent. `present::surface_for`
   decides: the page is hidden for the live picture (every touch reaches the picture), fills the
   window for forms and the overlay, or becomes the HUD's corner panel (`present::hud_frame`
   with a top-left origin).
4. **Visibility follows the app.** `UIApplicationWillEnterForeground` / `DidEnterBackground`
   send `SetVisible` to every session, and the foreground is also the reconnect "wake" trigger
   (ADR IOS-3). A 120 Hz `CADisplayLink` runs while any picture is live (IOS-4).
5. **No pasteboard polling on iOS.** iOS shows a paste prompt when an app reads the pasteboard on
   its own, so `PlatformServices` only runs the reconnect triggers there; the clipboard waits for
   IOS-10.
6. **Xcode project in the repository.** `src-tauri/gen/apple` (from `cargo tauri ios init`) is
   tracked; only `gen/schemas` stays ignored. Keys go in `project.yml` *and* `Info.plist`
   (xcodegen regenerates the plist): `NSLocalNetworkUsageDescription`, multiple scenes,
   `CADisableMinimumFrameDurationOnPhone`. The deployment target is iOS 17. A static library
   carries no link directives, so the project links VideoToolbox, CoreMedia, CoreVideo,
   IOSurface, Network and `libz` itself. `tests/app_config.rs` checks all of this.
7. **`run()` is the mobile entry point** through a private module with
   `#[tauri::mobile_entry_point]` (the macro's FFI function has no doc comment, so the module
   allows `missing_docs`). The library is built as `staticlib` + `rlib`.
8. **Simulator smoke runs.** The Keychain cannot be seeded from a script on iOS either, and iOS
   has no example binaries, so the `smoke-env` cargo feature makes `run()` read
   `DRIFT_SMOKE_SECRET*` into memory (`drift_app::smoke`, shared with the macOS `smoke`
   example). No release build enables it; the gate lints it for both iOS targets.
9. **A loopback host for first light.** `drift-testkit`'s `fake_host` example serves FakeServer
   NLA legs with GFX, Display Control and CLIPRDR, draws a moving square with a new
   `ServerAction::GfxFill`, writes a pinned profile, and can close every connection after a
   delay to exercise reconnects. The iOS simulator shares the Mac's loopback.
10. **The pure pointer decoder moved to `drift-apple`** (`drift_apple::cursor`, re-exported by
    `drift-macos`), so `present.rs` builds on iOS. Drawing the remote pointer on iPad is IOS-6.

## Consequences

- First light was verified in an iPad simulator against `fake_host`; the GNOME host was down,
  so the device run against it is a manual step in `docs/acceptance.md`.
- `cargo xtask ci` lints `drift-app --lib --features smoke-env` for both iOS targets. The full
  Xcode build is not part of the gate (it needs `cargo tauri ios build`, minutes long).
- Two shells must keep the same function set; a missing function on one side fails that
  target's clippy step.
