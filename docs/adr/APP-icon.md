# APP-icon — one Icon Composer app icon for the Mac and iOS apps

- Status: accepted
- Date: 2026-10-19
- Supersedes the icon parts of `M9-4-accessibility-and-app-icon` and `IOS-fix-ipad-launch`

## Context

The owner drew a new icon in Icon Composer: a monitor over Drift's waves on a deep-blue
gradient, with Liquid Glass translucency and shadow. The Mac bundle used PNGs and an `icns`
rendered from `icon.svg`, and iOS used a separate PNG appiconset rendered from
`icon-ios.svg`. Both SVGs drew the old waves-only design.

## Decision

1. **One source.** `src-tauri/icons/AppIcon.icon` (the Icon Composer file: `icon.json` and its
   layer SVGs) replaces `icon.svg` and `icon-ios.svg`.
2. **Mac.** `tauri.conf.json` bundles `icons/AppIcon.icon`. The Tauri CLI compiles it with
   actool (Xcode 26 or later) into `Contents/Resources/Assets.car` and sets `CFBundleIconName`.
   The Mac App Store build inherits the list. `icon.icns` and the PNGs stay as the flat fallback
   (`CFBundleIconFile`).
3. **iOS.** The Xcode target takes `../../icons/AppIcon.icon` as a resource (`project.yml`, and
   the same four entries in the tracked `project.pbxproj`). `ASSETCATALOG_COMPILER_APPICON_NAME`
   stays `AppIcon`. actool renders the flat icons for iOS 17 to 25. The PNG
   `AppIcon.appiconset` is deleted because a set with the same name would compete with the file.
4. **Flat renders.** `icon.icns` (16 to 1024 px) and the 32, 128, 256 and 512 px PNGs are
   rendered from the compiled `Assets.car`: build it with
   `xcrun actool src-tauri/icons/AppIcon.icon --compile <dir> --app-icon AppIcon --platform macosx --target-device mac --minimum-deployment-target 27.0 --output-partial-info-plist <dir>/p.plist`,
   put it in a throwaway `.app` with `CFBundleIconName = AppIcon`, draw
   `Bundle(path:).image(forResource: "AppIcon")` at each size with AppKit, then run
   `iconutil -c icns`. actool's own `AppIcon.icns` stops at 256 px, and `ictool` and Quick Look
   cannot export an `.icon` here.
5. **Checks.** `xtask::icons::icon_composer_audit` requires `icon.json` to parse and every layer
   image it names to exist. `audit` requires `tauri.conf.json` to bundle the file. `ios_audit`
   now checks that the project spec and the project both add the file and that no PNG
   appiconset is left. The old PNG checks (size, opacity, Tauri's logo hash) are dropped
   because there is no PNG set left to check.

## Consequences

- Release builds need Xcode 26 or later. With an older actool, Tauri skips `Assets.car` and the
  Mac app shows the flat `icns`.
- Editing the icon means editing `AppIcon.icon` in Icon Composer and re-rendering the flat set
  (decision 4). Nothing yet checks that the flat set matches the file.
- `cargo tauri ios init` would write a PNG appiconset again; `ios_audit` fails if one appears.
