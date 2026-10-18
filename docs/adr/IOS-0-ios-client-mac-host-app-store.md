# IOS-0 — Drift on iPhone and iPad, a Mac host, and App Store distribution

- Status: accepted
- Date: 2026-10-17
- Amended: 2026-10-17. Decision 6 now treats UDP as a first-class transport, with QUIC as the
  fallback for Drift-to-Drift sessions; decision 2 plans HEVC and HDR (EXT-5). The goal is parity
  with Screen Sharing's High Performance mode, not only a working host.
- Tasks: `TASKS.md` (IOS-*, HOST-*, EXT-*, STORE-*)
- Code today: `crates/*`, `src-tauri/`, `third_party/ironrdp/crates/{ironrdp-server,ironrdp-egfx,ironrdp-rdpeudp,ironrdp-rdpsnd,ironrdp-cliprdr,ironrdp-displaycontrol}`
- Prior art reviewed: macOS Screen Sharing (High Performance mode); Sill
  (`github.com/Saffsanity/sill`, Apache-2.0, Swift, reviewed 2026-10-17). **No Sill code is
  copied**; only ideas are taken, and every one is reimplemented here from its description.

## Context

macOS Screen Sharing is the reference experience between two Macs: a system host, a virtual
display sized to the viewer, 60 fps, 4:4:4 chroma, HDR, stereo audio, and access at the login
window (High Performance mode: Apple silicon at both ends, macOS 14+, UDP 5900–5902, about
75 Mbps for one 4K display). Apple ships no client for iPhone or iPad, and the High Performance
protocol is private, so a third-party client cannot speak it. Getting that experience on an iPad
means shipping **both ends**: a host on the Mac and a client on the device.

Drift today is a sandboxed Tauri 2.11.6 app for macOS that speaks RDP to GNOME Remote Desktop.
It already has the hard client half: the GFX pipeline including AVC420 and AVC444
(`drift-gfx`, `drift-video`), CPU codecs (`drift-codec`), a Metal compositor (`drift-render`),
keyboard/scroll translation (`drift-input`), CLIPRDR text + images (`drift-clipboard`),
display-control resize (M4), and reconnect (M7). The vendored IronRDP tree also carries the
server halves: `ironrdp-server` (TLS, fast-path input, bitmap updates; `egfx` optional),
`ironrdp-egfx/src/server.rs` with AVC PDUs, `ironrdp-rdpeudp` (sans-I/O MS-RDPEUDP2),
`ironrdp-rdpsnd`, `ironrdp-cliprdr` and `ironrdp-displaycontrol`.

Sill is the closest open prior art for "Mac on iPhone/iPad". It confirms several choices below
(ScreenCaptureKit, VideoToolbox, `CGEventPost` with scroll phases, private `CGVirtualDisplay`,
host outside the Mac App Store, client in the App Store) and has gaps Drift is positioned to
fill: TCP only, 4:2:0 HEVC Main, no audio, text-only clipboard, no login window, one window or
desktop at a time.

Facts checked for this ADR:

- Tauri 2 builds and uploads iOS apps (`tauri ios build --export-method app-store-connect`) and
  Mac App Store packages (sandbox entitlement, provisioning profile, `productbuild` .pkg).
- Tauri's config reference says `macOSPrivateApi` "enables the transparent background API" and is
  a "no-op in Tauri 2.12.1+ because the APIs are always enabled now".
- Drift's own code calls no private selectors (searched `crates/*/src`, `src-tauri/src`). Its
  only private-API exposure is wry's transparent webview, which ADR `M7-3` turned on.
- `src-tauri/entitlements.plist` already enables the App Sandbox with `network.client` only.
- No crate has a single `cfg(target_os)`; everything assumes macOS.

## Decisions

### 1. One product, three deliverables

| Deliverable | Tech | Distribution |
|---|---|---|
| **Drift for Mac** (client) | today's app | Mac App Store **and** Developer ID DMG |
| **Drift for iPhone and iPad** (client) | the same Tauri app, iOS target | App Store |
| **Drift Host** (Mac host) | new Rust binary + menu-bar app | Developer ID, notarized DMG only |

The clients stay one Tauri codebase. The host is a separate bundle because it cannot be
sandboxed (decision 9).

### 2. RDP is the wire protocol for every host

Drift Host speaks RDP, built on the vendored `ironrdp-server` + `ironrdp-egfx`. Reasons:

- Drift's client pipeline (GFX, AVC444, CLIPRDR, DISP, reconnect) is reused unchanged against a
  Mac, so one client serves GNOME and Mac hosts.
- AVC444 gives full-chroma text with H.264 on hardware every Apple device can decode, which is
  the main visible quality gap in Sill.
