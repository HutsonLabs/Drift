# Drift — iOS client, Mac host and App Store

Tasks for ADR `docs/adr/IOS-0-ios-client-mac-host-app-store.md`. Same rules as the rest of
Drift: one branch per task (`task/<ID>-<slug>`), Red tests first, `cargo xtask ci` green before
merge, and an ADR for any decision the IOS-0 ADR does not already fix.

Legend: `[ ]` pending · `[~]` in progress · `[✓]` landed

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
- [✓] **IOS-9 Pairing UI.** Bonjour list of Drift Hosts, QR scan or typed code, pinned host key
  stored in the Keychain (pairs with HOST-8). *The handshake landed with HOST-8 (ADR
  `HOST-8-pairing`) and runs end to end on loopback; the camera and a real device are manual
  steps in `docs/acceptance.md`.*
- [✓] **IOS-10 Clipboard on iOS.** UIPasteboard backend for `drift-clipboard`; paste only on an
  explicit user action, so iOS never shows its paste prompt unasked.
- [✓] **IOS-11 Session switcher.** On iOS, the UI-windows "one window per connection" becomes an
  in-app switcher (or scenes, if IOS-1 says Tauri allows them).

## HOST — Drift Host (Mac)

- [✓] **HOST-1 Server spike.** `ironrdp-server` with `egfx` streams a static AVC420 frame to
  Drift's own client on loopback. Record what the server lacks for AVC444, DISP and CLIPRDR
  (ADR open question 4).
- [~] **HOST-12 UDP transport.** Runs straight after HOST-1 (ADR decision 6). Measure
  `ironrdp-rdpeudp`'s lossy mode for GFX on lossy Wi-Fi (ADR open question 6). If it holds up,
  ship it. If not, write an ADR choosing a QUIC library and carry RDP PDUs over QUIC between
  Drift ends: reliable streams for input, clipboard and DVC, and one stream (or datagrams) per
  GFX frame. The client tries UDP first with a paired Drift Host and falls back to TCP.
  *Measured and decided (ADR `HOST-12-quic-transport`): no lossy RDP-UDP in the vendored tree;
  reliable RDP-UDP2 collapses at 0.5 % loss; QUIC with BBR holds. `drift-transport` has the QUIC
  session (pinned host, stream per frame) and the QUIC-then-TCP order. Wired end to end after
  HOST-8: Drift Host listens on UDP on its RDP port, and a paired profile connects over QUIC
  first and falls back to TCP (tested through a 2 % lossy link). Left: GFX still rides the main
  stream; one stream per frame needs the client to reorder frames and ask for a keyframe when
  one is lost, because AVC P-frames depend on the previous frame (ADR amendment).*
- [✓] **HOST-2 Host skeleton.** New `drift-host` crate and binary: TLS listener, one session,
  fast-path input logged only. Menu-bar shell later (HOST-9).
  *The screen, pointer and injection are wired in (ADR `HOST-pipeline`); the test pattern stays
  as the fallback without Screen Recording.*
- [~] **HOST-3 Capture.** New `drift-capture` crate: ScreenCaptureKit `SCStream` per display,
  IOSurface frames, dirty rects, cursor shape and position reported separately.
  *`drift-capture` built and tested (ADR `HOST-3-capture`); the pointer sampler ran for real, but
  this Mac has no Screen Recording grant, so a live stream is a manual step in `docs/acceptance.md`.*
- [✓] **HOST-4 Real-time encoder.** Extend `drift-video::encode` with a real-time profile (no
  reordering, low-latency rate control, bounded keyframe interval) and an AVC444 mode. Measure
  60 fps at 4K on M1 (ADR open question 5). *Measured on an M4 Pro (ADR `HOST-4-realtime-encoder`): 4K60
  AVC420 yes; AVC444 60 fps up to about 1080p, about 25 fps at 4K. M1 not measured.*
- [✓] **HOST-5 Frame mailbox.** One-slot newest-frame mailbox between capture and encode, and
  between encode and the socket. Stats for dropped frames.
- [✓] **HOST-6 Input injection.** Inverse of `drift-input::keymap` (set-1 scancodes → `kVK_*`),
  mouse and wheel through `CGEventPost`. One shared table and round-trip tests.
