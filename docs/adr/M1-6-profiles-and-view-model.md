# M1-6 / M3-2 — Profile store, credentials UX and the webview view model

- Status: accepted
- Date: 2026-09-22
- Code: `crates/drift-core/src/{store,messages}.rs`, `crates/drift-core/src/profile.rs`
  (`validate`, `SecretRole`), `src-tauri/src/{profiles,secrets,view,commands}.rs`, `ui/src/`

## Decisions

1. **`profiles.toml`** in the Tauri app config directory
   (`~/Library/Application Support/com.example.drift/`): `version = 1` plus a
   `[[profiles]]` array of `ConnectionProfile`s. The pure document model (`ProfileStore`) lives in
   `drift-core`; file I/O (atomic temp-file + rename) in `drift-app::profiles::ProfileFile`.
   Structurally broken files are errors; profiles that merely fail validation still load so the
   user can fix them.
2. **Validation in Rust.** `drift_core::profile::validate` returns *all* issues as
   `{field, problem, message}`; the UI only marks fields. `save_profile` validates again.
3. **Linux-password opt-in = stored secret.** The plan's §3 `ConnectionProfile` contract is not
   extended. A Remote Login profile opts in to Drift typing the Linux password at the greeter
   exactly when a `linux-login` secret is stored for it (checkbox "Type my Linux password at the
   login screen"); unticking deletes it. Headless / Desktop Sharing profiles never keep one.
   Changing the mode moves the RDP password between the `rdp-system` and `rdp-user` roles.
4. **Secret store seam.** `drift-app::secrets::SecretStore` (get/set/delete by profile id + role;
   Keychain account `<uuid>/<role>` via `SecretRole::account`). Until the Keychain adapter from
   `drift-macos` (M3-2, stream C) is wired, the app uses `MemorySecretStore` — passwords last for
   the process lifetime only. Plugging in the Keychain is a one-line change in `drift_app::run`.
5. **Rust decides the screen.** `drift-app::view::SessionView` folds `SessionEvent`s into a
   snapshot with `screen` (`profiles | connecting | certificate | greeter-hint | reconnecting |
   error | live`), the pending certificate prompt (with the per-mode `grdctl … status` command),
   the `resuming` flag for the greeter hint and an `ErrorExplanation`. It is emitted to the
   session's window as the tauri-specta event `SessionViewChanged`; the webview listens on its
   own window only and renders.
6. **Error texts in Rust.** `drift_core::messages::explain_disconnect(reason, mode)` holds the
   M9-4 wording and next steps (table-tested), including the Local Network Privacy screen whose
   action opens `x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork`
   via `NSWorkspace` (`open_local_network_settings`).
7. **Session intent commands exist before the SessionManager.** `connect`, `accept_certificate`,
   `reject_certificate`, `reconnect_now`, `cancel_reconnect`, `close_session` take the calling
   `tauri::Window` (the SessionManager maps window → session) and currently return
   `CommandError::NotImplemented`; M6-1 replaces the bodies without changing the bindings.
