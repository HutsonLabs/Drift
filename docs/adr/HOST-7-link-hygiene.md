# HOST-7 — Link hygiene: service class, idle heartbeat, backlog

- Status: accepted
- Date: 2026-10-18
- Task: HOST-7 (`TASKS.md`); ADR `IOS-0` decision 6 (both ideas from Sill, reimplemented)

## Decisions

1. **Interactive video on the session socket.** Right after accepting, the host sets
   `SO_NET_SERVICE_TYPE` to `NET_SERVICE_TYPE_VI` on the TCP socket, so macOS marks its packets
   for Wi-Fi's video access category. A failure is logged and the session continues.
   `HostStats::video_service_class` reports it. The QUIC transport (HOST-12) sets the same class
   on its UDP socket when it is wired in.
2. **Idle heartbeat = the last pointer update again.** After 100 ms with nothing sent, the pump
   repeats the last pointer position (or hide) and does so every 100 ms while the link stays
   idle. It is a few bytes, works with every RDP client, and changes nothing on screen: Drift
   never warps its pointer, and other clients move theirs to where it already is. MS-RDPBCGR's
   Server Heartbeat PDU was rejected: its period is in whole seconds, and a phone's radio sleeps
   after a few hundred milliseconds. Before any pointer update is known (the synthetic test
   source has no pointer) no heartbeat is sent.
3. **Backlog from two signals.** Each pump turn samples the GFX frames awaiting acknowledgement
   (`frames_in_flight`) and the bytes waiting in the socket's send buffer (`SO_NWRITE`, read
   through a duplicated descriptor so a closed and reused descriptor is never touched). Levels:
   `Building` at 3 frames or 64 KiB, `Backlogged` at 6 frames or 512 KiB. A level rises after 3
   samples in a row and falls after 5, so one slow frame is not a backlog. The level is in
   `HostStats::backlog` and logged on change; EXT-4 turns it into quality hints.
4. Link hygiene runs in the live pump, so a host showing only the test pattern sends no
   heartbeats; nothing there needs them.

## Consequences

- Unit tests cover the heartbeat timing, the meter's hysteresis and the pump calling both; a
  loopback test checks the service class is set and a fast link stays `Clear`.
- Whether the heartbeat really keeps an iPhone's radio awake, and how much latency it saves, is
  a measurement on a phone over Wi-Fi (`docs/acceptance.md`).
