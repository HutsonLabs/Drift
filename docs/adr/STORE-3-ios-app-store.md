# STORE-3 — iOS App Store packaging

- Status: accepted (packaging); archive and validation blocked on the developer account
- Date: 2026-10-18
- Task: STORE-3 (`TASKS.md`); ADR `IOS-0` decision 9, ADR `STORE-2-review-risk`

## Decisions

1. **Usage strings.** `NSLocalNetworkUsageDescription`, `NSBonjourServices`
   (`_drift-host._tcp`) and `NSCameraUsageDescription` were already in `Info.plist` and
   `project.yml` (IOS-5, IOS-9); `tests/app_config.rs` keeps them there.
2. **Privacy manifest** `drift-app_iOS/PrivacyInfo.xcprivacy`, bundled as a resource: no
   tracking, no tracking domains, no collected data, and four required-reason API categories
   with the reasons that fit Drift: file timestamps (`C617.1`, files in the app's container),
   system boot time (`35F9.1`, elapsed time), user defaults (`CA92.1`, the app's own), active
   keyboards (`54BD.1`, UI follows the keyboard). They come from scanning the built app:
   `stat`/`fstat`/`lstat` in the debug build, and the `standardUserDefaults`, `systemUptime`
   and `activeInputModes` selectors in the release static library. Declaring a category the
   linker later strips costs nothing; missing one is a rejection.
3. **Export options** (`gen/apple/ExportOptions.plist`): `app-store-connect`, team
   `M9Z3N2R97A`, manual signing, and `destination = export`, so `xcodebuild` writes an `.ipa`
   and never uploads. The owner chose validate-only; nothing in the repository uploads.
4. **No private transparency key on iOS.** Tauri's `macos-private-api` now reaches the app only
   through `drift-macos` (a macOS-only dependency), so wry's `drawsBackground` key is not in the
   iOS build (`cargo xtask review-risk` on the iOS app: 0 risks); `drift-ios` makes the page
   see-through with public UIKit properties, as before. `macOSPrivateApi` is dropped from
   `tauri.conf.json` because Tauri 2.11 ignores it at run time and tauri-build would demand the
   cargo feature on the shared `tauri` entry.
5. **`libapp.a` is linked, not bundled.** xcodegen had added `Externals/*/libapp.a` (900 MB)
   to the app's resources whenever the project was regenerated after a build. `Externals` is now
   a source group with no build phase.

## TestFlight (2026-10-18)

The owner created the App Store Connect record ("Drift - Screen sharing", SKU HL005, app id
6824351800; bundle id `com.hutsonlabs.drift`, universal) and asked for TestFlight.

- `tauri.conf.json` sets `bundle.iOS.developmentTeam` (`M9Z3N2R97A`) and `bundleVersion`.
  `CFBundleVersion` must grow with every upload: pass `--build-number <n>` (or raise
  `bundleVersion`) for the next one.
- Build, validate and upload, with the App Store Connect API key from `~/code/term.hut/.env`
  (`APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_PATH`; never printed or committed):

  ```text
  (cd ui && bun run build)
  set -a; . ~/code/term.hut/.env; set +a
  cd src-tauri && cargo tauri ios build --export-method app-store-connect --build-number <n>
  export API_PRIVATE_KEYS_DIR="$(dirname "$APPLE_API_KEY_PATH")"
  xcrun altool --validate-app -f gen/apple/build/arm64/Drift.ipa -t ios --apiKey "$APPLE_KEY_ID" --apiIssuer "$APPLE_API_ISSUER"
  xcrun altool --upload-app   -f gen/apple/build/arm64/Drift.ipa -t ios --apiKey "$APPLE_KEY_ID" --apiIssuer "$APPLE_API_ISSUER"
  ```

  With the API key in the environment, Xcode signs automatically and created the
  "iOS Team Store Provisioning Profile: com.hutsonlabs.drift" on the first build.
- Testers: the internal group "Drift Internal" (all builds) holds the account holder. Internal
  testers must be App Store Connect users; external testers need Beta App Review.
- Export compliance stays unanswered in `Info.plist` (owner's decision), so each build asks the
  encryption question in App Store Connect before testers can install it.
