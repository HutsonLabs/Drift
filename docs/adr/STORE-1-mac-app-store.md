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

## Not done (needs the account owner)

- **No App ID, profile or app record.** The developer account has no `com.hutsonlabs.drift`
  App ID, no Mac App Store (or iOS App Store) provisioning profile for it, and no App Store
  Connect app record (`altool --list-apps` lists Boards, Iceberg and term.hut only). Creating
  them changes the account, so it was not done. Until then the package has no embedded profile
  and App Store validation rejects it (STORE-2).
- **Export compliance (`ITSAppUsesNonExemptEncryption`) is not set.** Drift encrypts the whole
  session with its own TLS (rustls with ring) and uses NTLM/CredSSP for sign-in; it does not
  rely only on Apple's OS crypto, and the encryption is not limited to authentication. That
  reads as standard, mass-market encryption (ECCN 5D992, License Exception ENC
  §740.17(b)(1)), which usually means answering "yes, uses non-exempt encryption", choosing
  "standard encryption algorithms", and filing BIS's annual self-classification report. This
  is a legal answer for the account owner. On 2026-10-18 the owner chose to leave the key unset
  for now, so App Store Connect asks the export questions on each upload.
- **Skipped for now (owner, 2026-10-18).** No App ID, profiles or App Store Connect record are
  created; STORE-1 to STORE-3 stay `[~]` until the owner sets the account up.

## Checked on 2026-10-18

`cargo xtask mas-package` built `Drift.pkg` (15 MB): `lipo` shows `x86_64 arm64`, the app is
signed by Apple Distribution with the Hardened Runtime flag, its entitlements are the four
above, and `pkgutil --check-signature` shows the installer identity.
