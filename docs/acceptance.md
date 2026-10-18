# Manual acceptance checklist

Steps that cannot be automated (they need a human, a GUI permission prompt, physical network
changes or a clean machine). Each milestone's owner appends its "Done (manual …)" items here.

## M0 — Foundation

- [ ] `cargo tauri dev` opens a window titled "Drift" showing the connections screen ("Add a GNOME computer" on first launch).

### M0-6 / §5.3 — CI and the nightly e2e workflow

The gate's *contents* are asserted by `cargo test -p xtask --test ci_plan --test workflows`
(ADR `M0-6-ci-gate-coverage`). What is left needs a person, because nothing is pushed to the
public GitHub remote (`docs/team-conventions.md`), so no workflow ever runs there.

- [ ] **Canary (plan M0-6 Red):** on a throwaway branch, make one unit test fail (e.g. change an
      assertion in `crates/drift-core`), run `cargo xtask ci`, and see it stop at the nextest step
      with a non-zero exit; restore the test and see it pass again. The same command is the only
      thing `.github/workflows/ci.yml` runs, so a red test is a red CI run.
- [ ] **Nightly e2e (plan §5.3):** `.github/workflows/e2e-nightly.yml` is written against a
      self-hosted macOS arm64 runner labelled `drift-lan` with an SSH key for the GNOME host and
      the `DRIFT_E2E_*` repository secrets. Until a runner is registered and the repository is
      published, exercise the same steps by hand: `cargo xtask host-setup-check` then
      `cargo xtask e2e` (add `DRIFT_E2E_BENCH=1` on an idle host for the M9-1 budgets), and
      confirm no credential appears in the output.

## M2 — Input translation (drift-input decisions to confirm by hand)

These need a physical keyboard/trackpad and a person watching the remote desktop
(see `docs/adr/M2-1-keyboard-translation.md`).

- [ ] **ISO keyboard:** on an Apple ISO keyboard, the key left of `1` and the key right of left
      Shift type the same characters on the remote (with a matching remote XKB layout) as locally.
- [ ] **JIS keyboard:** `¥`, `ろ`, `英数` and `かな` behave as on a local GNOME session.
- [ ] **Deferred Command:** Cmd+Tab away from Drift and back does *not* open the GNOME overview;
      tapping Cmd alone *does*; Cmd+T opens a Drift tab and nothing happens on the remote.
- [ ] **Super+drag:** holding Cmd and dragging a window moves it on the remote.
- [ ] **Caps Lock:** toggling Caps Lock locally changes the remote caps state; switching tabs keeps
      it in sync.
- [ ] **Type using Mac layout:** Option+e, e → `é`; Option+s → `ß`; Japanese IME (Kotoeri)
      composes locally and commits into gedit; Ctrl+C / Cmd+V still act as chords.
- [ ] **Unicode beyond the remote layout:** type `Grüße ✓` into GNOME Text Editor on a host
      whose session layout provides those keysyms (e.g. add German in Settings › Keyboard and
      switch to it) and copy it back. g-r-d turns each Unicode event into an XKB keysym and
      mutter only injects keysyms the *session's* layout can produce, so on a US-only session
      `ü`, `ß` and `✓` never arrive — `e2e_unicode_typing` therefore only automates the ASCII
      (incl. shifted, typed without Shift) part. See `docs/adr/M4-2-session-actor-channels.md`.
- [ ] **Scroll:** two-finger scrolling in Files and Firefox is smooth and follows the local
      natural-scrolling direction; a notched mouse wheel scrolls one step per notch.

## M1-6 / M3-2 / M7-3 / M9-4 — Connect UI (stream D)

Rendering and intents are covered by `bun test`; these need eyes, VoiceOver or a real prompt.

- [ ] Light and dark appearance (System Settings › Appearance): the connections screen, the
      certificate prompt and the error screens use system colours and stay readable in both.
- [ ] VoiceOver (Cmd+F5): every field of the profile form is announced with its label; the
      connection type is announced as a radio group “Connection type”; an invalid field is
      announced as invalid together with its message; the certificate prompt is announced as an
      alert dialog with its title; the reconnect overlay countdown is announced politely.
- [ ] VoiceOver on the **hints that M9-4 made announceable** (`ui/test/accessibility.test.ts`
      can only check that the references exist): landing on the Connection type group reads the
      mode explanation; the certificate prompt reads “On the host, run sudo grdctl …” after its
      description; the reconnect overlay reads “Homelab · Attempt 2 of 20” with the countdown.
