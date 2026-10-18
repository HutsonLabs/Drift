# Team conventions

Read `plan.md` §0–§5 first; it is the source of truth. This page summarises how we work.

## Branches, commits, merges

- One branch per task: `task/<ID>-<slug>` (e.g. `task/M1-2-gfx-state-machine`).
- Strict TDD (plan §0):
  1. write the task's **Red** tests, run them, confirm they fail for the right reason, commit
     `test(<ID>): …`;
  2. implement until green, commit `feat(<ID>): …`;
  3. refactor, commit `refactor(<ID>): …`; `cargo xtask ci` must pass.
- **Nothing is pushed.** The GitHub remote is public: no `git push`, no GitHub PRs, no forks, no
  upstream IronRDP PRs. A "PR" is a local branch that the integrator merges with `--no-ff`; the
  merge commit message records the Red tests and their failing output.
- Do not tick `plan.md` checkboxes; the integrator does that.

## ADRs

Any decision not already fixed by `plan.md` gets an ADR in `docs/adr/<TASKID>-<slug>.md`
(task-ID prefix, so parallel tasks never collide on numbers). Existing ADRs:

| ADR | Topic |
|---|---|
| `M0-1-workspace-conventions` | workspace, lints, coverage gate, bindings, UI build |
| `M0-2-ironrdp-vendored-fork` | IronRDP vendored in `third_party/ironrdp` + patch queue |
| `M0-5-session-interface` | `spawn_session` / `SessionHandle` / `SessionEvent` seam |
| `M0-5-nv12-frame-location` | `Nv12Frame` / `H264Decoder` live in `drift_core::video` |
| `M0-6-dependency-policy` | cargo-deny licenses (MPL-2.0 allowance), advisories |
| `M0-6-ci-gate-coverage` | the CI gate as data, `--all-features` lints, vendored fork tests, required workflows |
| `M1-4-cpu-codecs` | parallel RFX Progressive, `BgraTile`, DRFTGFX1 GFX captures, release strip fix |
| `M2-1-keyboard-translation` | drift-input: ISO/JIS mapping, deferred Command, lock keys, Unicode, scroll, viewport, allow-list |
| `M1-6-macos-platform-layer` | drift-macos: RemoteView, key equivalents, IME, cursor decode, Keychain, NWPathMonitor, tabs, main-thread test harness |
| `M1-6-profiles-and-view-model` | profiles.toml, credentials UX, SessionView/screen model, error texts |
| `M4-1-layout-policy` | desired_layout details (Retina threshold, device scale, max area) |
| `M7-1-reconnect-policy` | backoff RNG, attempt budget semantics, trigger merger debounce |
| `M0-3-fixture-capture-and-sanitization` | fixture capture, layout, `.rec` format, sanitizer |
| `M0-4-host-setup` | check-first host setup script, host-setup-check |
| `M1-2-gfx-client` | GFX client: suspend-ack timing, queueDepth, surface/cache validation, fuzz crate |
| `M1-1-connect-and-redirect` | TLS pin/TOFU, error classification, redirect loop, GFX wiring, FakeServer, e2e redaction |
| `M6-1-session-manager-and-windows` | SessionManager/SessionHost seam, tab windows, menus, quit cap, greeter subtitle, launch options |
| `M4-2-session-actor-channels` | resize debounce, CLIPRDR glue, suppress/ack policy, reconnect resume, greeter typing, FakeServer channels, real-host e2e findings |
| `UI-tabs-connection-manager` | Connection Manager / Session tabs: hidden AppKit tab bar, HTML strip webview per window, full screen, focus, main-queue dispatch (superseded in most part by `UI-windows-gallery`) |
| `UI-windows-gallery` | Connections gallery window + one window per session: no tabbing, title-bar webview, frames, thumbnails, Dock menu, menus, IPC contract |
| `IOS-3-drift-apple-split` | `drift-apple` (Keychain, `NWPathMonitor`, LNP) shared by `drift-macos` and `drift-ios`; foreground as the iOS wake trigger; Settings link |
| `IOS-5-ios-app-shell` | the app on iOS: `windows_ios.rs` shell, one window (scene on iPad) per connection, tracked Xcode project, `smoke-env`, `fake_host` |
| `IOS-9-pairing` | client half of Mac pairing: `_drift-host._tcp` TXT, Damm-checked code, `drift-pair:` QR link, `paired_host` profiles, Keychain device key and pins |
| `IOS-10-clipboard-on-ios` | UIPasteboard adapter, no polling, writes for the front window, `UIPasteControl` paste = clipboard + remote Ctrl+V |
| `HOST-3-capture` | `drift-capture`: one `SCStream` per display, NV12 (BGRA for AVC444), damage from dirty rects, pointer sampled separately (`currentSystemCursor`), permission preflight |
| `HOST-5-frame-mailbox` | `drift_core::mailbox`: one slot, newest wins, drops counted, `post_merging` keeps damage, async `recv` |
| `IOS-11-session-switcher` | native switcher button over session windows, own-scene activation (tao `set_focus` bug), closed scenes close sessions, `DRIFT_SMOKE_SCRIPT` |
| `IOS-6-ios-input` | iPad input in Rust (not a Swift plugin): HID table, `Pointer`, `GCKeyboard`/`GCMouse`, hover, pointer lock per view controller, shortcuts iPadOS keeps |
| `IOS-7-touch-modes` | trackpad and direct touch, two-finger scroll and right click, `ConnectionProfile::touch`, iPhone pinch zoom as a view transform |
| `IOS-8-iphone-layout` | iPhone portrait (picture, trackpad, key row) and landscape (full screen, auto-hiding key row); web key row with `send_keys`/`type_text`/`set_touch_mode`; `ResizeWatcher` |
| `HOST-6-input-injection` | `drift-inject`: `kvk_for` from the same table, injection plan (flags, drags, clicks, 40 px per notch), CGEventPost at the HID tap, Accessibility |
| `EXT-1-channel-framing` | `drift-ext`: `DRFT` v1 header, types by task, Hello negotiation, fallback without DVC errors, `SessionOptions::drift_mac`, fuzz target |
| `IOS-0-ios-client-mac-host-app-store` | Drift on iPhone/iPad (Tauri iOS), Drift Host on the Mac over RDP + `Drift::Mac::v1` DVC, App Store eligibility |

