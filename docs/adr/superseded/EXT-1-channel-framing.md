# EXT-1 — `Drift::Mac::v1` framing and negotiation

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Task: EXT-1 (`TASKS.md`); follows ADR `IOS-0` decision 3

## Decisions

1. **A crate of its own, `drift-ext`.** Both the client (`drift-rdp`) and Drift Host need the
   framing and a DVC processor; neither should depend on the other.
2. **Framing.** One message per DVC payload (DRDYNVC reassembles fragments): a 12-byte header
   (`DRFT`, version 1, reserved flags 0, `u16` type, `u32` length) and the payload. Decoding is
   strict: wrong magic, an unknown framing version, reserved flags, a length that is not exactly
   what follows, or more than 1 MiB is refused. A future version 2 needs a new channel name or a
   negotiated switch, not a silent change.
3. **Types by task.** The high byte of the type names the owner: `0x00` negotiation, `0x01`
   scroll and gestures (EXT-2), `0x02` menus (EXT-3), `0x03` host status (EXT-4), `0x04`
   pairing (HOST-8), `0x05` codecs (EXT-5). Only `Hello` (`0x0001`) is decoded here; every
   other type stays `Message::Unknown` with its payload until its task decodes it.
4. **Negotiation.** Each end sends a `Hello` (min and max protocol version, a capability bit
   set, an agent string for logs) as soon as the channel starts. The highest common version and
   the intersection of capabilities win. Unknown capability bits are kept and never match.
5. **Fallback.** Dynamic channels are opened by the server, so GNOME and other servers simply
   never open it: the client's `Link` stays `Absent` and the session is plain RDP. A malformed
   message, or no common version, switches the extension off (`Failed`) without a DVC error,
   because ironrdp ends the whole RDP session on one.
6. **Session wiring.** `SessionOptions::drift_mac` carries a `Link` (shared state + inbox); the
   app passes one for every session, and it is reset at each connection leg. The client offers
   no capability bits yet; each EXT task turns its own on.
7. **Fuzzing.** `crates/drift-ext/fuzz` (`drift_mac`) decodes, round-trips and feeds frame
   sequences through a client processor, which must never error. It runs nightly in both
   `.github/workflows` and `.workflows`, and `cargo xtask fuzz-targets` requires it.

## Consequences

- FakeServer can open the channel (`Channels::with_drift_mac`) and records what its end
  negotiated (`LegRecord::drift_mac_negotiated`); the loopback test covers both cases.
- Sending messages outside the handshake (from the session loop) arrives with the first task
  that needs it (EXT-2 or HOST-8), like the GFX frame acknowledgements do today.
