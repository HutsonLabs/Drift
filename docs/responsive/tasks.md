# Mobile responsiveness — remediation tasks

These tasks fix the findings in [`findings.md`](findings.md) (F1–F20, breakpoints B1–B10). Same
rules as the rest of Drift (`docs/team-conventions.md`):

- one branch per task, `task/RESP-<n>-<slug>`;
- Red tests first, then `cargo xtask ci` green before merge;
- an ADR for any decision the existing ADRs don't already fix.

A task is done when its findings no longer show at the listed breakpoints in the RESP-1 harness,
and its device checks are in `docs/acceptance.md`.

Legend: `[ ]` pending · `[~]` in progress · `[✓]` done

All RESP tasks landed on 2026-10-22; the harness passes (172 page states, 0 failures). The device
checks are in `docs/acceptance.md` § RESP.

## Audit

- [✓] **RESP-0 Breakpoint audit.** Evaluate every page state of the iOS app's web UI at the 10
  breakpoints B1–B10 (320 pt to 1376 pt, phone landscape, iPad split). Inventory the findings and
  plan the fixes. *`docs/responsive/findings.md`, with screenshots in `docs/responsive/shots/`.
  F12 corrected from P1 to P3 on 2026-10-22 (see the finding).*

## Foundation

- [✓] **RESP-1 Breakpoint regression harness.** Land the audit's harness in the repo. It renders
  each state of `ConnectionsApp` and `SessionApp` from the `ui/test` fakes and loads it in
  Playwright WebKit (bun) at B1–B10, using the frames the iOS shell gives each page. It asserts:
  - no horizontal page overflow;
  - no interactive element outside the viewport;
  - every sheet's top and bottom can be reached;
  - touch targets ≥ 44 pt under `pointer: coarse`;
  - form controls ≥ 16 px.

  Run it from `cargo xtask` (a step in `ci_plan.rs` if WebKit can be provisioned in CI, otherwise a
  `cargo xtask responsive` command plus an acceptance step). Every later RESP task adds its cases
  to it as Red tests first.
  *`ui/responsive` (`bun run responsive`), a step of `cargo xtask ci`; also two Mac breakpoints that must stay clean.*
- [✓] **RESP-2 Platform and touch hooks (ADR `RESP-2-touch-platform`).** (F20, base for F8, F9,
  F11, F19)
  - Give the pages a `data-platform="ios|macos"` on `<html>`, set before first paint (a Tauri
    initialization script or a query parameter on the page URL).
  - Adopt `@media (pointer: coarse)` for touch sizing.
  - Decide safe areas: keep WKWebView's automatic insets, or `viewport-fit=cover` plus
    `env(safe-area-inset-*)` padding. Record the choice in the ADR.
  - Define the compact breakpoint the CSS uses (container width < 560 pt for sheets, viewport
    < 600 pt for the toolbar), and use container queries on `.cfg` and `.sheet` so iPad split
    views behave like phones.

  *`ui/src/platform.ts`, ADR `RESP-2-touch-platform`.*
## P1 — blocks use on a breakpoint

- [✓] **RESP-3 Single-column configuration fields on compact widths.** (F1; B1–B4, B6)
  - Below the compact breakpoint, `.group .field` stacks label, control, hint and error in one
    column, with full-width controls.
  - The Host / Port row wraps Port onto its own line, or gives Host at least 60 % of the width.
  - Red: Name, Host and RDP user are each at least 200 pt wide at 320, and placeholders aren't cut
    off.
- [✓] **RESP-4 Connection-type tiles that fit.** (F2; B1–B3)
  - On compact widths the legend sits above the tiles and the left-column padding goes.
  - The tiles become three full-width rows (glyph, name, check) at < 360 pt, or a 3-up grid that
    fits from 360 pt up.
  - The mode hint goes full width.
  - Red: no tile extends past `.cfg-body` at B1–B4.
- [✓] **RESP-5 Sheet footer that wraps.** (F3; B1–B4)
  - On compact widths the note moves to its own line above the buttons.
  - Primary actions fill the width (Add & Connect, Save), with secondary actions beside or below.
  - Delete… stays reachable and visually apart.
  - Red: every footer button is inside the viewport at 320, and the note is at most 2 lines.
- [✓] **RESP-6 iOS / compact toolbar.** (F8; B1–B4)
  - Drop the 88-pt traffic-light padding on iOS.
  - Below the compact breakpoint, search becomes a 44-pt button that expands a full-width field
    under the title (or a second toolbar row). The title, count, filter and + stay on one row.
  - Red: no horizontal page overflow at 320 or 390, + is fully visible, and the search field is at
    least 200 pt wide while open.