## Commands

Always `export PATH="$HOME/.cargo/bin:$PATH"`. Use `bun` for anything JavaScript; never
another JS package manager.

| Command | What it does |
|---|---|
| `cargo xtask check` | fast loop: `cargo fmt --check`, clippy `-D warnings`, nextest |
| `cargo xtask ci` | the merge gate: npm-ban, secret-scan, workflow audit, `bun install --frozen-lockfile`, `bun test`, `bun run typecheck`, `bun run build`, bindings freshness, fmt, clippy `--all-features --all-targets`, the vendored IronRDP fork's fmt + tests, nextest under llvm-cov + coverage gate, `cargo deny`, and the iOS crates linted for
`aarch64-apple-ios` and `aarch64-apple-ios-sim` with their simulator tests linked (IOS-2). The step list lives in `xtask/src/ci_plan.rs` and is asserted by `xtask/tests/ci_plan.rs` (ADR `M0-6-ci-gate-coverage`) |
| `cargo xtask ci --no-coverage` | same, without instrumentation (faster locally) |
| `cargo xtask bindings` | regenerate `ui/src/bindings.ts` after changing IPC commands/types |
| `cargo xtask npm-ban` / `secret-scan` / `workflows` | the individual hygiene checks |
| `cargo xtask e2e [nextest args]` | real-host tests through SSH forwards (below) |
| `cargo xtask import-fixtures [--staging DIR]` | sanitize + import `~/code/drift-spikes/fixtures-staging` into `fixtures/` (M0-3, ADR `M0-3-fixture-capture-and-sanitization`) |
| `cargo xtask host-setup-check` | read-only check of the GNOME host over SSH (M0-4, `docs/gnome-host-setup.md`) |
| `cargo xtask bundle` | M9-5 (stub until then) |
| `cargo tauri dev` / `cargo tauri build` | run / bundle the app (UI is built by bun first) |

