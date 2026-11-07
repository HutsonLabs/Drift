# Mobile responsiveness audit — findings

- Date: 2026-10-21
- Scope: the web UI that Drift for iPhone and iPad shows (`ui/src`, one stylesheet
  `ui/src/styles.css`): the Connections page (`index.html`) and the session page (`session.html`)
  in each state it renders. The title-bar page (`titlebar.html`) is Mac only (no title-bar webview
  on iOS, `windows_ios.rs`) and is out of scope. Native views (remote picture, trackpad, switcher,
  paste button) are in scope only where they meet the page.
- Remediation: [`tasks.md`](tasks.md).

## Method

The real controllers (`ConnectionsApp`, `SessionApp`) were rendered with the fakes and fixtures
from `ui/test` (four profiles with long names and hosts, one live and one reconnecting) into the
production stylesheet. Each state was then loaded in Playwright WebKit at every breakpoint below,
at 2× with touch on. Each page is sized to the frame the iOS shell gives it: full window for
pages and sheets; `min(560, w − 32) × 76` for the greeter banner and `300 × 56` for the stats HUD
(`present::hud_frame`); `w × 56` for the iPhone key row (`phone::KEY_ROW_HEIGHT`, landscape inside
the 62-pt safe insets). Every run was screenshotted and measured for:

- horizontal page overflow and elements outside the viewport;
- sheets cut off vertically;
- text cut short with an ellipsis;
- touch targets smaller than 44 × 44 pt;
- form controls under 16 px, which iOS zooms in on when they get focus.

States: welcome (no profiles), gallery, search with no results, New Connection sheet, Edit sheet
(trusted certificate, open session, Advanced expanded), ⋯ menu, delete confirmation; session
connecting, certificate, error, reconnecting, greeter banner, stats HUD, key row.

The stylesheet has **no width- or pointer-based media queries**. Its only media queries are for
colour scheme, reduced motion and reduced transparency. Nothing in the pages knows it is on iOS
(no platform class, no `viewport-fit`, no `env(safe-area-inset-*)`). Safe areas are left to
WKWebView's default automatic content insets.

## Breakpoints evaluated

| # | Breakpoint | Size (pt) | Stands for | Verdict |
|---|---|---|---|---|
| B1 | Narrow | 320 × 568 | iPad Slide Over / narrowest split, large Dynamic Type zoom | **Broken**: page overflows 76 pt sideways; New Connection form unusable; error sheet clipped |
| B2 | iPhone 17e | 390 × 844 | compact-width iPhone portrait | **Broken**: config sheet fields collapse; + button partly off screen; footer clipped |
| B3 | iPhone 17 Pro | 402 × 874 | default iPhone portrait | **Broken**: config sheet fields collapse; tiles overflow; search unusable; key row hides arrows |
| B4 | iPhone Pro Max | 440 × 956 | large iPhone portrait | **Degraded**: same sheet layout problems, milder; key row hides arrows |
| B5 | iPhone landscape | 874 × 402 | compact height | **Broken**: certificate and error sheets taller than the screen, top unreachable |
| B6 | iPad split | 590 × 834 | iPad half split / Stage Manager small | **Usable**: fingerprint wraps to 6 lines; touch targets small |
| B7 | iPad mini | 744 × 1133 | iPad mini portrait | **Usable**: touch targets small; sheet translucency (verify on device) |
| B8 | iPad 11″ | 834 × 1210 | iPad portrait | **Good** apart from the touch-wide issues (F9, F11, F18, F19) |
| B9 | iPad 11″ landscape | 1210 × 834 | iPad landscape | **Good** apart from the touch-wide issues; card names cut short (4 narrow columns) |
| B10 | iPad 13″ landscape | 1376 × 1032 | largest | **Good** apart from the touch-wide issues |

Screenshots of the worst cases are in [`shots/`](shots/) (`<breakpoint>_<state>.png`).