- [✓] **RESP-7 Tap to connect.** (F9; all breakpoints)
  - On `pointer: coarse` a single tap on a card runs its primary action (Connect / Show Window),
    and the action button shows on the card all the time (not on hover).
  - Long press opens the ⋯ menu (a touch-hold timer, because WKWebView doesn't send
    `contextmenu`).
  - Keyboard and mouse behaviour on the Mac is unchanged (gallery tests stay green).
  - Red: `pointer: coarse` + `click` on a card calls `connect`.
- [✓] **RESP-8 Session sheets clear the switcher.** (F17, F12; B1, B5)
  - *Re-scoped:* F12 was a measuring error (WebKit had scrolled the focused button into view);
    tall sheets were always reachable by scrolling, so F12 is P3 and needs no change.
  - On iOS the session page pads its sheets' top by the switcher's 44 + 12 pt and a 12-pt gap
    (68 pt) so the switcher never covers sheet content.
  - Red: harness F17 (a session sheet under the switcher at 320 pt).
- [✓] **RESP-9 Error sheet within the width.** (F13; B1–B2)
  - The action buttons may wrap and stack full-width on compact widths.
  - `.sheet` gets `min-width: 0` and `max-width: 100%`, so nothing inside can make it wider.
  - Red: no horizontal overflow on any session sheet at 320.

## P2 — works, but wrong or hard to use

- [✓] **RESP-10 No zoom on focus.** (F18; all)
  - Form controls are 16 px under `pointer: coarse` (or on `data-platform="ios"`), with labels
    and rows adjusted so the sheet's rhythm holds.
  - Don't disable user scaling.
  - Acceptance: focusing Name, Host and search on an iPhone does not zoom.
- [✓] **RESP-11 44-pt touch targets.** (F19; all)
  - Under `pointer: coarse`, these get at least a 44 × 44 pt hit area (by padding or `::before`
    expansion where the visual should stay small):
    - ⋯, toolbar buttons, the All/Open segments;
    - menu items;
    - sheet and dialog buttons;
    - switches (whole label row tappable);
    - key-row keys (44 tall).
  - Red: the harness's coarse-pointer target check passes at B3 and B8.
- [✓] **RESP-12 Configuration sheet as a full-height sheet on phones.** (F6, F4, F7; B1–B4)
  - On compact widths the sheet fills the safe area, square at the top or as a bottom sheet with
    a grabber.
  - The scrim gets a tint.
  - The header wraps: title on its own line, with Show Window moved under the meta line or into
    the footer, so the title isn't cut off at 390.
  - The Advanced summary hides its preview text on compact widths.
  - Acceptance: on a device, confirm the sheet surface hides the gallery behind it, both light
    and dark.
- [✓] **RESP-13 Stacked certificate row.** (F5; B1–B6)
  - The trusted-certificate row stacks label, then the full-width fingerprint (in a `.fp-card`
    as in the certificate prompt), then Forget Certificate, whenever its container is < 560 pt.
  - Red: the fingerprint is at most 3 lines at 390 pt and wider (95 monospace characters need
    about 650 pt for one line, so 2 lines can't be reached on a phone).
- [✓] **RESP-14 Key row fits the phone.** (F14, F15; B1–B5)
  - In portrait, every key is reachable without a hidden swipe. Pick one of these in the IOS-8
    ADR amendment:
    - two rows (modifiers + Activities / esc, ⇥, arrows, keyboard, touch), which means raising
      `phone::KEY_ROW_HEIGHT` to fit;
    - or paging with a visible indicator and edge fade.
  - Replace the ⌨ and ▭ glyphs with `icons.ts` SVGs at 18 pt, keeping their accessible names.
  - Red: every key is inside the strip at 390 × 56, or the overflow shows an indicator.
  *ADR `IOS-8-iphone-layout` decision 9.*
- [✓] **RESP-15 Greeter banner fits its frame.** (F16; B1)
  - Either the banner's text clamps to 2 lines with a shorter copy at narrow widths, or
    `present::hud_frame` takes the banner's measured height (page → Rust).
  - Red: at 288 × 76 the banner is inside its frame (`presentation.rs` test if the frame changes).

## P3 — polish

- [✓] **RESP-16 Gallery columns on wide iPads.** (F10; B6, B9)
  - Raise the card minimum (e.g. `minmax(300px, 1fr)`), or let names wrap to 2 lines, so names
    aren't cut off sooner in landscape than in portrait.
- [✓] **RESP-17 Hide keyboard hints without a keyboard.** (F11; all)
  - Hide `kbd` shortcut hints (the add card, ⋯ menu) under `pointer: coarse` unless a hardware
    keyboard is attached (`GCKeyboard` presence from the iOS shell, see IOS-6).