- [✓] **HOST-7 Link hygiene.** Interactive-video service class on the socket; idle heartbeat to
  keep a phone's Wi-Fi radio awake; backlog detection feeding EXT-4.
  *ADR `HOST-7-link-hygiene`; the effect on a phone over Wi-Fi is a manual measurement.*
- [✓] **HOST-8 Pairing and pins.** Code shown on the Mac, device key stored, host certificate
  pinned on the device. Pairing on by default; listener refuses unpaired devices.
- [~] **HOST-9 Menu-bar app.** Status, paired devices, permissions check (Screen Recording,
  Accessibility, Local Network) with plain-language first-run guidance. *`drift-host-app` and `cargo xtask host-bundle` (ADR
  `HOST-9-menu-bar`). The paired-devices submenu lists HOST-8's paired devices ("Pairing is off"
  without pairing); looking at the real menu and alert is a manual step.*
- [~] **HOST-10 Virtual display.** `CGVirtualDisplay` created and resized from Display Control
  layouts; launch self-test; fallback to the physical display. *Planning, the private-API
  module with its self-test, the display manager and the physical fallback are in (ADR
  `HOST-10-virtual-display`); a virtual display was created and removed on the dev Mac. The live
  picture switches capture to the virtual display at its mode's size (ADR `HOST-pipeline`,
  "Virtual displays and AVC444"); a real client resize is manual.*
- [~] **HOST-11 Clipboard and audio.** CLIPRDR server role reusing `drift-clipboard`'s mapping;
  `ironrdp-rdpsnd` fed from ScreenCaptureKit audio.
  *Both directions of the clipboard and the host's sound to Drift's client work over loopback
  (ADR `HOST-11-clipboard-and-audio`); the client plays it through `AVAudioEngine`. Live system
  audio needs Screen Recording and the real pasteboard a person: manual steps in
  `docs/acceptance.md`.*
- [ ] **HOST-13 Login window.** LaunchDaemon + per-session LaunchAgent + LoginWindow agent, so a
  paired device reaches the Mac after a restart.
- [ ] **HOST-14 Extra paths.** USB cable link and peer-to-peer Wi-Fi alongside the LAN; remote
  access over the user's own VPN.

## EXT — `Drift::Mac::v1` channel

- [✓] **EXT-1 Channel framing.** Versioned, length-delimited DVC messages with a fuzz target
  (M9-2 style). Client and host negotiate and fall back to plain RDP when absent.
- [✓] **EXT-2 Precise scroll and gestures.** Scroll with phase and momentum; swipe and pinch
  mapped on the host to Mission Control, App Exposé and Spaces shortcuts. *ADR
  `EXT-2-scroll-gestures`; the live Mac behaviour is a manual step in `docs/acceptance.md`.*
- [ ] **EXT-3 Menu mirroring.** Host reads the frontmost app's menu bar through Accessibility;
  iPad shows it in the iPadOS 26 menu bar and chooses items remotely.
- [✓] **EXT-4 Quality hints.** Host reports backlog and suggests a quality; a separate default
  quality for devices away from the local network. *ADR `EXT-4-quality-hints`; a real slowed
  link is a manual step in `docs/acceptance.md`.*
- [ ] **EXT-5 HEVC and HDR.** After HOST-4 measures AVC444: a Drift extension codec (HEVC Main 10,
  4:4:4 where VideoToolbox allows it, HDR metadata) negotiated over the channel, encoded by
  VideoToolbox on the host and decoded on the client, with AVC444 as the fallback (ADR decision 2).

## STORE — Distribution

- [ ] **STORE-1 Mac App Store packaging.** Provisioning profile, application/team identifier
  entitlements, universal build, `ITSAppUsesNonExemptEncryption` and export-compliance answers.
  Update `bundle.shortDescription` for GNOME and Mac hosts.
- [ ] **STORE-2 Review-risk check.** Upload a build to App Store Connect and validate it,
  including the transparent webview (ADR open question 7). Plan native overlays if it fails.
- [ ] **STORE-3 iOS App Store packaging.** `NSLocalNetworkUsageDescription`,
  `NSBonjourServices`, privacy manifest, TestFlight build.
- [ ] **STORE-4 Drift Host release.** Developer ID signing, notarization and DMG through the
  M9-5 release path; Hardened Runtime without the App Sandbox.
