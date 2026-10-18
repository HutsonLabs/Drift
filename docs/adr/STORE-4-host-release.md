# STORE-4 — Drift Host release: Developer ID, notarized DMG

- Status: accepted
- Date: 2026-10-18
- Task: STORE-4 (`TASKS.md`); ADR `IOS-0` decision 9, ADR `HOST-9-menu-bar`

## Decisions

1. **`cargo xtask host-release [--notarize]`** (`xtask::store::host_release_plan`): builds the
   bundle with `host-bundle`, signs it with the Developer ID identity Drift already uses (pinned
   by SHA-1), puts it with an `Applications` link in a compressed DMG, signs the DMG, and with
   `--notarize` submits it to Apple's notary service and staples the ticket. The App Store
   Connect key comes from `APPLE_API_KEY_PATH`, `APPLE_KEY_ID` and `APPLE_API_ISSUER` and is
   never printed. It does not publish anything.
2. **Signed from the inside out**, no `codesign --deep`: first `Contents/MacOS/drift-host`, then
   the bundle (whose main executable is the menu-bar app). Every signature has the Hardened
   Runtime and a secure timestamp.
3. **No App Sandbox and no entitlements.** `crates/drift-host-app/entitlements.plist` is an
   empty dictionary with the reasons: Screen Recording and Accessibility are TCC permissions,
   not entitlements; nothing needs a Hardened Runtime exception (no JIT, no third-party
   libraries).
4. **Apple silicon only** (`aarch64`), like the Drift DMG; IOS-0 targets Apple-silicon hosts.

## Checked on 2026-10-18

`cargo xtask host-release --notarize` produced `Drift Host_0.2.0_aarch64.dmg` (10 MB). The
notary accepted it (submission `95a0319c-5709-469b-b189-976f891c5ba3`) and `stapler validate`
passes. `spctl -a -t open --context context:primary-signature` on the DMG and `spctl -a` on the
mounted app both say "accepted, source=Notarized Developer ID"; `codesign --verify --deep
--strict` passes and the app has the runtime flag and no sandbox entitlement.
