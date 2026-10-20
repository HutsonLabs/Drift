# IOS-9 — Pairing with a Mac host, client half

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Task: IOS-9 (`TASKS.md`); follows ADR `IOS-0` decision 7 and decision 3's pairing row. The
  host half is HOST-8; the channel is EXT-1.

## Context

Drift Host pairs a device once and pins keys from then on (ADR `IOS-0` decision 7). The device
needs a way to find the Mac, take its code, pin its certificate and keep a key of its own. HOST-8
and the `Drift::Mac::v1` channel (EXT-1) do not exist yet, so this task defines the formats and
the client side so the host can be built against them.

## Decisions

1. **Bonjour type `_drift-host._tcp`.** Not `_rdp._tcp`: the TXT record carries Drift fields,
   and GNOME hosts do not advertise over Bonjour anyway. TXT keys: `v` (pairing version, 1),
   `id` (host UUID), `host` (the Mac's `.local` name), `fp` (SHA-256 of the host certificate,
   64 hex digits), optional `name` (default: the service name) and `port` (default 3389). The
   host name is in the record, so no endpoint resolution is needed. Records with a newer `v`
   or missing fields are skipped. Both apps declare the type in `NSBonjourServices`.
2. **Browsing with `nw_browser`** (`drift_apple::bonjour::HostBrowser`), shared by macOS and
   iOS; `Advertisement` (`DNSServiceRegister`) is what HOST-8 will use and what the smoke test
   and the `advertise_host` example use now. Discovery runs only while the pairing sheet is
   open (`start_host_discovery` / `stop_host_discovery`, event `HostsDiscovered`).
3. **The code**: seven random digits and a Damm check digit, shown `1234 5678`. The check
   digit catches every single typo and every swap of neighbouring digits before anything is
   sent. Spaces and dashes are ignored.
4. **The QR code is a link**:
   `drift-pair:1?id=<uuid>&name=<pct>&host=<pct>&port=<n>&fp=<64 hex>&code=<8 digits>`. It names
   the host and its certificate, so no Bonjour list is needed. Parsing is strict (scheme,
   version, every field once and valid); unknown fields are ignored for later versions. Both
   apps register the `drift-pair` URL scheme, so the system Camera (or a click on a Mac) opens
   Drift straight into "Pair a Mac" (`RunEvent::Opened`; a link that launches the app waits in
   `take_pairing_link` until the page asks).
5. **Scanning**: on iOS, AVFoundation's metadata output in a full-screen view with Cancel
   (`drift_ios::qr`; VisionKit's scanner is Swift-only). `NSCameraUsageDescription` is set. A
   Mac has no scanner: the user types the code or pastes the link.
6. **A paired profile** is a Headless-mode profile with `cert_pin` set from the pairing and the
   new `paired_host: Option<Uuid>` field. `paired_host` is omitted from `profiles.toml` when
   `None`, so GNOME profiles and older files are unchanged. `ProfileStore::set_pin` refuses to
   clear or replace a paired host's pin (no TOFU prompt, no "forget"); pairing again is the
   only way. Duplicating a paired profile drops the pairing. Pairing the same host again
   updates its profile in place.
7. **Keys in the Keychain** (`drift_apple::pairing`): a P-256 device key (PKCS#8 from `ring`,
   kept as a generic item `pairing/device-key`) and each host's fingerprint
   (`pairing/host/<uuid>`), in Drift's Keychain service next to the passwords; the accounts can
   never collide with `<uuid>/<role>`. Not the Secure Enclave yet: enclave keys cannot be stored
   as items and do not exist in the simulator; the interface stays when it moves there.
8. **Connecting a paired profile** checks that its pin still equals the Keychain record
   (`PairingService::check_before_connect`, in the `connect` command); the existing pin check
   in `drift-rdp` then compares the live certificate before any credential is sent.
9. **The handshake is modelled, not run.** `drift_core::pairing::PairingFlow` checks the TLS
   fingerprint against the pin before the code is sent, then expects `Accepted` or
   `Rejected(WrongCode | Expired | TooManyAttempts)`. Its messages travel on `Drift::Mac::v1`
   (EXT-1), which is not built. Until HOST-8, "Pair" records the host on the device only:
   the Mac never sees the code or the device key, and the session still authenticates with the
   Mac account's password (NLA).

## Consequences

- HOST-8 must advertise the TXT record above, show the code and the `drift-pair:` QR, and answer
  `PairRequest` on EXT-1's channel; the client side then wires `PairingFlow` into a connection.
  *Done in HOST-8 (ADR `HOST-8-pairing`): the live path uses `drift_ext::pairing::ClientPairing`
  and `drift-rdp`'s pin check instead of `PairingFlow`.*
- `ConnectionProfile` gained a field (cross-stream change); old files load unchanged.
- The iOS app links AVFoundation, and both apps' `Info.plist` gained `NSBonjourServices` and
  `CFBundleURLTypes`; `tests/app_config.rs` checks them.
