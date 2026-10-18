# STORE-2 — App Store review risk: the transparent webview

- Status: accepted (plan); validation blocked on the developer account
- Date: 2026-10-18
- Task: STORE-2 (`TASKS.md`); ADR `IOS-0` open question 7, ADR `M7-3`

## What was run

- `xcrun altool --validate-app -t macos` on the STORE-1 package (validate only, nothing
  uploaded) stops before any check of the app: `Unable to find Apple ID for Bundle ID
  'com.hutsonlabs.drift' on platform 'macOS App'. Either create this app in App Store Connect
  first … (19)`. App Store Connect validates against an app record, and there is none yet
  (STORE-1). So open question 7 is still open.
- `cargo xtask review-risk <Drift.app>` (new; `xtask::store::review_risks`) scans the built
  executables for private API strings. It finds one: `drawsBackground`. wry 0.55 sets that
  private WKWebView key by KVC whenever a webview is transparent (`wkwebview/mod.rs`, feature
  `transparent`, enabled through Tauri's `macos-private-api`) — on macOS *and* iOS. Drift's
  session windows are transparent so overlays and HUDs draw over the picture (M7-3).

## Decisions

1. **iOS does not need the private key.** On iOS a `WKWebView` becomes see-through with public
   API (`opaque = NO`, clear background colours), which `drift-ios` already applies (IOS-4). So
   the iOS build stops enabling Tauri's `macos-private-api` (STORE-3), and the scan must find
   nothing in the iOS app.
2. **macOS keeps it for now.** There is no public way to make a `WKWebView` transparent on
   macOS. The Developer ID build is unaffected. For the App Store build we wait for the first
   real validation and review.
3. **If App Review rejects the Mac app**, the Mac App Store build turns transparency off and
   draws the three things that sit over the picture natively:
   - the reconnect overlay (dimmed last frame + status): a `CALayer` dim over the
     `RemoteView` plus an `NSVisualEffectView` panel with the status text and the Reconnect /
     Cancel buttons, driven by the same `SessionView`;
   - the greeter hint and statistics HUDs: `NSVisualEffectView` panels with `NSTextField`s at
     `present::hud_frame`;
   - the page itself is then only shown full-window (forms, prompts), where it is opaque.
   A `mas` cargo feature on `drift-app` would select this; the Developer ID build keeps the
   web overlays. Nothing of this is built until a rejection makes it necessary.

## Consequences

- The account owner must create the App Store Connect record (STORE-1) before validation can
  answer open question 7; `docs/acceptance.md` lists the run.
- `cargo xtask review-risk` runs on any built app and documents why each hit matters.
