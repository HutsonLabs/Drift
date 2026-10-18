# IOS-3 — `drift-apple` shared by `drift-macos` and `drift-ios`

- Status: accepted
- Date: 2026-10-18
- Task: IOS-3 (`TASKS.md`); follows ADR `IOS-0` decision 8

## Context

`drift-macos` mixed AppKit glue with services that work the same on iOS: the Keychain
adapter, `nw_path_monitor` and the Local Network (errno 65) check. The iOS client needs those
without AppKit.

## Decisions

1. **`drift-apple`** holds `keychain`, `network` (`PathMonitor`, `PathStatus`, `TriggerFeed`)
   and `lnp` (`EHOSTUNREACH`, `is_local_network_denied`, the macOS settings URL constant). It
   depends on no UI framework and builds for every Apple target (`#![cfg(target_vendor =
   "apple")]`).
2. **`drift-macos` keeps its public paths.** `drift_macos::keychain`, `drift_macos::Keychain`,
   `drift_macos::network::{PathMonitor, PathStatus, TriggerFeed}` and the `lnp` functions are
   re-exports, so `drift-app` did not change. `WakeObserver`, `ReconnectTriggers` and
   `open_local_network_settings` stay in `drift-macos` because they use `NSWorkspace`.
3. **Keychain on iOS.** iOS has no keychain files, so `Keychain::create_file`/`open_file` are
   macOS-only and the iOS build uses the app's data-protection keychain through
   `security_framework::passwords`. `contains` stays attribute-only. The Keychain tests stay
   macOS-only: an unsigned test binary in the simulator has no keychain entitlement.
4. **The iOS "wake" trigger is the foreground.** iOS has no system-wake notification for apps,
   and it suspends a backgrounded app's sockets, so `drift-ios::ForegroundObserver` sends
   `Trigger::Wake` on `UIApplicationWillEnterForegroundNotification`. `drift_ios::ReconnectTriggers`
   mirrors the macOS one.
5. **Local Network settings on iOS** open the app's own page in Settings
   (`UIApplicationOpenSettingsURLString`, `app-settings:`); iOS has no deep link to
   Privacy › Local Network, and the switch for Drift is on the app's page. iOS reports the
   denial as the same `EHOSTUNREACH`, so detection is shared.
6. **Gate.** `drift-apple` and `drift-ios` join `ci_plan::IOS_CRATES` (clippy for both iOS
   targets, simulator tests linked). `objc2-ui-kit` may only appear under
   `[target.'cfg(target_os = "ios")'.dependencies]`, the mirror of the AppKit rule; both are
   asserted in `xtask/tests/ci_plan.rs`.

## Consequences

- `drift-ios` tests (`tests/foreground.rs`) and the shared `drift-apple` tests pass in an iOS 27
  simulator via `xcrun simctl spawn`; `cargo xtask ci` only links them.
- The Keychain on iOS is verified in the app (IOS-5 / IOS-9), not by unit tests.