`cargo tauri build` signs with the Developer ID identity pinned in `tauri.conf.json`
(`bundle.macOS.signingIdentity`, by SHA-1 — the name is shared by two certificates). It lives in
`~/Library/Keychains/term-hut-signing.keychain-db`, which `~/.local/bin/signing-unlock` unlocks
from every zsh (hooked in `~/.zshenv`) and at GUI login, so builds over SSH work too. If codesign
fails with `errSecInternalComponent`, the keychain is locked: run `signing-unlock -v`. Check a
build with `codesign -dv --verbose=2 target/release/bundle/macos/Drift.app`.
Notarize and staple the .dmg with `xcrun notarytool submit … --wait` (App Store Connect API key)
and `xcrun stapler staple`; `cargo xtask bundle` does not wire this up yet (M9-5).

Builds are large; long cargo runs can exceed a 10-minute tool call — run them in the background
and poll. Other agents build in parallel on the same machine.

## Code rules

- Pure logic in pure crates with unit/property tests; FFI crates (`drift-apple`, `drift-macos`,
  `drift-ios`, `drift-video`, `drift-render`) are humble objects with smoke tests. `drift-ios`'s
  tests run in a simulator: `xcrun simctl spawn <device> target/aarch64-apple-ios-sim/debug/deps/<test>`.
- No `unwrap`/`expect` on network data (clippy `unwrap_used` is an error outside tests); every
  `unsafe` block has a `// SAFETY:` comment; public items are documented (`missing_docs`).
- Apple platforms only (ADR `IOS-0`): `cfg(target_os = "macos")` / `"ios"` gates, nothing for
  other platforms. AppKit (`objc2-app-kit`) is only ever a
  `[target.'cfg(target_os = "macos")'.dependencies]` entry; `xtask/tests/ci_plan.rs` checks it.
- Shared contracts (plan §3) live in `drift-core` (+ `FrameSink` in `drift-gfx`, the session
  interface in `drift-rdp::session`). Changing them is a cross-stream change: update the ADR.
- IronRDP changes: separate commits touching only `third_party/ironrdp/`, exported to
  `third_party/ironrdp-patches/` (see the M0-2 ADR).

## Fixtures

`fixtures/` is tracked with git-lfs (`.gitattributes`: `fixtures/**`, except `README.md` and
`MANIFEST*`). Run `git lfs install --local` once per clone. Fixtures come from
`~/code/drift-spikes` via `cargo xtask import-fixtures` (M0-3), which sanitises them; the
secret scan must stay green. Load them in tests through `drift_testkit::fixtures` (path
constants in `drift_testkit::fixtures::names`, `read`, `records` for `*.rec`); provenance and
formats are in `fixtures/README.md`. The host scripts in `host/` are tested with `shellcheck`
(`brew install shellcheck`).

## Secrets and e2e

- Real credentials live only in `~/code/drift-spikes/secrets/*.txt` (mode 600) on the dev Mac.
  Never print them, commit them, or put them in fixtures, logs, snapshots or test source.
  Tests use obviously fake values.
- `cargo xtask e2e` opens `ssh -N -L 1339x:localhost:339x homelab@10.1.2.40` (required by macOS
  Local Network Privacy) and runs `cargo nextest run -p drift-e2e --run-ignored only` with:
  `DRIFT_E2E_HOST=127.0.0.1`, `DRIFT_E2E_TLS_NAME=10.1.2.40`, `DRIFT_E2E_PORT_<3389..3392>`
  (local forward ports), and credentials mapped from the secrets directory unless already set:
  `DRIFT_E2E_SYS_USER/PASS` (system.txt), `DRIFT_E2E_LOGIN_USER/PASS` (testuser.txt),
  `DRIFT_E2E_HL_USER/PASS` + `DRIFT_E2E_HL_PORT` (headless2.txt), `DRIFT_E2E_HL1_*`
  (headless.txt), `DRIFT_E2E_SHARE_*` (share.txt).
- E2E tests live in `tests/e2e` and are all `#[ignore]` so `cargo xtask ci` never needs the host.
- Anything that genuinely needs a human (permission prompts, Wi-Fi toggles, clean machines)
  goes into `docs/acceptance.md` as a manual step.
