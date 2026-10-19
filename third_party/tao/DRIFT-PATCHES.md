# tao 0.35.3 — Drift patches

The crates.io release of tao 0.35.3, copied unchanged except for the patches below, and used
through `[patch.crates-io]` in the workspace `Cargo.toml`. Drop this copy once a tao release
carries the fix.

1. **iOS scene configuration returned already released** (`src/platform_impl/ios/view.rs`,
   `configuration_for_connecting_scene_session`). The delegate method returned
   `Retained::as_ptr(&config)` and then dropped `config`, so UIKit received a freed
   `UISceneConfiguration`. Release builds of Drift crashed at launch on iPad with
   `EXC_BAD_ACCESS` in `objc_retain` under `-[UIApplication _connectUISceneFromFBSScene:…]`
   (TestFlight 26.10 (4)). Fixed by returning it autoreleased:
   `Retained::autorelease_return(config)`.
