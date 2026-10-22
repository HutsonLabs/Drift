# RESP-2 — Touch, platform and compact layout in the web UI

- Status: accepted
- Date: 2026-10-22
- Task: RESP-2 (`docs/responsive/tasks.md`); findings F8, F9, F11, F19, F20 of
  `docs/responsive/findings.md`; builds on ADRs `IOS-5-ios-app-shell` and `IOS-8-iphone-layout`

## Context

One stylesheet serves the Mac app and the iOS app. It had no width- or pointer-based rules and
nothing told a page which app it ran in, so the iPhone got the Mac's title-bar toolbar (88 points
kept for traffic lights), hover-only actions and 24–30-point controls.

## Decisions

1. **Platform.** `ui/src/platform.ts` sets `data-platform="ios"` or `"macos"` on `<html>` before
   a page's controller starts. It reads the user agent: iPhone/iPad/iPod, or a Mac user agent with
   more than one touch point (iPadOS's WKWebView reports a Mac user agent; a Mac's WKWebView has no
   touch points). This is used only for facts about the app shell: the traffic-light space, and
   the native switcher button over session pages (IOS-11).
2. **Touch sizing follows the pointer, not the platform.** Rules for touch go under
   `@media (pointer: coarse)`: 44-point targets, 16-pixel form controls (WebKit zooms in on
   smaller ones). Keyboard hints hide under `(any-hover: none)`: an iPad with a trackpad or mouse
   attached keeps them, which is the usual sign of a hardware keyboard too.
3. **Compact layout.** The configuration sheet (`.cfg`, container `cfg`) and the session sheets
   (`.sheet`, container `sheet`) are inline-size containers, so a sheet can no longer grow wider
   than its container because of unbreakable content. The configuration sheet's rows switch to one
   column below 560 points (`@container cfg (max-width: 559px)`), so an iPad split view gets the
   phone layout; the Mac's sheet is never that narrow (Connections `minWidth` 640). Session sheets
   are 380–500 points wide on the Mac too, so they switch only below 360 points
   (`@container sheet (max-width: 359px)`), which only narrow iOS widths reach. The page-level toolbar and the sheet's presentation use the viewport
   (`@media (max-width: 599px)`).
4. **Safe areas stay with WKWebView.** The pages keep `width=device-width` without
   `viewport-fit=cover`, so WKWebView's automatic content insets keep content out of the notch and
   home indicator. No `env(safe-area-inset-*)` padding is needed; the native frames (`phone`,
   `present::hud_frame`) already account for the insets.
5. **The breakpoint harness is the test.** happy-dom has no layout, so layout rules are tested by
   `ui/responsive` (RESP-1) in WebKit at B1–B10, in the merge gate.

## Consequences

- The Mac's look doesn't change: it has a fine pointer, a hover-capable pointer and widths
  ≥ 640 points (`minWidth` of the Connections window).
- A Mac with a touch screen would not exist today; if one did, it would get the touch sizes,
  which is the right outcome.
