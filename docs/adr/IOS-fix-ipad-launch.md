# IOS-fix — iPad launch crash and the iOS app icon

Status: accepted (2026-10-19). Fixes TestFlight 26.10 (4).

## Context

- **Crash.** Build 4 crashed as soon as it opened on iPad. A release build in an iPad simulator
  reproduced it: `EXC_BAD_ACCESS` in `objc_retain` under
  `-[UIApplication _connectUISceneFromFBSScene:transitionContext:]`, logged just after
  "Info.plist contained no UIScene configuration dictionary (looking for configuration named
  "TaoScene")". Debug builds did not crash. tao 0.35.3's
  `application:configurationForConnectingSceneSession:options:` built a `UISceneConfiguration`,
  returned `Retained::as_ptr(&config)` and then dropped `config`, so UIKit received a freed
  object. tao has been at 0.35.3 since the workspace began, so earlier iOS builds carried the bug
  too; release optimisation decides whether the freed object still looks valid when UIKit
  retains it.
- **Icon.** `src-tauri/gen/apple/Assets.xcassets/AppIcon.appiconset` still held the Tauri logo
  that `cargo tauri ios init` wrote in IOS-5; the M9-4 icon work covered the Mac bundle only.

## Decisions

1. **Vendor tao with the fix.** `third_party/tao` is the crates.io 0.35.3 source with one patch
   (`DRIFT-PATCHES.md`): the method returns `Retained::autorelease_return(config)`. The
   workspace uses it through `[patch.crates-io]`. Drop the copy once a tao release has the fix.
2. **A full-bleed iOS icon.** `src-tauri/icons/icon-ios.svg` draws `icon.svg`'s waves on the
   whole square (iOS masks the corners itself). The appiconset is generated from it with
   `cargo tauri icon` and flattened to opaque RGB. `xtask::icons::ios_audit`, run by the gate,
   checks every image `Contents.json` lists: pixel size, no alpha, and not Tauri's default logo
   (by SHA-256).

## Consequences

- Checked in iPad and iPhone simulators with a release build: the app stays up at launch, and
  the bundle's icons are Drift's waves.
- The gate does not launch a release build, so a crash like this one still needs a simulator run
  before a release (`docs/acceptance.md`, IOS section).
