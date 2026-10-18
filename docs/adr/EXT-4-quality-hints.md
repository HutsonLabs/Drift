# EXT-4 — Quality hints on `Drift::Mac::v1`

- Status: accepted
- Date: 2026-10-18
- Task: EXT-4 (`TASKS.md`); builds on ADR `HOST-7-link-hygiene` (the backlog meter) and ADR
  `HOST-4-realtime-encoder`

## Context

HOST-7 measures how far behind a client is. RDP has no way to say "you are falling behind", so
the host has to act on it itself and tell a Drift client what it did (ADR `IOS-0` decision 3).
A client away from the host's network also wants a lower start than one on the same Wi-Fi.

## Decisions

1. **The host decides; the client caps.** The host steps down a four-level ladder (`Best`,
   `High`, `Medium`, `Low`) from its own backlog and never goes above the client's `max`. The
   client never measures the link itself; the host sees the queue directly.
2. **Policy** (`drift_host::quality`, pure): `Backlogged` steps down at once and then at most
   once every 2 s; `Building` for 3 s steps down; `Clear` for 10 s steps up one level. Slow up,
   quick down: a stall is worse than a softer picture.
3. **Levels** change the encoder's average bit rate (100 %, 70 %, 45 %, 25 % of HOST-4's
   real-time rate, never under 1 Mbit/s) and, for `Medium` and `Low`, halve the frame rate.
   Fewer pictures leave more bits for each one. The bit rate changes on the running
   VideoToolbox session (`VtEncoder::set_average_bitrate`); the frame-rate cap is a pacer that
   leaves frames in the capture mailbox, which merges their damage. The codec is not stepped:
   AVC444 or AVC420 is chosen when the session starts (ADR `AVC444-client`).
4. **Messages** (`drift_ext::status`, types `0x0301`, `0x0302`): the client sends
   `QualityPreference { max, away }` as soon as both ends agreed on `STATUS`; the host answers
   with `HostStatus { backlog, quality, bitrate_kbps, fps }` and sends a new one on every level
   change. The host sends on its own through DRDYNVC with the channel id the link now records.
5. **Away from the local network** gets `max = Medium` by default. The client decides from the
   profile's host: loopback, RFC 1918, link-local and IPv6 unique-local addresses, single-label
   names, `.local`, `.lan` and `.home.arpa` names are local; any other address or name is away.
   The CGNAT range `100.64.0.0/10` counts as away because VPNs such as Tailscale use it. A
   cellular-only path cannot reach a LAN host without a VPN, so the address rule covers it.
6. **The client shows it.** `SessionStats::host_quality` carries the host's level, and the
   statistics HUD adds "medium quality" (or the current step) for a Drift Host.

## Consequences

- The policy and its timing are unit tested, the pump test shows a backlogged link stepping the
  source down once, and a loopback test shows an away client's cap reaching the host and the
  host's status reaching the client's statistics.
- Not tested end to end: a real slowed link stepping down and back up through the encoder; it
  is a manual step in `docs/acceptance.md`.
- There is no user setting for the cap yet; `SessionOptions::quality` takes one when the app
  grows a control for it.