- Any RDP client (Microsoft's Windows App on iPad, `mstsc`) can test the host before Drift's iOS
  client exists, and remains a fallback for users.

HEVC and HDR are not part of RDP GFX, but Screen Sharing has both, so they are planned rather than
left out: a Drift extension codec (HEVC Main 10, 4:4:4 where VideoToolbox allows it, HDR
metadata) negotiated over the `Drift::Mac::v1` DVC between Drift ends (EXT-5). AVC444 stays the
default and the fallback for every other client. EXT-5 starts once AVC444 is measured (HOST-4).

### 3. Mac-specific features ride a Drift dynamic virtual channel

Plain RDP cannot express several things that make a Mac feel native. A DVC named
`Drift::Mac::v1` carries them when both ends are Drift; anything else falls back to plain RDP:

| Message | Why plain RDP is not enough | Fallback |
|---|---|---|
| Scroll with phase + momentum | RDP wheel events have no phase, so momentum and rubber-banding are lost | wheel units, as `drift-input::scroll` does today |
| Trackpad gestures (swipe, pinch, smart zoom) | no RDP equivalent; public CGEvent cannot synthesize them | host maps to Mission Control / Spaces keyboard shortcuts |
| Menu mirroring | lets iPadOS 26's menu bar show the remote app's menus (idea from Sill) | none |
| Host status (link backlog, encoder state, suggested quality) | RDP has no "you are falling behind" signal | none |
| Pairing handshake (decision 7) | RDP authenticates users, not devices | NLA password |

Messages are versioned, length-delimited and fuzzed like the GFX parser (M9-2).

### 4. Host capture, encode and input reuse Drift crates

- **Capture:** ScreenCaptureKit `SCStream` per display (IOSurface-backed frames, dirty rects,
  cursor reported separately, system audio). New crate `drift-capture`.
- **Encode:** extend `drift-video::encode` (today the VideoToolbox H.264 encoder for recording,
  `EncoderConfig`) with a real-time profile: no frame reordering, low-latency rate control,
  bounded keyframe interval, and an AVC444 mode that emits the luma and chroma streams GFX
  expects. Encoder input is a one-slot mailbox that always keeps the newest frame and drops older
  ones rather than queueing (idea from Sill).
- **Input injection:** `CGEventPost` at the HID tap, needing Accessibility. Key translation is
  the inverse of `drift-input::keymap` (set-1 scancodes → `kVK_*`), so both directions share one
  table and its tests. Scroll injection uses pixel-unit scroll events with phase and momentum
  fields when the DVC supplies them.
- **Clipboard:** `drift-clipboard`'s format mapping is direction-agnostic; the host reuses it with
  `ironrdp-cliprdr`'s server role.
- **Audio:** `ironrdp-rdpsnd` server, fed from ScreenCaptureKit audio (later task; Sill has none).

### 5. A virtual display sized to the device

The client already sends Display Control monitor layouts (M4-1/M4-2). Drift Host answers each
layout by creating or resizing a `CGVirtualDisplay` at that size and scale and capturing it, so
an iPad gets its own native resolution instead of a scaled Mac desktop. `CGVirtualDisplay` is a
private CoreGraphics API, accessed through the Objective-C runtime and isolated in one module with
a self-test at launch; when it is unavailable the host falls back to capturing the physical
display and the client letterboxes (today's `ScaleMode::Fit`).

### 6. Transport: UDP is first-class, TCP is the baseline

Screen Sharing's High Performance mode runs over UDP, and on lossy Wi-Fi a TCP stream stalls
every frame behind one lost packet (head-of-line blocking). Drift Host therefore treats UDP as a
first-class transport from its first release, not a later follow-up:

- **Baseline: TLS over TCP.** This is the path the client already has. It stays the only
  transport for GNOME hosts and for third-party RDP clients, and the fallback whenever UDP fails.
- **First choice for UDP: `ironrdp-rdpeudp`** (MS-RDPEUDP2, lossy mode for GFX). It is standard
  RDP, so any RDP client with UDP support can use it.
- **Fallback for UDP: QUIC, between Drift ends only.** If HOST-12 finds `ironrdp-rdpeudp`'s lossy
  mode unusable, a paired Drift client and Drift Host carry the same RDP PDUs over QUIC instead.
  Input, clipboard and DVC traffic use reliable streams; each GFX frame goes on its own stream
  (or in datagrams), so a lost packet delays only its own frame. QUIC's TLS 1.3 uses rustls, and
  the pins from decision 7 apply unchanged. The client tries QUIC first with a paired Drift Host
  and falls back to TCP. The QUIC library is chosen in its own ADR under the dependency policy
  (`M0-6-dependency-policy`).
- **Ordering:** HOST-12 runs straight after the server spike (HOST-1), before the encoder is
  tuned. Loss and latency behaviour shape real-time rate control (HOST-4) and quality hints
  (EXT-4).

On every transport, the `Network.framework` service class is set to interactive video (WMM video
queue on Wi-Fi), and the host sends a small heartbeat when idle so a phone's Wi-Fi radio does not
enter power save between frames (both ideas from Sill).

### 7. Devices pair once and pin keys

On first contact over the local network the host shows a short code (QR + digits); the device
sends it with its public key, and the host stores the device key while the device pins the host
certificate. Later connections, local or remote, are refused unless both pins match. This
replaces trust-on-first-use (`M1-1`) for Mac hosts only; GNOME profiles keep TOFU. Pairing runs
over the `Drift::Mac::v1` DVC after TLS. The idea is from Sill; the protocol is ours.

### 8. The iOS client is the existing Tauri app with a new platform crate

- **Gate first:** add `cfg(target_os = "macos")` / `"ios"` to every AppKit use, so the
  platform-neutral crates (`drift-core`, `drift-rdp`, `drift-gfx`, `drift-codec`, `drift-input`)
  and the Apple-framework crates (`drift-video`, `drift-render`) build for `aarch64-apple-ios`
  and `aarch64-apple-ios-sim`.
- **Split `drift-macos`:** a new `drift-apple` holds what is common (Keychain, `NWPathMonitor`,
  Local Network detection); `drift-macos` keeps AppKit; a new `drift-ios` holds UIKit.
- **Session view on iOS:** a `UIView` hosting the `CAMetalLayer` sits under the `WKWebView` in
  the same superview, the same layering ADR `M7-3` uses on macOS, so overlays and the HUD port as
  web code.
- **Input on iOS:** a Swift Tauri plugin (`tauri-plugin-drift-input`) reads `GCKeyboard` raw key
  codes, `GCMouse` deltas under pointer lock, `UIPointerInteraction`, touches and Apple Pencil
  hover, and forwards normalized events to `drift-input` through the existing command layer.
  Touch has two modes: trackpad (relative) and direct (absolute tap-to-click).
- **Windows:** UI-windows' one window per connection becomes one in-app session switcher on iOS.
  IOS-1 showed that Tauri mobile can open a window per `UIWindowScene` on iPad, so on iPad a
  session may also move to its own scene (IOS-11). iPhone has one scene, so the switcher stays.
- **iPhone layout:** portrait puts the picture above an on-screen trackpad and key row
  (idea from Sill); landscape is full screen.

### 9. App Store eligibility

- **Mac client:** eligible. It is already sandboxed with `network.client` only. Remaining work
  is packaging: Mac App Store provisioning profile, application/team identifier entitlements,
  universal build, `ITSAppUsesNonExemptEncryption` and the export-compliance answers (Drift ships
  its own TLS via rustls and NTLM/CredSSP, not only OS crypto). The transparent webview from
  `M7-3` is a review risk; we validate an App Store Connect upload early (STORE-2), and if review
  objects the overlays move to native layers.
- **iOS client:** eligible. Generic remote-desktop clients are approved today (Windows App, Jump
  Desktop, Sill). Guideline 4.2.7's limits apply only to apps that mirror *specific* software.
  Needs `NSLocalNetworkUsageDescription`, `NSBonjourServices` and a privacy manifest.
- **Drift Host:** not eligible, and not attempted. It uses a private API (decision 5), needs a
  login-window agent (decision 10), and injects input into other apps. It ships as a notarized
  Developer ID DMG through the existing M9-5 release path, like Sill's host.

### 10. Login window and lock screen are in scope, after the first light

The host is split into a LaunchDaemon (listener, pairing store, session broker) and a per-session
LaunchAgent (capture, encode, inject), with a second agent that loads in the `LoginWindow`
session. This is what lets Drift reach a Mac after a restart, which Sill cannot do. It lands after
the logged-in path works (HOST-10). Curtain mode is not available to third parties and is out of
scope.

## Ideas taken from Sill (reimplemented, no code copied)

| Idea | Where it lands |
|---|---|
| Keep only the newest frame; drop rather than queue | decision 4, HOST-5 |
| Interactive-video service class and an idle heartbeat for phone Wi-Fi | decision 6, HOST-7 |
| One-time pairing with a code, then pinned keys | decision 7, HOST-8 / IOS-9 |
| Host notices backlog and suggests a lower quality; a separate default quality when away | decision 3, EXT-4 |
| Mac menus in the iPadOS 26 menu bar | decision 3, EXT-3 |
| iPhone portrait: picture above a trackpad and keys | decision 8, IOS-8 |
| USB cable and peer-to-peer Wi-Fi as extra paths besides the LAN | later; HOST-14 |

## IOS-1 spike notes

Run on an iPad Pro 13-inch (M5) simulator, iOS 27.0, Xcode 27.0, Tauri 2.11.6 (tao 0.35.3,
wry 0.55.1), tauri-cli 2.11.4.

- `cargo tauri ios init --ci` generates `gen/apple` with xcodegen. The Rust crate is built as a
  `staticlib` (`libapp.a`) by a pre-build script (`cargo tauri ios xcode-script`) and linked
  into a small `main.mm` that calls `ffi::start_app()`. The lib needs
  `#[cfg_attr(mobile, tauri::mobile_entry_point)]` on `run()`.
- Xcode 27 rejects the generated iOS 14.0 deployment target (supported range 15.0 to 27.0).
  Set `bundle.iOS.minimumSystemVersion` in `tauri.conf.json` and rerun `xcodegen generate` in
  `gen/apple` (or re-init).
- A lone `tauri` dependency resolved a newer `tauri-runtime` that does not compile with tauri
  2.11.6. The spike reused Drift's `Cargo.lock`; the real app gets this for free.
- Pointer lock: the method is added to the class, so it applies to every Tao view controller.
  Drift should return a per-instance value (an associated object or a lookup by pointer) so only
  a session view locks. The lock is applied asynchronously and drops when the scene goes
  inactive or to the background, and iPadOS only grants it to a full-screen scene. A Swift
  plugin could do the same through the Objective-C runtime from `load(webview:)`
  (`webview.window?.rootViewController`); there is no need to subclass.
- Scenes: tao sets its scene delegate in code, so the manifest only needs
  `UIApplicationSceneManifest` → `UIApplicationSupportsMultipleScenes = true`. UIKit logs a
  harmless "no UIScene configuration dictionary … named TaoScene" error. A new window's scene
  comes to the front and the previous one goes to the background. `RunEvent::SceneRequested`
  fires only for scenes the system opens (for example from the app switcher), not for ones Drift
  creates. Closing a scene from the system UI was not tested.
- The app's stdout reaches the unified log (category `app`, prefixed `[stdout]`), not
  `simctl launch --console-pty`.

## Consequences

- Drift stops being "a GNOME client" and becomes a client for GNOME and Mac hosts; `project.md`'s
  one-line description and the `bundle.shortDescription` change with STORE-1.
- Every platform crate gains `cfg` gates; CI gains iOS builds (`aarch64-apple-ios`,
  `aarch64-apple-ios-sim`) to keep them building.
- The host introduces a server role, a listener and a private API. It gets its own threat model:
  pairing is mandatory by default, the listener binds only after pairing is configured, and the
  daemon drops privileges for everything but session brokering.
- Drift-to-Drift sessions may run over QUIC, which no other RDP client speaks. Third-party RDP
  clients always get TCP (or RDP-UDP), so the host stays testable with them. The host listens on
  UDP as well as TCP, and its threat model and first-run firewall guidance cover both.
- `CGVirtualDisplay` can break with any macOS release; the launch self-test and the
  physical-display fallback keep the host usable when it does.
- Screen Recording and Accessibility must be granted at the Mac in person (or by MDM for
  Accessibility). First-run setup must say so plainly.

## Open questions (each has a task)

1. Does Tauri mobile allow replacing the root view controller's `prefersPointerLocked`? (IOS-1)
   **Answered: yes.** The root view controller is tao's `TaoUIViewController`, a class tao builds
   at runtime that does not override `prefersPointerLocked`. Adding the method with
   `class_addMethod` and calling `setNeedsUpdateOfPrefersPointerLocked` locks the pointer: UIKit
   logged `pointerLockState … isLocked: YES` while the scene was foreground active. Spike branch
   `spike/IOS-1-tauri-mobile`.
2. Does Tauri mobile support more than one window/scene? (IOS-1)
   **Answered: yes, on iPad.** With `UIApplicationSupportsMultipleScenes` in `Info.plist`,
   `WebviewWindowBuilder::build` connects a new `UIWindowScene` (connected scenes went from 1 to
   2, one window each). Same spike branch.
3. Do `objc2-io-surface` / `objc2-core-video` features used by `drift-render` build for iOS
   unchanged? (IOS-2)
4. How complete is `ironrdp-server` with `egfx` for AVC420/AVC444 streaming? (HOST-1)
5. Can VideoToolbox's real-time H.264 encoder produce AVC444's two streams at 60 fps on M1? (HOST-4)
6. Is `ironrdp-rdpeudp`'s lossy mode usable for GFX, or do Drift-to-Drift sessions need the
   QUIC fallback? (HOST-12)
7. Does App Store review accept wry's transparent webview? (STORE-2)