- [ ] VoiceOver on the **live picture** (M9-4): with a session connected, VO-cursor onto the
      picture. It is announced as an image named “<profile> — remote desktop, 1280 by 800
      pixels”, described as “remote desktop”, and VoiceOver does not descend into it. Typing
      still reaches the remote desktop while VoiceOver is on.
- [ ] Switching Connection type between Remote Login, Headless session and Desktop Sharing
      swaps the credential fields without losing the typed name and host.
- [ ] Certificate prompt for the homelab system daemon shows the same fingerprint as
      `sudo grdctl --system status` (“TLS fingerprint: f3:e7:a2:…”) on 10.1.2.40.
- [ ] Local Network Privacy screen (a freshly ad-hoc-signed build connecting to 10.1.2.40
      directly, errno 65): “Open Local Network Settings” opens System Settings › Privacy &
      Security › Local Network.

### Overlays over the live picture (M7-3, M1; see `docs/adr/M7-3-overlays-over-the-live-picture.md`)

The AppKit half is checked by `cargo test -p drift-app --features macos-ui-tests --test
overlays_ui` (window not opaque, overlay fills the window, HUD is a corner panel with
`DriftRemoteView` as first responder). What a person still has to *see*:

- [ ] **Stats overlay, "Done (manual M1)":** connect Headless to `drifttest2`, run `anim.py`
      full-screen on the host and choose **Session ▸ Show Statistics**. A small panel appears in
      the bottom-right corner of the picture and reads **≥ 55.0 fps** (1280×800; the plan's
      reference run was 58.9 fps). Choosing the item again removes it. While it is up, clicking
      and typing anywhere else in the window still reach the remote desktop, and the GNOME top
      bar and dash are not covered.
- [ ] **Reconnect overlay, M7-3:** with the same session live, `sudo systemctl restart
      gnome-remote-desktop` on the host. The desktop's last frame stays on screen, **dimmed**,
      with the "Reconnecting in N s… [Now] [Cancel]" card centred over it — not a solid panel.
- [ ] **Greeter banner, M3-2/M7-3:** in Remote Login mode at the GDM greeter, the hint banner
      ("Log in as “drifttest” …") floats over the greeter picture near the top, the greeter is
      visible around it, and the password can still be typed into GDM's field.
- [ ] **Transparency did not break the chrome:** with three tabs in one group, the tab strip,
      window shadow, rounded corners, light/dark appearance and full-screen all look normal, and
      the connections screen is fully opaque (no desktop showing through).

## M1-1 / M3-1 — Connect and redirect (drift-rdp)

Automated: loopback tests against `FakeServer` and `cargo xtask e2e` (`e2e_headless_connects`,
`e2e_remote_login`). These need macOS Local Network Privacy, which only a GUI prompt can grant:

- [ ] A Drift build **without** the Local Network permission (ad-hoc signed, or permission denied
      in System Settings › Privacy & Security › Local Network) connecting straight to
      `10.1.2.40:3392` ends in `Failed { LocalNetworkDenied }` (errno 65 is mapped by
      `drift_rdp::connect::classify_io_error`; the loopback test injects the errno).
- [ ] After granting the permission once, the same build connects without SSH forwards, and a
      Remote Login profile goes leg 1 → greeter (`AwaitingGreeterLogin`) → desktop after logging
      in at the greeter.

## M3-2 / M7-3 — Greeter password opt-in (stream A)

Automated: the loopback tests in `crates/drift-rdp/tests/m7_3_reconnect.rs` (typed only after a
click, never without the opt-in). What needs a person is the real GDM greeter:

- [ ] With a `linux-login` secret stored for a Remote Login profile, reconnect to the greeter,
      click your user tile and **do nothing else**: Drift types the password and logs in about
      1.5 s later, landing in the same session.
- [ ] With the same profile but no stored Linux password, the greeter stays untouched no matter
      where you click.
- [ ] Clicking "Not listed?" instead of a tile and waiting: Drift types the password into the
      user-name field (known consequence of "type into the focused field"); it is visible, so
      clear it and pick the tile. Nothing is typed before the first click.

## M1-5 / M4-3 — Rendering (drift-render)

