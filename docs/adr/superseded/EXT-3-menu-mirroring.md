# EXT-3 — Menu mirroring

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Task: EXT-3 (`TASKS.md`); builds on ADR `EXT-1-channel-framing` and ADR `HOST-8-pairing`

## Context

ADR `IOS-0` decision 3 plans to show the remote Mac app's menus in the iPadOS 26 menu bar. The
host must read the frontmost app's menus, send them over `Drift::Mac::v1`, and press an item
when the iPad chooses one.

## Decisions

1. **The client polls; the host never pushes.** A DVC processor can only send in reply to what
   it receives, and pushing from the host would need a new `ServerEvent` in the vendored
   `ironrdp-server`. Instead the client sends `Request { known_revision }` once a second while
   its session is in front and visible; the host answers `Snapshot`, `Unchanged` or `Empty`. A
   press is `Press { revision, id }` with no reply. A menu that shows within a second is soon
   enough.
2. **Messages are `0x0201`–`0x0205`** (`drift_ext::menus`). A snapshot is a revision, the app's
   name and a tree of items (id, flags, title, optional key and modifiers, children). Decoding is
   strict: at most 8 levels, 4096 items, 256-byte titles, 16-byte keys, known flag bits only,
   no trailing bytes. The `drift_mac` fuzz target also decodes menu messages and drives the
   client's mirror.
3. **Ids are depth-first indices of one revision.** The host bumps the revision only when the
   menu's content hash changes, so ids stay stable while the menu does. A press for an older
   revision is ignored.
4. **The host reads through Accessibility, lazily.** `AxMenus` reads on a worker thread when the
   frontmost app changes, and otherwise at most every 2 s, and only while a client has polled in
   the last 3 s. Each element is one `AXUIElementCopyMultipleAttributeValues` call, the app's
   messaging timeout is 0.5 s, and a reading stops at 8192 elements. The Apple menu is left out:
   it belongs to the iPad. An untitled, disabled item without a submenu is a separator. No
   `AXObserver`: it needs a run loop per app and would still miss most menu changes, which apps
   make when a menu opens.
5. **Only an authorized connection sees or presses menus** (HOST-8): the responder answers a
   stranger with nothing. Pairing and menus share the host's channel end through a new
   `channel::Chain` of responders. `host.toml` has `menus = true` by default; without
   Accessibility the reader finds nothing and the host logs it.
6. **The iPad builds menus with `UIMainMenuSystem`'s build handler.** tao owns the app delegate,
   so `buildMenuWithBuilder:` is not available; `UIMainMenuSystem.sharedSystem
   setBuildConfiguration:buildHandler:` (iOS 26) is. While a snapshot is shown, the handler
   removes Drift's File, Edit, Format, View, Window and Help menus and inserts the remote menus
   after Drift's app menu; a new snapshot calls `setNeedsRebuild`. The menu bar shows the menus
   of the session window in front. iPhones have no menu bar and install nothing.
7. **Items are `UIAction`s; shortcuts are subtitles.** Hardware keys already go to the Mac
   (IOS-6), so a `UIKeyCommand` would fire the shortcut twice. Separators become inline groups,
   since UIKit has no separator item.
8. **No mirroring on Mac clients.** Drift's own menu bar is the Mac's, and Command shortcuts
   already reach the remote app. The Mac app does not offer the capability.

## Consequences

- Verified: model and wire tests, the responder's authorization gate, a real Accessibility read
  of this Mac's frontmost app (4 menus, 77 items), a loopback session that receives the menu,
  presses an item and sees a new revision, the iPad menu-bar builder in a simulator, and the iPad
  app showing the Mac's menus from `cargo run -p drift-host --example menu_host` (log line
  `remote menus in the menu bar … menus=4`).
- Not verified: choosing an item from the real iPadOS menu bar (the simulator shows it only
  under a pointer), and apps with very large menus. Both are steps in `docs/acceptance.md`.
- A menu change shows up to about 3 s late (poll plus reread). Pushing would need the vendored
  server to send DVC messages from outside a reply.
