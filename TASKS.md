# Drift — iOS client and App Store

Tasks for ADR `docs/adr/IOS-0-ios-client-mac-host-app-store.md` (its iOS client half) and ADR
`docs/adr/GNOME-only.md`. Drift for Mac, iPhone and iPad connects to GNOME Remote Desktop;
Drift Host is retired. Same rules as the rest of Drift: one branch per task
(`task/<ID>-<slug>`), Red tests first, `cargo xtask ci` green before merge, and an ADR for any
decision the ADRs do not already fix.

Legend: `[ ]` pending · `[~]` in progress · `[✓]` landed

## GNOME-only

- [✓] **GNOME-only ADR.** Drift connects to GNOME only; Drift Host is retired (ADR `GNOME-only`).
  Superseded ADRs moved to `docs/adr/superseded/`.
- [✓] **Remove Drift Host and host-only client code.** The six host crates, `host-bundle`,
  `host-release`, `--verify-reproducible` and the Host DMG; pairing, target kinds, quality
  hints, menu mirroring, HEVC/AVC444 and gestures in the client. Old Mac profiles are dropped on
  load.
- [✓] **`drift-connect:` links.** Parsed in `drift_core::link`; opening one shows a prefilled New
  Connection form, nothing is saved until Save. Replaces `drift-pair:` in both apps.
- [✓] **`host/drift-gnome-setup`.** Sets up a Linux machine with GNOME from the Mac over SSH
  (install, headless session, firewall, fingerprint) and opens the prefilled connection in
  Drift, with a QR code for iPhone and iPad (`docs/gnome-host-setup.md`). *The run against a
  real machine is a manual step in `docs/acceptance.md`.*

## IOS-0 — Decision

- [✓] **IOS-0 ADR.** Write and accept `IOS-0-ios-client-mac-host-app-store`; add it to the ADR
  table in `docs/team-conventions.md`.

## IOS — Drift for iPhone and iPad (client)

- [✓] **IOS-1 Tauri mobile spike.** `tauri ios init` on a throwaway branch. Answer the ADR's open
  questions 1–2: can a plugin set `prefersPointerLocked` on the root view controller, and does
  Tauri mobile support more than one window or scene? Write the answers into the ADR.
- [✓] **IOS-2 Platform gating.** `cfg(target_os)` gates on every AppKit use. `drift-core`,
  `drift-rdp`, `drift-gfx`, `drift-codec`, `drift-input`, `drift-video` and `drift-render` build
  for `aarch64-apple-ios` and `aarch64-apple-ios-sim`. Add both targets to `cargo xtask ci`.
- [✓] **IOS-3 Split `drift-macos`.** New `drift-apple` (Keychain, `NWPathMonitor`, Local Network
  detection) shared by `drift-macos` (AppKit) and a new `drift-ios` (UIKit). No behaviour change
  on macOS.
- [✓] **IOS-4 Remote view on iOS.** `drift-ios` hosts the `CAMetalLayer` in a `UIView` under the
  `WKWebView` (M7-3 layering). Drawable size tracks the view and the screen scale; ProMotion
  pacing at up to 120 Hz.
- [~] **IOS-5 First light on iPad.** The iOS app connects to the GNOME test host (headless
  mode), shows a live desktop, and reconnects after backgrounding. *Done in the iPad simulator
  against `fake_host` (live picture, reconnect while backgrounded); the GNOME host was down, so
  the device run is in `docs/acceptance.md`.*
- [~] **IOS-6 Input plugin.** Swift Tauri plugin: `GCKeyboard` raw key codes, `GCMouse` deltas
  with pointer lock, `UIPointerInteraction`, and Apple Pencil hover as pointer movement, all fed to
  `drift-input`. Document which system shortcuts iPadOS keeps for itself. *Done in Rust in
  `drift-ios` instead of Swift (ADR `IOS-6-ios-input`); smoke-tested in the simulator, which
  cannot synthesize a keyboard, mouse or Pencil, so those checks are in `docs/acceptance.md`.*
- [✓] **IOS-7 Touch modes.** Trackpad mode (relative pointer, two-finger scroll, tap to click,
  two-finger tap for right click) and direct mode (tap where you touch). Pinch zooms the viewport
  on iPhone.
- [✓] **IOS-8 iPhone layout.** Portrait: picture on top, trackpad and key row below. Landscape:
  full screen with auto-hiding controls.
