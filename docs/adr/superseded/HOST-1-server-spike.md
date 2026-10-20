# HOST-1 — `ironrdp-server` with `egfx` as Drift Host's server

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Task: HOST-1 (`TASKS.md`); answers ADR `IOS-0` open question 4

## Context

ADR `IOS-0` decision 2 builds Drift Host on the vendored `ironrdp-server` and `ironrdp-egfx`.
Before HOST-2 writes the real listener, a spike checks what the server half does today against
Drift's own client.

## The spike

`drift_testkit::host_spike::SpikeHost` runs `ironrdp-server` (feature `egfx`) on loopback: NLA
with one account, a fixed 1280×800 display with no bitmap updates, the EGFX bridge
(`GfxServerFactory::build_server_with_handle`) and the Display Control channel the server always
offers. When the graphics channel is ready it creates one surface, maps it to the output and
sends access unit 0 of `fixtures/h264/leg3.h264` with `send_avc420_frame`, then pushes the drained
PDUs through `ServerEvent::Egfx`. `crates/drift-rdp/tests/host1_server_spike.rs` connects
`drift-rdp` to it.

Results:

- **AVC420 works end to end.** Drift negotiates it (the server confirms V8.1 with
  `AVC420_ENABLED`), decodes the frame with VideoToolbox and acknowledges it. The decoded picture
  is identical to ffmpeg's decode of the same unit (PSNR infinite, i.e. every byte equal).
- **Display Control works.** A client `Resize` reaches `RdpServerDisplay::request_layout` as a
  1000×700 primary monitor.

## Answer to open question 4: what `ironrdp-server` lacks

- **AVC444.** The server can *pack* it: `send_avc444_frame` and `send_avc444v2_frame` take a
  luma stream and an optional chroma stream, and it negotiates AVC444 whenever the client offers
  any V10.x capability set. What is missing is everywhere else:
  - an encoder that turns a 4:4:4 picture into the two 4:2:0 streams (HOST-4);
  - **Drift's own client**: `drift-gfx` advertises only `V8_1 { AVC420_ENABLED }` and `V8`
    (chosen for g-r-d, see `drift-gfx/src/caps.rs`) and rejects `Avc444`/`Avc444v2` codec ids
    (`drift-gfx/src/client.rs`). The IOS-0 context line "the client half already has AVC444" is
    wrong. A Drift-to-Drift session needs V10.x caps offered only to Drift Hosts and an AVC444
    decode path (two `VtDecoder`s and a combine pass) before AVC444 can be used.
- **DISP.** The channel and the layout callback exist. The server does not act on a layout:
  the host must resize its capture (or virtual display, HOST-10), then call
  `GraphicsPipelineServer::resize_with_monitors` so the client gets `ResetGraphics` and a new
  surface. `DisplayUpdate::Resize` only drives the legacy bitmap path (deactivation and
  reactivation), which Drift Host will not use.
- **CLIPRDR server role.** Present: `CliprdrServerFactory` gives each connection an
  `ironrdp-cliprdr` `CliprdrServer` with a backend; the host announces its own copies through
  `ServerEvent::Clipboard`. Not exercised by the spike; HOST-11 maps it onto
  `drift-clipboard`.
- **Other gaps for HOST-2:** `RdpServer` and `run_connection` are not `Send` (the spike runs the
  server on its own thread and current-thread runtime); `RdpServer` serves one connection at a
  time (`ConnectionPolicy`); NLA uses one pre-loaded account (`set_credentials`), so per-device
  pairing (HOST-8) needs its own check before or after CredSSP.

## Consequences

- HOST-2 can build on `ironrdp-server` as is for AVC420, input, DISP and CLIPRDR.
- AVC444 is a client and encoder task, not a server one; HOST-4 and EXT-5 plan for it.