## Findings

Severity: **P1** blocks a task on that breakpoint · **P2** works but wrong or hard to use ·
**P3** polish.

### Connections page — configuration sheet

**F1 · P1 · B1–B4 (B6 partly).** The field grid stays two-column at every width.
`.cfg-body .group .field` is `minmax(120px, 168px) 1fr`. With the sheet's 24-pt scrim padding
and 24-pt body padding, controls get about 138 pt at 402. The Host row then also fits a Port
label and an 84-pt port field, so **the Host input collapses to about 22 pt at 390–402 and
disappears at 320**. Name is about 25 pt at 320. The password placeholder cuts off
("Saved – lea"). Shots: `402-iphone-17pro_c-edit.png`, `320-narrow_c-new.png`.

**F2 · P1 · B1–B3.** The connection-type tiles overflow the sheet.
`fieldset.mode` keeps the label column, padding-left `16 + min(168px, 30%) + 16`, and forces
`repeat(3, 1fr)` tiles. **Remote Login is clipped off the right edge** at 320–402 (element right
edge 388–412 in a 320–402 viewport). The mode hint wraps to 5–6 lines in a narrow column.

**F3 · P1 · B1–B2 (B3–B4 degraded).** The sheet footer doesn't wrap.
Note, Delete…/Cancel, Add and Add & Connect sit in one `nowrap` row, so **Add & Connect is cut
off** at 320 and 390. At 402 the note "Applies on next connect" wraps to 3 lines and "Passwords
are kept in your Keychain" to 4.

**F4 · P2 · B1–B4.** The sheet header collides.
The title cuts off ("Build b…") and the Show Window button overlaps the meta line
("10.0.2.41:3389 · Trusted") because the head row is `nowrap` with a spacer.

**F5 · P2 · B1–B6.** The trusted-certificate row is a three-column grid (label, fingerprint,
Forget button). At 402 the fingerprint column is **one character wide and renders vertically**;
at 590 it wraps to 6 lines and the label wraps "(SHA-\n256)".

**F6 · P2 · B1–B4.** On compact widths the sheet is still a floating card: 58 pt from the top,
24 pt sides, 26-pt radius, a 3-pt blurred scrim with no tint, and an 86 % opaque surface. That
wastes about 100 pt of a phone screen. In WebKit renders the gallery and welcome text show
through the sheet (`390-iphone-17e_c-welcome.png`, `744-ipad-mini_c-new.png`). *Check the
translucency on a device: WKWebView's backdrop blur may hide it.*

**F7 · P3 · B1–B4.** The Advanced summary ("Keyboard, display and clipboard") wraps to 3 lines
and its summary text cuts off.

### Connections page — toolbar and gallery

**F8 · P1 · B1–B4.** The toolbar is the Mac title bar.
It reserves **88 pt on the left for macOS traffic lights that iOS doesn't have**. The 220-pt
search field is squeezed to a **41-pt icon-only field you can't type in** at 402. The **+ (New
Connection) button sits outside the viewport** at 320 (366–396 in 320) and partly outside at 390,
so the page **scrolls sideways by 76 pt (320) and 6 pt (390)**. Shot:
`402-iphone-17pro_c-gallery.png`.

**F9 · P1 · all breakpoints.** A card's primary action has no tap target.
Connect / Show Window lives in `.over`, which only shows on `:hover` / `:focus-within`, or comes
from `dblclick` on the card (`views/card.ts`). On touch, it depends on WebKit's sticky-hover
heuristics or a double tap; nothing on the card says it can be tapped. Long press doesn't open
the context menu (`contextmenu` is not fired by WKWebView on touch), so Edit, Duplicate and Delete
are only reachable through the 28-pt ⋯ button.

**F10 · P3 · B6, B9.** Card names and hosts cut off. At 1210 the `minmax(250px, 1fr)` grid makes 4
narrow columns, so names ("Homelab workstation (…") cut off sooner than in portrait at 834.
Phone landscape has the same problem.

