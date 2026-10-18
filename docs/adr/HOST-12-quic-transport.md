# HOST-12 — UDP for Drift-to-Drift sessions: QUIC with BBR, not RDP-UDP

- Status: accepted
- Date: 2026-10-18
- Task: HOST-12 (`TASKS.md`); follows ADR `IOS-0` decision 6, answers its open question 6

## Context

Decision 6 of ADR `IOS-0` makes UDP a first-class transport for Drift Host: first choice the
vendored `ironrdp-rdpeudp` (MS-RDPEUDP2, "lossy mode for GFX"), and QUIC between Drift ends if
that does not hold up on lossy Wi-Fi.

## What `ironrdp-rdpeudp` offers

- The vendored crates implement **reliable** RDP-UDP only. `ironrdp-rdpeudp-tokio`'s README says
  it plainly: "lossy RDP-UDP-L, FEC, and DTLS are not implemented". `SYNLOSSY` exists only as a
  flag constant. So the "lossy mode for GFX" the ADR assumed is not there to use.
- Reliable RDP-UDP2 is an ordered byte stream with TLS on top: a lost packet stalls everything
  behind it, like TCP.

## Measurement

`crates/drift-transport/examples/udp_loss_bench.rs` sends 300 frames of 20 000 bytes at 60 fps
(9.6 Mbit/s, a 1080p desktop in motion) from host to client through
`drift_transport::lossy::LossyUdpProxy` on loopback: 10 ms each way (20 ms RTT), deterministic
random loss, packets kept in order. Latency is from the host queuing a frame to the client
holding all of it. Five transports:

- **RdpUdp2:** `ironrdp-rdpeudp-tokio` (RDPEUDP2 + TLS + RDPEMT), one message per frame.
- **QuicOrderedCubic / QuicOrderedBbr:** QUIC, all frames on one ordered stream (TCP-like
  head-of-line blocking), with quinn's CUBIC or BBR.
- **QuicPerFrameCubic / QuicPerFrameBbr:** QUIC, one stream per frame.

Results (2026-10-18, M-series Mac, debug build with optimised dependencies):

| transport | loss | p50 ms | p95 ms | p99 ms | max ms | frames > 100 ms | lost |
|---|---|---|---|---|---|---|---|
| RdpUdp2 | 0% | 13.6 | 20.5 | 33.4 | 52.7 | 0 | 0 |
| QuicOrderedCubic | 0% | 14.8 | 20.8 | 34.2 | 47.4 | 0 | 0 |
| QuicPerFrameCubic | 0% | 14.6 | 24.4 | 38.0 | 64.0 | 0 | 0 |
| QuicOrderedBbr | 0% | 12.7 | 15.3 | 21.8 | 23.7 | 0 | 0 |
| QuicPerFrameBbr | 0% | 12.7 | 15.7 | 18.4 | 23.2 | 0 | 0 |
| RdpUdp2 | 0.5% | 50.0 | 1492.8 | 1601.4 | 1632.2 | 135 | 0 |
| QuicOrderedCubic | 0.5% | 30.0 | 726.3 | 797.4 | 813.7 | 80 | 0 |
| QuicPerFrameCubic | 0.5% | 195.9 | 994.1 | 1024.2 | 1120.6 | 193 | 0 |
| QuicOrderedBbr | 0.5% | 12.9 | 36.2 | 41.8 | 49.2 | 0 | 0 |
| QuicPerFrameBbr | 0.5% | 14.1 | 37.9 | 53.5 | 55.5 | 0 | 0 |
| RdpUdp2 | 1% | 1539.7 | 3583.0 | 3649.5 | 3674.9 | 260 | 0 |
| QuicOrderedCubic | 1% | 934.5 | 1893.2 | 1980.2 | 2015.4 | 246 | 0 |
| QuicPerFrameCubic | 1% | 3030.7 | 4020.8 | 4071.4 | 4084.4 | 248 | 0 |
| QuicOrderedBbr | 1% | 14.8 | 52.2 | 78.3 | 91.8 | 0 | 0 |
| QuicPerFrameBbr | 1% | 13.2 | 39.8 | 52.1 | 68.2 | 0 | 0 |
| RdpUdp2 | 2% | 2853.9 | 4459.5 | 4535.9 | 4572.4 | 280 | 0 |
| QuicOrderedCubic | 2% | 1896.2 | 2302.4 | 2468.2 | 2517.8 | 255 | 0 |
| QuicPerFrameCubic | 2% | 5552.0 | 6314.3 | 6651.5 | 6746.1 | 276 | 0 |
| QuicOrderedBbr | 2% | 15.7 | 46.7 | 57.6 | 89.1 | 0 | 0 |
| QuicPerFrameBbr | 2% | 14.1 | 46.6 | 59.4 | 71.4 | 0 | 0 |
| RdpUdp2 | 5% | 5189.5 | 7080.0 | 7152.2 | 7176.4 | 196 | 87 |
| QuicOrderedCubic | 5% | 3858.3 | 4732.1 | 4871.6 | 4970.2 | 204 | 69 |
| QuicPerFrameCubic | 5% | 8631.6 | 12959.7 | 13384.4 | 13510.1 | 64 | 217 |
| QuicOrderedBbr | 5% | 37.1 | 71.8 | 109.4 | 158.7 | 6 | 0 |
| QuicPerFrameBbr | 5% | 35.0 | 60.9 | 126.8 | 170.5 | 4 | 0 |

