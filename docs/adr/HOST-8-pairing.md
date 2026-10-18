# HOST-8 — Pairing and device keys

- Status: accepted
- Date: 2026-10-18
- Tasks: HOST-8 (host side) and the rest of IOS-9 (client side); ADR `IOS-0` decision 7,
  ADR `IOS-9-pairing` (formats), ADR `EXT-1-channel-framing` (the channel)

## Context

IOS-9 built the client's half: Bonjour discovery of `_drift-host._tcp`, the eight-digit code
with a Damm check digit, the `drift-pair:` QR link, a P-256 device key and host pins in the
Keychain. HOST-2's host served any client that knew the account's password, and only on
loopback. IOS-0 says pairing is on by default, unpaired devices are refused, and the listener
binds beyond loopback only once that holds.

## Decisions

1. **The exchange runs on `Drift::Mac::v1` after TLS and NLA.** Both ends offer
   `Capabilities::PAIRING`. Once the Hellos agree the host sends a `Challenge` with 32 fresh
   random bytes. The client answers with a `PairRequest` (version, code, device name, public key,
   signature) the first time and an `AuthRequest` (public key, signature) afterwards. The host
   answers `PairReply` or `AuthReply`. Types are `0x0401`–`0x0405`, in EXT-1's pairing range.
2. **What is signed.** ECDSA P-256 / SHA-256 over a purpose label (`drift-pair-v1` or
   `drift-auth-v1`), the nonce, and the SHA-256 fingerprint of the host certificate the client
   pinned. The client checks that pin during TLS before anything is sent (`drift-rdp`'s existing
   pin check), so a mismatched host never sees the code, and a signature cannot be replayed to
   another host or connection. Binding to the TLS session itself (an exporter) would be stronger;
   ironrdp does not expose one, and the pinned certificate plus a fresh nonce already stop relay
   through a host with a different certificate.
3. **Keep NLA.** The account password (HOST-2) still signs the session in, so a device needs both
   its key and the Mac account. Dropping NLA would need an ironrdp server that skips CredSSP,
   and a stolen device would then be enough on its own.
4. **Nothing before authorization.** Until the device paired or proved its key, the host draws
   nothing (no EGFX frame) and drops input. A refusal is answered, then the connection closes
   after 400 ms so the answer reaches the client. A client that never authenticates (any plain RDP
   client, or a Drift client without pairing) is dropped after 15 s.
5. **The code.** Seven random digits (system RNG) and a check digit, valid for five minutes, used
   once. Five wrong codes in a row lock pairing for a minute, after which the window is closed.
   A request whose signature does not verify is answered as a wrong code without checking it.
   Codes are compared in constant time.
6. **Paired devices** live in `devices.toml` in the host's config directory: id, name, public key
   (hex), time. Pairing the same key again replaces its entry. `drift-host devices` lists them and
   `drift-host forget ID` removes one.
7. **Listener.** `pairing = true` is the default and `listen` now defaults to `0.0.0.0:3389`.
   With `pairing = false` the host binds loopback only, whatever `listen` says
   (`HostConfig::effective_listen`).
8. **Advert and QR.** With pairing on the host registers `_drift-host._tcp` with the TXT record
   IOS-9 defined (`v`, `id`, `name`, `host`, `port`, `fp`). `drift-host pair` opens a pairing
   window and prints the code, the `drift-pair:` link and a QR code drawn in the terminal (the
   `qrcode` crate, MIT/Apache-2.0). HOST-9's menu-bar app shows the same through
   `Pairing::open_window` and `HostAnnouncement`. `host_id` is created on first start and saved.
9. **Client side.** `drift_ext::pairing::ClientPairing` answers the challenge and records the
   outcome; `SessionOptions::pairing` hands it to every connection leg. The app's
   `PairingService::client_pairing` gives a paired profile's next connection the code once, then
   the device key alone after the Mac accepted. A refusal ends the session with the new
   `DisconnectReason::PairingRefused(text)`, which is not retried. IOS-9's `PairingFlow` model is
   not used on the live path; the pin check and `ClientPairing` replace it.

## Consequences

- `crates/drift-host/tests/pairing_e2e.rs` runs the whole flow on loopback: pair with the code,
  reconnect with the key, refuse an unknown device and a wrong code, stop at TLS on a pin
  mismatch without spending an attempt, and drop a plain client after the timeout.
- The host is reachable from the local network by default. Its threat model is TLS with a pinned
  certificate, the account password, and a device key the Mac approved in person.
- Unpairing on the device (forgetting the host) and on the Mac are separate; a forgotten device
  is refused on its next connection with "This device is not paired with this Mac."

## Clipboard and sound (added when HOST-11 merged)

- The clipboard and the sound are held back like the picture and input: until the connection's
  device paired or authenticated, the host ignores the client's copies, announces none of its
  own, answers its data requests with an error, and sends no sound (`drift_host::gate::Authorized`).
  A clipboard channel that came up early starts once the device is authorized.
- Found when merging: before this, an unpaired device that passed NLA could write the Mac's
  pasteboard. `tests/auth_gate.rs` covers it.

