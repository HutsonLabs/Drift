# STORE-1 — Mac App Store packaging

- Status: accepted (packaging); blocked on the developer account and an export-compliance answer
- Date: 2026-10-18
- Task: STORE-1 (`TASKS.md`); ADR `IOS-0` decision 9

## Decisions

1. **A separate config, not a separate app.** `src-tauri/tauri.mas.conf.json` is merged over
   `tauri.conf.json` for the App Store build: only the `app` bundle, signed with Apple
   Distribution (pinned by SHA-1, like the Developer ID identity), and
   `entitlements.mas.plist`. The Developer ID build and DMG are unchanged.
2. **Entitlements.** The same sandbox as M9-3 (`app-sandbox`, `network.client`, nothing else)
   plus `com.apple.application-identifier` (`M9Z3N2R97A.com.hutsonlabs.drift`) and
   `com.apple.developer.team-identifier` (`M9Z3N2R97A`), which the App Store needs and which must
   match the embedded provisioning profile.
3. **Universal.** `x86_64-apple-darwin` joins `rust-toolchain.toml`; the build target is
   `universal-apple-darwin`. The whole app builds for Intel unchanged.
4. **`cargo xtask mas-package [--profile <file>]`** (`xtask::store::mas_plan`): builds the
   universal app, embeds the profile as `Contents/embedded.provisionprofile` and re-signs, then
   `productbuild` wraps it in `bundle/mas/Drift.pkg` signed with the "3rd Party Mac Developer
   Installer" identity (pinned by SHA-1).
5. **Short description** now names both host kinds: "Remote desktop for GNOME Remote Desktop and
   Macs running Drift Host".

## Account and TestFlight (2026-10-18)

- The owner created the App Store Connect record (app id 6824351800, universal bundle id
  `com.hutsonlabs.drift`). The Mac App Store profile "Drift Mac App Store" was created through
  the App Store Connect API for both Apple Distribution certificates; a copy lives in
  `~/Library/MobileDevice/Provisioning Profiles/Drift_Mac_App_Store.provisionprofile`
  (re-download it from the developer portal on another Mac).
- Mac App Store builds ship only together with iOS, through `cargo xtask release --testflight`
  (ADR `RELEASE-pipeline`): one version and one build number for both platforms, set with a
  `bundle.macOS.bundleVersion` config override. Tauri notarizes whenever App Store Connect
  credentials are in its environment, so the Mac App Store build step runs without them (an
  App Store package is never notarized).
- Build 26.10 (3) for macOS passed validation and was uploaded to TestFlight (before the
  lockstep rule; iOS stood at 26.10 (2), so the next release uses build 4 on both).
- **Export compliance: `ITSAppUsesNonExemptEncryption = false`** (owner's decision,
  2026-10-18), in `src-tauri/Info.plist` and the iOS `Info.plist`. Drift encrypts the whole
  session with its own TLS (rustls with ring) and uses NTLM/CredSSP; these are standard
  algorithms in a mass-market product (ECCN 5D992, License Exception ENC §740.17(b)(1)), which is
  *exempt* in Apple's terms. `true` means non-exempt and needs approved documentation and an
  `ITSEncryptionExportComplianceCode`; App Store validation rejected it without one (90592). The
  owner files BIS's annual self-classification report.
- **Skipped for now (owner, 2026-10-18).** No App ID, profiles or App Store Connect record are
  created; STORE-1 to STORE-3 stay `[~]` until the owner sets the account up.

## Checked on 2026-10-18

`cargo xtask mas-package` built `Drift.pkg` (15 MB): `lipo` shows `x86_64 arm64`, the app is
signed by Apple Distribution with the Hardened Runtime flag, its entitlements are the four
above, and `pkgutil --check-signature` shows the installer identity.
