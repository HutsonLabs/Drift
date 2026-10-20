# UI-kinds — Mac and GNOME target kinds

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

Status: accepted (2026-10-18). Builds part 2 of `docs/design/mockup-targets.html`.

## Context

Pairing a Mac (IOS-9) saves a *Headless session* profile, so a Mac running Drift Host looked and
behaved like a GNOME computer: the same glyph and `host · Headless session` line on its card, the
GNOME mode picker, RDP credentials and `grdctl` hints in its sheet, a mode glyph and host name in
its title bar, an empty iPad menu bar for GNOME, and one PC-style iPhone key row for both.

## Decisions

1. **The kind is derived, not stored.** `drift_core::HostKind { Mac, Gnome }`, and
   `ConnectionProfile::kind()` is `Mac` exactly when `paired_host` is set. No schema change, so
   `profiles.toml` and older readers are untouched. If a third kind (e.g. a Windows RDP host)
   shows up, make it an explicit field then. The UI mirrors the rule in `ui/src/kinds.ts`.
2. **Gallery.** The filter is All / Macs / GNOME / Open (one gallery, no sidebar, on the Mac,
   iPad and iPhone alike). Cards carry a kind glyph and a kind badge on the preview. A Mac's
   detail line is `Drift Host · host`; a GNOME card keeps `host · mode`. A saved Mac gets a muted
   Mac tile; GNOME keeps the mode gradients. Thumbnails stay in memory only.
3. **One add flow.** "+", ⌘N (the `new` intent), the dashed card and first launch open
   *Add a Connection*, which asks for the kind first: *Pair a Mac…* (the IOS-9 pairing sheet) or
   *Add GNOME Computer…* (the New Connection form). The toolbar's separate "Pair a Mac" icon is
   gone. A `drift-pair:` link still opens pairing directly.
4. **A sheet per kind.** A Mac's sheet shows its name, the pairing (account, address, pinned host
   key), display (fit the Mac's screen to the window, which drives the HOST-10 virtual display
   through Display Control, and Retina), a note that keys go to the Mac unchanged, clipboard and
   touch. It drops the mode picker, RDP credentials, `grdctl` hints, "⌘ sends" and Mac layout.
   Its destructive button is *Unpair…*, which deletes the profile (pairing again makes a new one).
   A GNOME sheet stays as it was, with "Connection type" renamed "GNOME session" and Retina
   labelled "Render at 2× (Retina)". Quality, HDR and the link path stay automatic: the mockup's
   pickers for them have no profile setting behind them yet, so the sheet only says quality
   follows the Mac.
5. **Title bar.** `WindowIdentity` gains `kind` and `quality` (the EXT-4 step from the latest
   statistics sample). The capsule shows the kind glyph, the name, and either the GNOME mode or
   the Mac's quality step (Sharp / Balanced / Smooth / Light for best / high / medium / low;
   "Drift Host" before the first report). The host moves to the tooltip when there is no greeter
   hint. The identity is already re-pushed on every view change and de-duplicated, so a quality
   change reaches the title bar without a new event.
6. **iPad menu bar.** A Mac keeps mirroring its front app's menus (EXT-3). The front GNOME window
   shows Drift's own menus (`crate::gnome_menu`): *Session* (Disconnect), *Keys* (Super, Switch
   Windows, Lock Screen, Ctrl+Alt+Delete) and *Activities* (Activities Overview, Show Apps). Item
   ids live at `0x4E00_0000 +`; a choice sends the chord through `phone::named_chord_events` or
   closes the session window.
7. **iPhone key row.** `renderKeyRow` takes the kind (the session page reads it from
   `window_identity`). GNOME: Ctrl Alt ⇧ Super and an Activities key (Super). Mac: ⌃ ⌥ ⇧ ⌘ (Drift
   Host maps Super to Command) with ⌘Space and ⌘Tab. `SoftKey` gains `Space`.

## Consequences

- No IPC command changes; two `WindowIdentity` fields and `HostKind` are new in the bindings.
- The kind is only as right as `paired_host`: a Mac added by hand as a plain RDP host is GNOME
  to the UI, which matches how Drift talks to it (no `Drift::Mac::v1` pairing).
