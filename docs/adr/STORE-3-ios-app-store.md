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

- Versions: the marketing version is `yy.mm` (`CFBundleShortVersionString`, e.g. `26.10`), set
  in the iOS `Info.plist`, `project.yml` and `src-tauri/Info.plist`; Cargo and `tauri.conf.json`
  carry the semver form `26.10.0`. The build number grows by one with every upload, across
  versions.
- `tauri.conf.json` sets `bundle.iOS.developmentTeam` (`M9Z3N2R97A`).
- iOS ships only together with macOS, through `cargo xtask release --testflight` (ADR
  `RELEASE-pipeline`): App Store Connect's highest build number on any platform + 1, passed as a
  `bundleVersion` config override (Tauri's `--build-number` would append to it: `1.2`), the same
  for both platforms. It reads the App Store Connect key from `APPLE_API_KEY_PATH`,
  `APPLE_KEY_ID` and `APPLE_API_ISSUER` (`set -a; . ~/code/term.hut/.env; set +a`); the token
  goes to `curl` on stdin and is never printed. With the key in the environment, Xcode signs automatically and created the
  "iOS Team Store Provisioning Profile: com.hutsonlabs.drift" on the first build.
- Testers: the internal group "Drift Internal" (all builds) holds the account holder. Internal
  testers must be App Store Connect users; external testers need Beta App Review.
- Export compliance: `ITSAppUsesNonExemptEncryption = false` (ADR `STORE-1-mac-app-store`), so
  builds need no per-build answer. Build 1, uploaded before the key, got the same answer through
  the App Store Connect API.