**F11 · P3 · all.** Keyboard-only hints on touch devices: `⌘N` on the add card, and `⌘E`/`⌘D`/`⌘⌫`
and `↩` in the ⋯ menu. They show without a hardware keyboard.

### Session page

**F12 · P3 · B5** *(corrected 2026-10-22; first filed as P1).* A sheet taller than the screen opens scrolled.
The audit measured the certificate sheet at y = −67 and called it unreachable. That was WebKit
scrolling the focused Connect button into view (scrollTop 99): at scroll 0 the sheet starts at
y = 32 and the page scrolls to every part of it (RESP-1's harness measures at scroll 0). What
remains is cosmetic: the sheet opens scrolled with its icon off the top. The native 44-pt
switcher button (safe area + 12) does sit over a session sheet's top-left where the sheet
reaches it (F17). Shot: `874x402-iphone-land_s-certificate.png`.

**F13 · P1 · B1 (B2 by 1 pt).** The error sheet overflows sideways.
The action buttons are `nowrap` ("Open Local Network Settings" is 204 pt), so the sheet grows past
`min(500px, 100%)`. At 320 the page scrolls 71 pt sideways and the primary action is cut off.
Shot: `320-narrow_s-error.png`.

**F14 · P2 · B1–B4.** The key row hides keys off screen.
The 13 keys need about 717 pt. In portrait only 7–9 show (402: Ctrl … ⇥). **The arrows, the
keyboard button and the touch-mode switch need a sideways swipe**, and nothing shows that the row
scrolls (it overflows by 315–397 pt). Shot: `402-iphone-17pro_s-keys.png`.

**F15 · P2 · B5 (all phones).** The key row's keyboard (⌨) and touch-mode (▭) keys are bare Unicode
glyphs. They render about 10 pt and are hard to read next to 15-pt labels. Shot:
`874x402-iphone-land_s-keys.png`.

**F16 · P2 · B1.** The greeter banner gets clipped. The native frame is a fixed 76 pt tall
(`BANNER_HEIGHT`), but at 288 pt wide the text wraps to 3 lines and the pill runs 8 pt past the
frame. Shot: `320-narrow_s-banner.png`.

**F17 · P3 · B5.** The native switcher button (44 pt at safe-area top-left + 12) overlaps the top-left
of any session sheet that reaches the top edge. Fixing F12 with top padding solves this too.

### Every page, every breakpoint

**F18 · P2 · all.** Inputs zoom on focus.
Every text input and select is 13 px (`:root` font). **iOS WebKit zooms the page in when a
control under 16 px gets focus**, and the viewport allows scaling. That's 9 controls in the
configuration sheet, plus search. *Known WebKit behaviour; confirm on a device (acceptance).*

**F19 · P2 · all.** Touch targets under 44 pt (Apple HIG minimum):

| Control | Size (pt) |
|---|---|
| ⋯ card actions | 28 × 28 |
| toolbar + | 30 × 30 |
| All / Open segments | 26 tall |
| menu items | 24 tall |
| sheet buttons (Cancel, Save, Connect, Reconnect, Delete…) | 28 tall |
| switches | 38 × 22 |
| port stepper | — |
| key-row keys | 40 tall |

**F20 · P3 · all.** The pages don't know about safe areas or the platform. They rely on WKWebView's
automatic insets, which letterbox landscape iPhones. There is also no `body` class for iOS /
touch, which F8, F9, F11 and F19 all need. Decide this once, in an ADR.

## What is fine

- Session connecting, reconnecting and stats HUD at every breakpoint.
- Certificate sheet in portrait at every width.
- Delete confirmation.
- The gallery grid itself (one column on phones, 2–4 on iPad).
- The search with no results.
- iPad full-screen layouts (B8–B10), apart from the issues that affect every page (F9, F11,
  F18, F19).
