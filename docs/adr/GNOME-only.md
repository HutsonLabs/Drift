# GNOME-only — Drift connects to GNOME; Drift Host is retired

- Status: accepted
- Date: 2026-10-20
- Supersedes: the Mac host half of `IOS-0-ios-client-mac-host-app-store` (decisions 2–7, 9's
  Drift Host bullet, 10), `UI-kinds-target-kinds`, `AVC444-client`, `IOS-9-pairing`,
  `STORE-4-host-release`, every `HOST-*` and `EXT-*` ADR, and `APP-icon` decision 6. They stay in
  `docs/adr/superseded/` for history.

## Context

Drift Host was meant to give a Mac what macOS Screen Sharing gives it, reached from Drift on any
Apple device. Carried over RDP it is a poor replacement: Screen Sharing's High Performance mode
(private protocol, virtual display, HDR, login window) cannot be matched by a third-party RDP
server, and keeping a second product (host bundle, pairing, QUIC transport, a private DVC,
HEVC, an extra DMG in every release) costs more than it returns. Mac users already have Screen
Sharing for Macs.

GNOME Remote Desktop is where Drift is strong: headless sessions, Remote Login, Display Control
resize, CLIPRDR, sound. What it lacks is an easy way to set up a Linux machine.

## Decisions

1. **One product: Drift for Mac, iPhone and iPad, connecting to GNOME.** The Mac and iOS apps
   stay (Mac App Store, Developer ID DMG, App Store/TestFlight). Every target is a GNOME
   Remote Desktop host; there are no target kinds.
2. **Drift Host is removed.** Crates `drift-host`, `drift-host-app`, `drift-capture`,
   `drift-inject`, `drift-transport` (QUIC) and `drift-ext` (`Drift::Mac::v1`) are deleted, as
   are `xtask host-bundle`, `host-release`, `release --verify-reproducible` and the
   `Drift-Host_*.dmg` release artifact. A release is three artifacts: iOS `.ipa`, Mac App Store
   `.pkg`, Drift DMG (iOS and macOS still in lockstep, ADR `RELEASE-pipeline`).
3. **Client code that only a Drift Host could reach goes too:** pairing (Bonjour browse,
   `drift-pair:` links, QR scanner, device key, approval stage, `PairingRefused`), QUIC-first
   dialing, quality hints, menu mirroring, HEVC, AVC444 decode and the V10 capability list
   (GNOME is always offered V8.1/V8, ADR `M1-2-gfx-client`), gesture events (swipe, pinch,
   smart zoom: GNOME has no equivalent and the fast-path encoder already dropped them), the host
   encoders in `drift-video` and the frame mailbox. Precise scroll stays: its wheel fallback is
   what GNOME receives.
4. **Kept, re-homed:** the menu model (`MenuItem`, `MenuSnapshot`, …) moves from `drift-ext` to
   `drift_core::menus` because the iPad menu bar's GNOME menus use it. Sound (rdpsnd) stays.
5. **Old Mac profiles are dropped on load.** A `profiles.toml` entry with `paired_host` pointed
   at a Drift Host; `ProfileStore::from_toml` skips it (and the next save removes it) rather
   than showing it as a GNOME computer with a Drift Host's pin.
6. **The UI has one kind.** No All/Macs/GNOME filter, no kind-first add sheet, no Mac tiles,
   badges, sheet, quality step or Mac key row. "+" opens the New Connection form directly.
7. **Setting up a Linux machine is one command on the Mac:**
   `host/drift-gnome-setup user@machine`. Over SSH it installs `gnome-remote-desktop` when
   missing (apt, dnf, zypper or pacman), configures a persistent headless session for the
   account on a fixed port with a TLS certificate and generated RDP credentials, opens the port
   in ufw/firewalld when one is active, turns off idle suspend and lock, and reads the
   certificate fingerprint. The remote part is `host/drift-host-setup.sh` (check-first,
   idempotent, ADR `M0-4-host-setup`) extended with `--install` and `--firewall`, and a
   `--fingerprint` line in its output. On the Mac it copies the password to the clipboard and
   opens a `drift-connect:` link, so Drift shows a prefilled New Connection form with the
   certificate already pinned; it also prints the link as a QR code (with `qrencode`) for
   iPhone and iPad.
8. **`drift-connect:` links.** `drift-connect:?name=…&host=…&port=…&mode=headless|remote-login|desktop-sharing&user=…&linux_user=…&pin=…`.
   Parsed in `drift_core::link` (pure, tested); never carries a password. Opening one brings
   Connections forward with the form prefilled; nothing is saved until the user clicks Save.
   It replaces the `drift-pair:` scheme in both apps' `Info.plist`.

## Consequences

- About 25k lines leave the tree; `quinn`, `qrcode`, ScreenCaptureKit and AVFoundation (QR)
  leave the dependency graph. iOS no longer declares Bonjour or camera use.
- Releases no longer build, notarize or publish Drift Host. Users of Drift Host 26.10 keep the
  build they have; it receives no updates.
- Bringing a Mac target back would mean a new ADR; the removed code is in git history at
  `ce6c578`.

## Plan

1. Delete the six host crates; move the menu model; make `drift-rdp`'s stream TCP only.
2. Remove pairing, kinds, quality, menu mirroring, HEVC/AVC444, gestures from the client,
   UI and tests; regenerate bindings.
3. xtask: drop host bundle/release, the Host DMG step and its tests.
4. Drop old Mac profiles on load; add `drift-connect:` links (core parser, Tauri open-URL
   handler, prefilled form).
5. `host/drift-gnome-setup` (Mac side) and the `--install`/`--firewall`/fingerprint steps in
   `host/drift-host-setup.sh`; shellcheck and xtask tests.
6. Docs: this ADR, superseded ADRs moved, `TASKS.md`, `docs/acceptance.md`,
   `docs/gnome-host-setup.md`, team conventions, the release skill, notes for the release.
7. `cargo xtask ci`, then `cargo xtask release` and publish on spaces.hutson.cloud.
