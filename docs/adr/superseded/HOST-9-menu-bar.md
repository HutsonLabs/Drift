# HOST-9 — Drift Host's menu-bar app

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Task: HOST-9 (`TASKS.md`); follows ADR `IOS-0` decision 9 and ADR `HOST-2-host-skeleton`

## Context

`drift-host` (HOST-2) is a command. People need a Mac app: something that starts the host,
shows whether a device is connected, lists paired devices and explains the permissions the host
needs in words they understand. It ships outside the Mac App Store (Developer ID, not
sandboxed), as its own bundle.

## Decisions

1. **Native AppKit in Rust, not Tauri.** A menu-bar agent needs an `NSStatusItem` and an
   `NSMenu`, nothing a webview helps with. A new crate, `drift-host-app`, does it with `objc2`,
   like `drift-macos`; it links `drift-host` directly. No second Tauri app, no webview, no JS.
2. **Pure view model.** `drift_host_app::model` decides the status line, the menu (status,
   paired devices, missing permissions, Start at Login, Quit), the icon state and the permission
   guidance. `status_item` only turns `MenuEntry`s into `NSMenuItem`s (one target object maps an
   item's tag back to its `MenuAction`). The menu is rebuilt once a second only when the model
   changed.
3. **Status comes from the host's stats.** "Connected" means a session was admitted and has not
   ended. The device's name arrives with pairing (HOST-8); until then the line says "A device is
   connected".
4. **Paired devices behind a trait.** `PairedDevices::devices()` returns `None` without pairing
   support, and the menu says "Paired devices: none yet (pairing comes in a later version)".
   HOST-8 implements the trait over its device store.
5. **Permissions.**
   - Screen Recording: `CGPreflightScreenCaptureAccess`; the menu item asks once with
     `CGRequestScreenCaptureAccess` and opens Privacy › Screen & System Audio Recording.
   - Accessibility: `AXIsProcessTrusted`; the item calls `AXIsProcessTrustedWithOptions` with
     the prompt option (which also adds Drift Host to the list) and opens Privacy ›
     Accessibility.
   - Local Network: macOS has no query. It counts as unknown until a device has connected while
     the host listens beyond loopback; macOS shows its own prompt the first time.
   - The icon switches to a warning symbol while Screen Recording or Accessibility is missing or
     the host stopped. Unknown Local Network is no alarm.
6. **First run.** At launch, if Screen Recording or Accessibility is missing, an alert names the
   first one, says why in one sentence and lists numbered steps; "Open Settings" goes straight to
   the pane. Local Network is left to macOS's own prompt.
7. **Start at Login** uses `SMAppService.mainAppService` (`register`/`unregister`). It needs the
   app bundle; run as a bare binary the item reports the error in the log.
8. **Bundle.** `crates/drift-host-app/Info.plist` (`com.hutsonlabs.drift.host`, `LSUIElement`,
   Local Network usage text, `_drift-host._tcp` Bonjour type) and `cargo xtask host-bundle
   [--debug]` build `target/<profile>/bundle/Drift Host.app` with the app and the `drift-host`
   command inside, signed ad hoc. Developer ID signing, notarization and the DMG are STORE-4.
9. **Shared start-up.** `drift_host::launch` (config, Keychain password, identity, listener) is
   used by both the command and the app, so they cannot drift apart.

## Consequences

- The model is unit tested; the `NSMenu` mapping and creating/removing a status item have a
  main-thread smoke test. Looking at the real menu, the alert and the Settings round trip is a
  manual step (`docs/acceptance.md`); this session had no Screen Recording grant, so no
  screenshot.
- The plist's version must follow the workspace version; a test fails when they differ.