- [ ] With a live Headless session (drifttest2), the desktop colours match the host monitor
      (compare a GNOME Settings window side by side: no tint, blacks are black, whites white).
- [ ] At a Retina drawable equal to the desktop size (Retina on, window not resized) terminal
      text is pixel-sharp (no filtering blur); after resizing to a non-matching size the picture
      scales smoothly with black bars, and snaps back to sharp once DISP re-layout completes.

## M1-6 / M2-2 / M2-4 / M2-5 / M3-2 / M6-2 / M7-2 — macOS platform layer (drift-macos)

These need a person, a real keyboard/IME, physical network changes or sleep
(see `docs/adr/M1-6-macos-platform-layer.md`).

- [ ] **RemoteView placement:** after connecting, the webview disappears, the desktop fills the
      window, and typing goes to the remote immediately (no click needed). Resizing the window
      and moving it between a Retina and a 1× display keeps the picture sharp.
- [ ] **Shortcut routing:** Cmd+C / Cmd+V / Ctrl+Tab / Cmd+K act on the remote; Cmd+T, Cmd+W,
      Cmd+Q, Cmd+1…9, Cmd+Shift+[ / ], Cmd+` act on Drift and nothing happens on the remote.
      Holding Cmd+K and releasing K first, then Cmd, leaves no stuck key on the remote.
- [ ] **Connect form still edits text:** with the connect form visible, Cmd+C / Cmd+V / Cmd+A
      work in its text fields (the RemoteView does not claim them when it is not focused).
- [ ] **IME / dead keys (Type using Mac layout on):** Option+E, E types `é`; the Japanese IME
      shows its candidate window near the bottom-left of the session and commits into gedit;
      Return while composing confirms locally instead of sending Enter.
- [ ] **Remote cursor:** the pointer takes GNOME's shapes (I-beam over text, resize arrows on
      window edges, hand over links) at the same size as local cursors, crisp at Retina/200 %;
      it disappears where GNOME hides it and becomes the arrow when leaving the window.
- [ ] **Keychain:** saving a profile with a password creates a Keychain Access item
      "com.hutsonlabs.drift", account `<profile uuid>/rdp-user` (or `rdp-system`); deleting the
      profile removes it; no password prompt appears on relaunch of the same signed build.
- [ ] **Tabs:** three sessions open as tabs of one window even with System Settings › Desktop &
      Dock › "Prefer tabs when opening documents" = "In Full Screen"; the tab strip's "+" opens
      a Connection Manager in a new tab; the Window menu lists the tabs with their state glyph
      (● ◐ ◌ ↻ ○ ⚠) — see "UI-tabs" below for the strip itself.
- [ ] **Network trigger:** with a live Headless session, Wi-Fi off pauses reconnecting (no
      attempts counted); Wi-Fi on reconnects within ~1 s without waiting for the backoff.
- [ ] **Wake trigger:** sleep the Mac for > 1 min with a live session; after wake it reconnects
      immediately (single attempt, not one per trigger).
- [ ] **Cancel survives a wake (M7-2):** in a live session, pull the network, press *Cancel* on
      the reconnect overlay, then sleep and wake the Mac. The session stays `Disconnected` —
      the wake trigger must not restart a session the user cancelled.
- [ ] **Local → remote clipboard in the app (M5-3/M5-2):** with a live session, copy text in a
      Mac app (Cmd+C) and paste it in GNOME Text Editor within ~250 ms of switching back;
      repeat with a PNG copied from Preview into a GTK4 app. With two tabs open, copying on the
      Mac and pasting in the *focused* tab's session works, and the unfocused tab's session
      receives it only once its tab is selected.
- [ ] **Clipboard before connecting (M5-3):** copy text on the Mac *before* opening a session,
      then connect; the first paste in GNOME already gives that text.

## M6-1 / M6-2 / M8-3 — Sessions, tabs and menus (drift-app) — tabs superseded by UI-windows

Automated: `cargo nextest run -p drift-app` (fake-actor lifecycle, menu model, presentation) and
`cargo test -p drift-app --features macos-ui-tests --test tabs_ui` (three real windows in one tab
group, AppKit's tab bar hidden, one strip per window in group order, traffic lights in the strip,
`newWindowForTab:`) and `--test overlays_ui` (page, picture and HUDs below the strip).

- [ ] **Tab-group test (M6-2; superseded by UI-windows `windows_ui`):** run `cargo test -p drift-app --features macos-ui-tests --test
      tabs_ui` from a logged-in graphical session (not over SSH, not on a locked screen: it opens
      real NSWindows and needs a window server) and see
      `session_windows_join_one_native_tab_group ... ok`. `cargo xtask ci` compiles this binary on
      every run (`clippy --all-features --all-targets`, see `docs/adr/M0-6-ci-gate-coverage.md`)
      but cannot run it unattended.

These need a person:

- [ ] **Three live sessions (UI-windows: three windows):** open Remote Login, Headless and Desktop Sharing profiles in three
      tabs of one window; switching tabs shows each desktop instantly, and background tabs stay
      below ~2 % CPU (Activity Monitor).
- [ ] **Tab keys (superseded by UI-windows: no tabs; Cmd+1…9 focus session windows):** Cmd+1…9 select tabs; Cmd+Shift+[ / ] move
      between them; the tab overview (Window ▸ Show All Tabs) shows live thumbnails; Window ▸
      Move Tab to New Window keeps that session running in its own window (with its own strip),
      and Window ▸ Merge All Windows rejoins the group. AppKit's tab bar (and so dragging tabs)
      is gone; the strip replaces it.
- [ ] **Titles (superseded by UI-windows: the title is the plain connection name):** the window title — only visible in the Window menu now
      — is the profile name with its state glyph, or "Connections"; the greeter hint is the
      floating banner and the tab's tooltip, no longer a title bar subtitle.
- [ ] **Close and quit (superseded in part by UI-windows: Cmd+W asks first when live; Cmd+Q asks once):** Cmd+W on a connected tab closes the session (the GNOME host shows no
      leftover session) and then the tab; Cmd+Q with three live sessions quits within about two
      seconds; the red close button behaves like Cmd+W. Closing the last tab leaves Drift running
      (Dock icon); clicking the Dock icon opens a new tab.
- [ ] **Session menu (Disconnect superseded by UI-windows: File ▸ Disconnect closes the window):** Send Ctrl+Alt+Del shows GNOME's screen; Reconnect restarts a failed
      session in the same tab; Disconnect returns the tab to the Connection Manager without
      closing it.
- [ ] **Recording (feature `recording` only):** `cargo run -p drift-app --features recording`,
      connect, then Debug ▸ Record Session (experimental); after a minute toggle it off and play
      `~/Movies/Drift <profile> <timestamp>.mp4` in QuickTime — it shows the session at the right
      size and duration.

## UI-windows — Connections gallery and one window per connection (`docs/design/mockup-windows.html`)

Supersedes the UI-tabs section (tab strip, tab keys, connect in place), which no longer applies.
Automated: `bun test` (gallery, cards, pills, filter/search, keyboard grid, labels, ⋯ menu,
sheet, identity capsule, error sheet), `cargo nextest run -p drift-app` (window model, close
confirmation table, menu and Dock models, frames, chrome, thumbnail throttle, identity table,
manager lookups) and, from a logged-in graphical session,
`cargo test -p drift-app --features macos-ui-tests --test windows_ui --test overlays_ui`
(three independent untabbed windows, traffic lights in the 52-pt bar, full screen covers the
screen). These need eyes on a real window (ADR `UI-windows-gallery`); check each in **light and
dark** appearance:

- [ ] **Board 0 — window model:** launch shows only Connections. Connect two profiles: each
      opens its own window at once (no tab bar anywhere, View has no Show Tab Bar). Connecting a
      profile that is already open brings its window forward, no second session. Close
      Connections with the red button: the sessions keep running; Cmd+0 brings it back.
- [ ] **Board 1 — gallery:** cards are glass with a 16:9 preview; open connections sit in
      "Open" above "Saved", followed by the dashed New Connection card. A live card shows the
      session's picture within ~15 s (refreshing a few times a minute) and a green Live pill with
      uptime; Connecting shows a spinner, Reconnecting an amber "Reconnecting in N s" over a
      dimmed preview, Failed a red pill. Hover and keyboard focus show one button (Connect /
      Show Window); double-click and Return do the same. ⋯ and right-click offer Connect, Edit…,
      Duplicate, Disconnect (open only) and Delete… (asks first). All / Open and search (name,
      host) filter the grid.
- [ ] **Boards 2–3 — sheet:** + , the dashed card and Cmd+N open the sheet over a dimmed,
      blurred gallery; Headless is preselected; the tiles read Headless, Desktop Sharing, Remote
      Login. Switching mode keeps name, host and port. Add & Connect is disabled until valid, and
      a missing host is flagged on the field. The body scrolls under a fixed header and footer;
      Esc cancels. Edit (⋯ or Cmd+E): Delete… on the left, Save disabled until a change; a live
      connection says "Applies on next connect". The "Keyboard, display and clipboard"
      disclosure is closed with a one-line summary.
- [ ] **Board 4 — connecting:** Connect opens the window with the stage checklist and the
      capsule's spinner; the certificate prompt is a sheet on that window while Connections stays
      usable. Cancel (or closing the window) stops the attempt, closes the window, and the card
      goes back to idle.
- [ ] **Board 5 — session window:** the transparent 52-pt title bar has the traffic lights at
      the usual place, a centred capsule (glyph, name, host, green dot) and the grid and gauge
      buttons; the picture starts right under it. The grid brings Connections forward; the gauge
      toggles the statistics HUD. Clicking anywhere in the title bar leaves the keyboard with the
      remote desktop. Mission Control and Cmd+` show the connection's name. Resize/move a window,
      close it, reconnect: it comes back at the same place and size (per connection).