Reading it:

- With no loss everything is fine (p99 under 40 ms).
- **Loss-based congestion control collapses.** At 0.5 % random loss RDP-UDP2 and CUBIC can no
  longer carry 9.6 Mbit/s at 20 ms RTT, frames queue up and latency grows to seconds; at 1 % it
  is 1.5 to 4 s. Random Wi-Fi loss is not congestion, but these controllers treat it as such.
  (The Mathis bound for 1 % loss at 20 ms RTT is about 5 Mbit/s.)
- **QUIC with BBR holds:** p99 under 80 ms up to 2 % loss, no lost frames, and 4 to 6 frames
  over 100 ms at 5 % loss.
- One stream per frame against one ordered stream matters much less than the congestion
  controller. With BBR, per-frame streams are a little better at 1 % (p99 52 vs 78 ms, max 68 vs
  92 ms) and similar elsewhere; with CUBIC they are worse, because a backlog is spread across
  many streams at once.

## Decisions

1. **Answer to open question 6: `ironrdp-rdpeudp` is not usable for GFX.** It has no lossy
   mode, and its reliable mode collapses at 0.5 % loss. Drift-to-Drift sessions use the QUIC
   fallback of decision 6. Standard RDP-UDP stays available to a future HOST task for
   third-party clients, but Drift does not depend on it.
2. **Library: `quinn` 0.11** (MIT OR Apache-2.0, with `quinn-proto` and `quinn-udp`), with
   `default-features = false` and `runtime-tokio`, `rustls-ring`, `log`. It uses rustls with
   the ring provider like the RDP TLS path, needs no C code, and passes `cargo deny`. No platform
   verifier: the client pins the host certificate (SHA-256 of the DER, the fingerprint Drift
   already pins; decision 7), and signatures are still checked.
3. **BBR is the default congestion controller** (`drift_transport::quic::Congestion`). quinn
   marks its BBR as experimental; the measurement above is the reason to use it anyway, and
   CUBIC stays selectable.
4. **Wire format (`drift_transport::quic`, ALPN `drift-rdp/1`):** the whole RDP connection
   (X.224, MCS, input, clipboard, every DVC) rides one reliable bidirectional stream that the
   client opens and writes first, so the IronRDP connector and server run over it unchanged.
   GFX frames ride one unidirectional stream each, `id: u32`, `len: u32`, then the bytes (at
   most 32 MiB); the client hands frames on as they complete. Up to 256 frame streams may be open.
5. **Transport order (`drift_transport::plan`):** a paired Drift Host is tried over QUIC, then
   TCP; every other host gets TCP only. Each attempt has a timeout, because a firewall that drops
   UDP makes QUIC wait rather than fail.

## Consequences

- Done here: the measurement, the `drift-transport` crate (QUIC session with pinned host and
  per-frame streams, the fallback policy, the lossy proxy), and tests, including a QUIC session
  at 5 % loss that delivers the RDP stream and 50 frames intact.
- **Not done yet (HOST-12 stays `[~]`):** Drift Host listening on UDP (needs HOST-2), and
  `drift-rdp` connecting over a `QuicSession` main stream and feeding per-frame streams to the
  GFX client. Both ends need the paired-host flag from HOST-8 to know when to try QUIC.
- The RDP TLS upgrade still happens inside the main stream, so a QUIC session is encrypted twice.
  That keeps the connector unchanged; removing it is a later optimisation.
- The host listens on UDP as well as TCP; the threat model and firewall guidance (ADR IOS-0
  consequences) must cover both.
