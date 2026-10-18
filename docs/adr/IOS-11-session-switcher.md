# IOS-11 — Session switcher on iPhone and iPad

- Status: accepted
- Date: 2026-10-18
- Task: IOS-11 (`TASKS.md`); follows ADR `IOS-5` decision 2 and the IOS-1 spike

## Context

UI-windows gives every connection its own window. On iPad each Tauri window is its own scene
(IOS-1), so iPadOS can show and switch them. On iPhone extra windows stack in the app's one
scene, and nothing on screen leads from one to another or back to Connections.

## Decisions

1. **Keep scenes on iPad, add a switcher everywhere.** Every session window gets a round button
   in its top-leading corner (inside the safe area) whose menu, shown on tap, lists Connections
   and every session in opening order with its status, the current window checked
   (`drift_app::switcher::entries`, pure; `drift_ios::switcher::SwitcherButton`, a `UIButton` with
   a `UIMenu`). It is native, not web, because the page is hidden while the picture is live. The
   menus are rebuilt whenever the gallery state is pushed. Connections already lists the open
   windows, so it gets no button.
2. **Bringing a window forward is Drift's own.** tao 0.35's `set_focus` asks iPadOS to activate
   a scene *without naming the window's session*, which opens a new, empty scene (seen as a black
   screen in the simulator). `drift_ios::webview::bring_to_front` activates the window's own
   scene session on iPad and calls `makeKeyAndVisible` on iPhone. `focus_window` uses it for
   every window, Connections included.
3. **A scene closed from the app switcher closes its session.** tao ignores
   `sceneDidDisconnect:`, so `AppObserver::scene_disconnected` listens for
   `UISceneDidDisconnectNotification`; a session window whose scene went away is closed like
   Disconnect. If the Connections scene is closed, the window is rebuilt (a new scene) the next
   time Connections is asked for.
4. **Simulator runs are scripted.** The simulator cannot be tapped from the command line, so the
   `smoke-env` build also reads `DRIFT_SMOKE_SCRIPT` (`wait:<s>;connect:<profile>;pick:<label>`)
   and drives the same `connect_profile` and switcher `pick` paths a tap would.

## Consequences

- Verified in iPhone 17 Pro and iPad Pro 13-inch simulators against `fake_host` with two
  profiles: picking `session-0` brings that session's window in front (logged
  `front=["session-0"]`), and picking Connections shows the gallery with both live sessions.
- Tapping the button and its menu, and closing a scene from the iPad app switcher, are manual
  steps in `docs/acceptance.md`.
- Drift relies on tao's window-per-scene behaviour on iPad; if a later tao fixes `set_focus`,
  `bring_to_front` can go.