- [ ] **Close confirmation:** closing a live window (red button or Cmd+W) asks "Disconnect …?"
      as a sheet; Cancel keeps it, Disconnect ends the session (no leftover session on the host)
      and closes the window. A connecting or failed window closes without asking.
- [ ] **Board 6 — full screen and Spaces:** full-screen two session windows; each gets its own
      Space, swipe between them; nothing is drawn over the picture, also while the menu bar is
      revealed (no title bar, capsule or buttons come back). The Window menu lists Connections
      (Cmd+0) and a "Sessions" section with every session, the current one checked, Cmd+1/Cmd+2
      switching. File shows New Connection… (Cmd+N), Show Connections (Cmd+0), Edit <name>…
      (Cmd+E), Disconnect <name> (Shift+Cmd+D), Close Window (Cmd+W); no New Tab. Every one of
      these shortcuts works while the remote desktop has the keyboard; Cmd+T reaches the remote.
- [ ] **Board 7 — failed:** a wrong password keeps the window with the error sheet and a red dot;
      the card shows Failed. Edit Connection… brings Connections forward with the edit sheet
      open; Close dismisses the window and the card returns to idle.
- [ ] **Dock menu:** right-click the Dock icon: a "Sessions" section lists each session window
      with its glyph and status, then Connections and New Connection…; each item works.
