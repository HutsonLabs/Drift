# HOST-15 — Guided setup: set up a Mac from the device

- Status: accepted
- Date: 2026-10-19
- Task: HOST-15 (`TASKS.md`); amends ADRs `HOST-8-pairing` (a second way to pair),
  `IOS-9-pairing` (two optional TXT keys) and `HOST-9-menu-bar` (setup windows)

## Context

Setting up Drift Host took a terminal: `drift-host set-password` (a binary inside the bundle,
not on `PATH`), then `drift-host pair` to see a code. The menu-bar app had no way to store the
password or show a code, so after a DMG install the host sat at "Stopped: no password stored".
On the device, "Pair a Mac" already listed the Macs it found with Bonjour, yet still wanted the
code from the Mac's screen.

## Decisions

1. **Approve on the Mac instead of a code.** A device that picked the Mac from the Bonjour list
   sends `ApprovalRequest { version, name, key, signature }` (`0x0406`) instead of a
   `PairRequest`. The signature uses its own purpose label, `drift-approve-v1`, over the nonce
   and the pinned certificate's fingerprint (as in HOST-8), so a request cannot be replayed to
   another host or connection. The host asks its user, "Allow “iPad” to use this Mac?", and
   pushes `PairReply` when they decide: `Accepted` stores the device as a code would;
   `Declined` (new, 5) and `NoAnswer` (new, 6) refuse it. The code and QR stay as the fallback
   and for older hosts.
2. **Why this is as strong as the code.** The device has already passed TLS against the pinned
   certificate and NLA with the Mac account's password. The code proved that someone stood at
   the Mac; Allow proves the same thing, and that person also sees the device's name. A request
   whose signature does not verify never reaches the prompt (it is answered `Declined`). "Don't
   Allow" counts like a wrong code: five in a row lock pairing for a minute, so nobody can flood
   the Mac with prompts. **Don't Allow is the default button**: Return never lets a device in.
3. **Negotiated, not assumed.** Both ends offer `Capabilities::APPROVAL` (bit 6). A client asking
   an older host (no bit) sends nothing and reports `ApprovalUnsupported` ("enter the code it
   shows"). The Bonjour TXT gains optional `approve=1` and `user=<account>`, so the device knows
   before connecting whether a code is needed and fills in the account. Older clients ignore
   unknown keys (IOS-9 decision 1).
4. **Waiting.** A request moves the connection to `AuthPhase::AwaitingApproval(id)`: nothing is
   shown, input is dropped, and the 15 s authentication deadline becomes
   `APPROVAL_TIMEOUT` (2 min). Then the host answers `NoAnswer` and drops the connection after
   the usual grace. A device that disconnects while waiting is withdrawn from the queue.
   `Pairing::approvals` / `approve` / `decline` / `give_up` are the menu-bar app's interface; the
   answer reaches the device through `ReplySender`, which pushes on `Drift::Mac::v1` (the path
   EXT-4's status already used, now `GfxShared::push_drift_mac`).
5. **The device says what is happening.** While asking, the client holds the activated leg's
   `Connected` state back and reports `Connecting { stage: Approval }` ("Click Allow on the Mac
   to finish pairing…"), then enters `Connected` once the Mac accepted. "Pair" in the sheet
   saves the profile and connects straight away, because approval happens on that first
   connection.
6. **Guided setup in the menu-bar app** (pure `model::setup_step` / `auto_show`):
   Screen Recording → Accessibility (the HOST-9 first-run alert) → the Mac's password → "Set Up
   a Device". When the step changes (at launch, after a permission is granted, after the
   password is saved) the next window opens by itself, once; the menu keeps "Enter Your Mac's
   Password…", "Set Up a Device…" and "Update Mac Password…". Local Network still has no query
   (HOST-9 decision 5); macOS asks when the host first advertises.
7. **The password** is asked for in an alert with a secure field and checked against the local
   directory (OpenDirectory `verifyPassword`) before it is saved, so a typo is caught on the Mac
   rather than as a failed sign-in on the device. Network accounts the local node cannot check
   are saved unchecked. Saving restarts the host with it. The app writes the Keychain item
   itself, which also avoids the "User interaction is not allowed" read of an item the
   `drift-host` command created.
8. **"Set Up a Device"** is a plain AppKit window (`setup_window`): four steps, the Mac's name,
   address, account and "this Mac's login password", and a fresh code with its QR code
   (`HostAnnouncement::pairing_qr_modules`, drawn as an 8-bit gray bitmap at 2 px per point),
   with Copy Pairing Link, New Code and Done. It closes when a device is approved.

## Consequences

- Red tests: `drift-ext/tests/approval.rs`, `drift-host/tests/approval.rs`,
  `approval_e2e.rs` (loopback: Allow after the old deadline shows the desktop, Don't Allow and
  no answer refuse, reconnect authenticates), `approval_advert.rs`,
  `drift-core/tests/approval_advert.rs`, `src-tauri/tests/pairing_approval.rs`,
  `drift-host-app/tests/setup.rs` and `ui/test/pairApproval.test.ts`. The window, the password
  check and the QR bitmap have AppKit smoke tests; clicking through the real prompts is in
  `docs/acceptance.md`.
- `HostAdvert`, `PairHostRequest`, `PairStatus`, `ConnectStage` and `HostAnnouncement` gained
  variants or fields (cross-stream change); `HostAdvert`'s new fields default when absent.
- The Mac account's short name is now in the Bonjour advert on the local network.