- **IOS-9 Pairing UI.** Retired by ADR `GNOME-only` (no Drift Host to pair with).
- [✓] **IOS-10 Clipboard on iOS.** UIPasteboard backend for `drift-clipboard`; paste only on an
  explicit user action, so iOS never shows its paste prompt unasked.
- [✓] **IOS-11 Session switcher.** On iOS, the UI-windows "one window per connection" becomes an
  in-app switcher (or scenes, if IOS-1 says Tauri allows them).
- **UI-kinds Mac and GNOME targets.** Retired by ADR `GNOME-only` (one kind of target). GNOME
  menus in the iPad menu bar and the iPhone key row stay.

## HOST — Drift Host (Mac)

Retired by ADR `GNOME-only`: Drift Host is gone, and with it this section's tasks. The code is in
git history at commit `ce6c578`; the ADRs are in `docs/adr/superseded/`.

## EXT — `Drift::Mac::v1` channel

Retired by ADR `GNOME-only`: Drift Host is gone, and with it this section's tasks. The code is in
git history at commit `ce6c578`; the ADRs are in `docs/adr/superseded/`.

## STORE — Distribution

- [✓] **STORE-1 Mac App Store packaging.** Provisioning profile, application/team identifier
  entitlements, universal build, `ITSAppUsesNonExemptEncryption` and export-compliance answers.
  Update `bundle.shortDescription` ("Remote desktop for GNOME computers"). *`cargo xtask
  mas-package` builds a universal, sandboxed, Apple Distribution-signed app in an installer-signed `.pkg` (ADR
  `STORE-1-mac-app-store`). Export compliance: `ITSAppUsesNonExemptEncryption = false`
  (standard, mass-market encryption; exempt). Build 26.10 (3) is on TestFlight for macOS
  (`cargo xtask release --testflight`, always with iOS).*
- [~] **STORE-2 Review-risk check.** Upload a build to App Store Connect and validate it,
  including the transparent webview (ADR open question 7). Plan native overlays if it fails.
  *The Mac package with wry's private `drawsBackground` key passed App Store Connect validation
  and is on TestFlight (26.10 (3)); validation does not scan for private API, so whether App
  Review accepts it is still open until a build is submitted for review. The native-overlay
  plan is in ADR `STORE-2-review-risk`, and iOS drops the key (STORE-3).*
- [✓] **STORE-3 iOS App Store packaging.** `NSLocalNetworkUsageDescription`,
  `NSBonjourServices`, privacy manifest, TestFlight build. *Usage strings, privacy manifest and
  App Store export options are in, iOS no longer carries wry's private transparency key, and the
  900 MB `libapp.a` is no longer bundled (ADR `STORE-3-ios-app-store`). Builds 0.2.0 (1) and
  26.10 (2) passed validation and are on TestFlight in the internal group "Drift Internal"
  (2026-10-18); `cargo xtask release --testflight` ships the next one, together with macOS.
  `NSBonjourServices` went with Drift Host (ADR `GNOME-only`).*
- **STORE-4 Drift Host release.** Retired by ADR `GNOME-only` (no Drift Host DMG).

- [✓] **RELEASE Release pipeline.** One command from a clean `main` to every artifact, resumable,
  with a manifest; publishing only with `--testflight` / `--spaces`. *`cargo xtask release`,
  notes in `release/notes/`, skill `.claude/skills/release`, ADR `RELEASE-pipeline`;
  supersedes the M9-5 `bundle` stub.*
- [✓] **APP-icon Icon Composer app icon.** The owner's `app.icon` is the one icon for the Mac and
  iOS apps. *`src-tauri/icons/AppIcon.icon`, bundled by Tauri into `Assets.car` and added to
  the iOS target; the flat `icns` and PNGs are rendered from it (ADR `APP-icon`).*
- **HOST-icon Drift Host app icon.** Retired by ADR `GNOME-only` (`APP-icon` decision 6).

## RESP — Mobile responsiveness

The iPhone and iPad web UI audited at 10 breakpoints (320 pt to 1376 pt, phone landscape, iPad
split). The findings are in `docs/responsive/findings.md` and the remediation tasks RESP-1 to
RESP-17 in `docs/responsive/tasks.md`.

- [✓] **RESP-0 Breakpoint audit.** *20 findings, 7 of them P1 (the configuration sheet on phones,
  the toolbar, tap to connect, and session sheets in phone landscape and at 320 pt); F12 later
  corrected to P3.*
- [✓] **RESP-1–17 Remediation.** *The breakpoint harness is in the gate and passes; device checks in
  `docs/acceptance.md` § RESP.*