- [ ] **Quit:** Cmd+Q with two live sessions asks once ("Quit Drift? 2 sessions will be
      disconnected."), then quits within about two seconds.
- [ ] **VoiceOver on the gallery:** VO+arrows move card to card; each reads name, mode, host and
      state; the ⋯ menu and the sheet's fields are announced with their labels.
- [ ] **Privacy:** after a session with the gallery open, nothing under the app container
      (`~/Library/Containers/com.hutsonlabs.drift/`) contains a picture; `window-frames.json`
      holds only numbers.

## M9-1 / M9-4 — Performance and polish

`cargo xtask e2e --bench` measures the M9-1 budgets against the homelab and compares them with
`tests/e2e/bench-baseline.json` (see `docs/adr/M9-1-performance-bench.md`). Two things still
need eyes:

- [ ] Run `cargo xtask e2e --bench` on an **otherwise idle Mac** (no parallel builds) and paste
      the table into the milestone notes. The latency numbers roughly halve compared with a
      contended run; if they do not, something really did get slower.
- [ ] **App icon:** after `cargo xtask bundle`, the waves glyph is legible in the Dock, in
      Finder's icon and list views, in Cmd+Tab and in the About panel — no white box, no blurry
      upscale at 16 px.

## E2E round 1 — what the automated live-app smoke cannot reach

`cargo xtask e2e` covers the protocol, input, clipboard, display and reconnect behaviour
against the homelab, and `e2e_live_desktop_pixels` (tests/e2e/tests/render.rs) drives the real
Metal compositor against the live `drifttest2` desktop, writing the composite to
`target/e2e/live-desktop-{still,anim}.png` and asserting ≥ 55 fps under `anim.py`.

The *window* itself still needs a person, because three things on the dev Mac are only
available to a session a human has unlocked:

- [ ] **Unlocked screen.** With the Mac's screen locked (`CGSSessionScreenIsLocked = true`),
      every Drift window reports `occlusionState ∌ Visible`, so Drift correctly sends Suppress
      Output and presents 0 fps. Verified during round 1: with `anim.py` running full-screen on
      the host, the app's socket received **0 bytes in 4 s** and the stats line stayed at
      `fps=0.0`. Repeat the smoke on an unlocked screen and confirm the stats line climbs to
      ~60 fps within a second of the window becoming visible.
- [ ] **Screen Recording permission.** `screencapture` fails with "could not create image from
      display" from an SSH shell *and* from a launchd agent in the Aqua session. Take the
      screenshot of the connected window as the logged-in user and check: GNOME's wallpaper
      gradient is blue (not orange — that would mean swapped R/B), blacks are black and the
      colour-bar test image (`/home/drifttest2/clip_in.png`) shows saturated primaries.
- [ ] **Keychain partition list.** A password written by `/usr/bin/security` (or by a *different*
      build of Drift) carries the ACL partition `apple-tool:`, so the app gets the "Drift wants
      to use your confidential information" panel the first time it reads it, and the session
      actor waits for the click. Save the profile *through Drift's own connect form*, with the
      stable "Apple Development" signing identity from plan §5.1, and confirm that relaunching
      and connecting never shows that panel. The unattended smoke harness
      (`cargo run -p drift-app --example smoke`, `DRIFT_SMOKE_SECRET=<uuid>:<role>:<password>`)
      exists only because this step cannot be scripted.

Repeatable setup for the manual run (round 1 used exactly this):

```bash
ssh -N -L 23392:localhost:3392 -L 23389:localhost:3389 homelab@10.1.2.40 &
DRIFT_CONFIG_DIR=/tmp/drift-smoke DRIFT_AUTOCONNECT="Homelab Headless" \
  cargo run -p drift-app --bin drift-app           # profiles.toml pins the daemon's fingerprint
```

## M9-3 — App Sandbox, Hardened Runtime and the Local Network prompt

`cargo test -p drift-app --test m9_3_hardening` pins the bundle configuration (entitlements
file, `hardenedRuntime`, `NSLocalNetworkUsageDescription`), and
`crates/drift-rdp/tests/m9_3_logging.rs` proves no credential reaches a log. What is left needs
a **signed, sandboxed build** (`cargo xtask bundle`, M9-5) and a person, because the sandbox and
the privacy prompts only exist for a real bundle — a `cargo run` binary is unsandboxed and the
tests would pass either way. See `docs/adr/M9-3-sandbox-hardening-and-secrets.md`.

Install the signed `Drift.app`, then confirm on that build:

- [ ] **The sandbox is on:** `codesign -d --entitlements - /Applications/Drift.app` lists
      `com.apple.security.app-sandbox` and `com.apple.security.network.client`, and
      `codesign -dv` shows `flags=0x10000(runtime)` (Hardened Runtime). A container appears at
      `~/Library/Containers/com.hutsonlabs.drift/`.
- [ ] **Local Network prompt:** on first connect to a LAN host macOS shows the prompt with
      Drift's usage description; allowing it connects, and the permission survives relaunching.
      Deny it (System Settings › Privacy & Security › Local Network) and confirm Drift shows the
      errno 65 screen whose button opens exactly that pane.
- [ ] **Keychain:** save a profile password in the sandboxed app, quit, relaunch and connect —
      the password is found without any prompt (the item lives in the app's own access group,
      keyed by the bundle identifier). `drift-macos` uses the **legacy** `SecKeychain` API
      (`security-framework`'s `os::macos` module), which a sandboxed app may use for its own
      items; if this step ever fails with `errSecInteractionNotAllowed` or an unexpected access
      panel, the fix is to move `drift_macos::keychain` to the data-protection keychain
      (`SecItemAdd` with `kSecUseDataProtectionKeychain`), not to widen the entitlements.
- [ ] **Metal:** the live picture renders at ~60 fps with `anim.py` on the host (the stats HUD);
      the sandbox must not affect `CAMetalLayer` or the `IOSurface`-backed frames.
- [ ] **VideoToolbox:** the same session decodes AVC420 in hardware (no fallback message in the
      log, `fps` stays at 60 with motion), and with the `recording` feature the encoder writes a
      playable MP4 into the container's `Movies` folder.
- [ ] **Pasteboard:** copy text and an image in both directions (M5 acceptance) inside the
      sandboxed app; the general pasteboard is reachable without an entitlement.
- [ ] **Config:** the profile list is written to
      `~/Library/Containers/com.hutsonlabs.drift/Data/Library/Application Support/com.hutsonlabs.drift/`
      and survives a relaunch; nothing is written outside the container.

## IOS — Drift for iPhone and iPad

Build and run in a simulator (ADR `IOS-5-ios-app-shell`):

```text
cd src-tauri && cargo tauri ios build --target aarch64-sim --debug --features smoke-env
xcrun simctl install <device> gen/apple/build/arm64-sim/Drift.app
cargo run -p drift-testkit --example fake_host -- "$(xcrun simctl get_app_container <device> com.hutsonlabs.drift data)/Library/DriftSmoke"
SIMCTL_CHILD_DRIFT_CONFIG_DIR=<that dir> SIMCTL_CHILD_DRIFT_AUTOCONNECT="Fake host" \
  SIMCTL_CHILD_DRIFT_SMOKE_SECRET=<printed by fake_host> xcrun simctl launch <device> com.hutsonlabs.drift
```

Done in an iPad Pro 13-inch (M5) simulator, iOS 27, on 2026-10-18 against `fake_host`: live
moving picture, the session survives 40 s in the background, and with
`FAKE_HOST_CLOSE_AFTER=25` the host's close is followed by a reconnect within a second while the
app is in the background. Still to do by hand:

- [ ] **IOS-5 first light on a device:** a real iPad (signed with a development team) connects to
      the GNOME test host in Headless mode, shows the live desktop at the iPad's native size, and
      after 5 minutes in the background (long enough for iOS to suspend it) reconnects on return.
      The GNOME host (10.1.2.40) was down when IOS-5 landed.
- [ ] **Local Network prompt on iOS:** the first LAN connection shows iOS's prompt with Drift's
      usage text; denying it shows the errno 65 screen, whose button opens Drift's page in
      Settings.
- [ ] **ProMotion:** on a 120 Hz iPad Pro or iPhone Pro, Quartz Debug-style frame-rate overlay
      (Xcode ▸ Debug ▸ View Debugging ▸ Rendering) shows up to 120 Hz while the picture moves and
      a drop to the system rate when Connections is in front.
- [ ] **IOS-6 keyboard:** on an iPad with a Magic Keyboard, typing, modifiers, arrows, F-keys and
      Cmd chords reach the remote (Cmd as Super by default); the shortcuts listed in ADR `IOS-6`
      stay with iPadOS; switching to Connections and back leaves no key stuck.
- [ ] **IOS-6 pointer:** without lock, the pointer moves the remote pointer, click and
      secondary click work, two-finger trackpad and wheel scroll scroll the remote. With the
      session full screen, the pointer locks (system pointer disappears) and mouse motion,
      buttons and wheel still work; check the wheel's direction.
- [ ] **IOS-6 Pencil:** hovering an Apple Pencil (M2 iPad or later) moves the remote pointer.
- [ ] **IOS-7 touch:** on an iPhone (trackpad by default) a drag moves the pointer, a tap clicks,
      tap-and-drag drags a window, two fingers scroll and a two-finger tap opens a context menu;
      a pinch zooms the picture and taps still land under the finger. On an iPad (direct by
      default) a tap clicks where you touch, a long press right-clicks, a drag drags. Switching the
      profile's "Touch on iPhone and iPad" changes the next session.
- [ ] **IOS-8 iPhone layout:** on an iPhone, portrait shows the picture on top, the trackpad and
      the key row; ctrl+c with the latched key copies on the remote; the keyboard button brings up
      the iOS keyboard and typing (including emoji and Return) reaches the remote. Turning to
      landscape gives a full-screen picture whose key row hides after 4 s and comes back from a
      touch at the bottom edge. (Checked by screenshot in the simulator on 2026-10-18; keys and
      touches need a device.)
- [ ] **H.264 on a device:** `headless_motion.h264` decodes without `-8969` (it fails only in the
      simulator's software decoder, ADR `IOS-0` open question 3).
- [ ] **IOS-10 paste button:** with a live session, copy text in Notes, tap the paste button in
      the picture's bottom-right corner: no "Drift would like to paste" prompt appears, and the
      text is typed into the remote's focused text field (Ctrl+V). Repeat with a copied photo
      and a "Text and images" profile. Copy on the remote, then paste in Notes. Switch the
      profile's clipboard off: the button disappears.
- [ ] **IOS-11 switcher:** on an iPhone with two sessions, tap the round button at the top left
      of a session: the menu lists Connections and both sessions with their status, the current
      one checked; choosing another session shows it, choosing Connections shows the gallery.
      On iPad, do the same, then close a session's window from the app switcher (swipe it up):
      its session ends and its card in Connections goes back to idle. Close the Connections
      window the same way, then choose Connections from a session's menu: it comes back.
      Simulator runs can script this with `DRIFT_SMOKE_SCRIPT="wait:8;connect:Fake host 2;wait:8;pick:session-0"`
      (`fake_host` with `FAKE_HOST_PROFILES=2`).

### IOS-9 — Pairing (ADR `IOS-9-pairing`)

Done in an iPad simulator on 2026-10-18 with `cargo run -p drift-apple --example advertise_host`
on the Mac: the "Pair a Mac" sheet lists the advertised host, and a `drift-pair:` link
(`DRIFT_SMOKE_PAIRING_LINK` with the `smoke-env` build) fills the code and names the host. Still
to do by hand:

- [ ] **Camera:** on a device, Scan Code asks for the camera once, reads the QR code printed by
      `advertise_host` (any QR generator), fills the link and closes; Cancel closes without a
      code; with the camera denied, the sheet says so.
- [ ] **System Camera app:** scanning the same QR code with the Camera app offers to open Drift,
      which opens "Pair a Mac" with the link (warm and cold launch).
- [ ] **Local Network prompt:** the first discovery on a device shows iOS's Local Network prompt;
      after denying it the list stays empty and pairing by link still works.
- [ ] **Pair and connect:** pair with a typed code and an account; the new card shows Trusted;
      connecting it reaches the pin check (a wrong certificate fails before credentials are sent).
- [ ] **After HOST-8:** the Mac accepts the code and lists the device; a wrong code is refused
      with "The Mac did not accept the code."

## HOST — Drift Host (Mac)

### HOST-3 — Capture (ADR `HOST-3-capture`)

Needs a Mac where the process running the tests (Terminal, or the CI runner) has Screen
Recording in System Settings › Privacy & Security:

- [ ] `cargo test -p drift-capture --test sck_smoke -- --nocapture` prints no "skipped" lines: the
      displays list with pixel sizes, the first frame is IOSurface-backed, the display's pixel
      size, and a full update.
- [ ] **Dirty rects:** with a stream running, move a small window; the reported rects cover it in
      output pixels on a Retina display (not half-size, which would mean they are points).
- [ ] **Pointer:** the stream's frames never contain the pointer; moving it to a second display
      reports `Hidden` for the first.

### HOST-6 / EXT — Injection and the `Drift::Mac::v1` channel

- [ ] **HOST-6 injection (by hand, once HOST-2 wires it):** with Accessibility granted to Drift
      Host, type from a client: letters with Shift, Command shortcuts (Windows key), arrows,
      Option-accented text, and Unicode text from the iOS keyboard. Click, double-click, drag a
      window, right-click, middle-click, scroll both ways with a wheel. Disconnect while holding
      Shift and a mouse button: nothing stays held on the Mac. Without Accessibility, the host
      says so and injects nothing. `cargo test -p drift-inject --test cg_events -- --ignored`
      posts one harmless move (passed on the dev Mac on 2026-10-18).
- [ ] **EXT-1 against a real Drift Host (after HOST-2):** connect the Mac client and an iPad to
      Drift Host; the client log shows `Drift::Mac::v1 negotiated`. Connect the same client to
      the GNOME host: no such line, and the session works as before.

### HOST-4 — Real-time encoder (ADR `HOST-4-realtime-encoder`)

Measured on an M4 Pro on 2026-10-18 with `cargo run --release -p drift-video --example
realtime_bench -- <width> <height> <frames>` while other builds ran. Still to do by hand:

- [ ] **Quiet machine:** rerun the benchmark at 3840×2160, 2560×1600 and 1920×1080 with nothing
      else running; update the ADR's table if the CPU-split or AVC444 rows move.
- [ ] **M1:** run it on an M1 Mac (ADR `IOS-0` open question 5 asked about M1).

