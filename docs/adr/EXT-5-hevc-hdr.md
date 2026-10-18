# EXT-5 — HEVC and HDR between Drift ends

- Status: accepted (HEVC); HDR partly done
- Date: 2026-10-18
- Task: EXT-5 (`TASKS.md`); follows ADR `IOS-0` decision 2, ADR `HOST-4-realtime-encoder`, ADR
  `AVC444-client`, ADR `EXT-1-channel-framing`

## Context

HOST-4 measured AVC444 at 60 fps only up to about 1920×1080 on an M4 Pro (two H.264 streams at
4K run about 25 fps). Screen Sharing streams HEVC with full chroma and HDR. RDP GFX has no HEVC,
so ADR `IOS-0` planned it as a Drift-only codec negotiated on `Drift::Mac::v1`, with AVC444/AVC420
for every other client.

## What this Mac's encoder can do

`VTCopySupportedPropertyDictionaryForEncoder` on an M4 Pro (macOS 27) lists HEVC Main, Main 10,
Main 4:2:2 10, Main 4:4:4, Main 4:4:4 10, Monochrome and Monochrome 10, all on the hardware encoder
(`com.apple.videotoolbox.videoencoder.ave.hevc`). The iOS 27 simulator's software encoder has
Main and Main 10 only. `drift_video::hevc::probe_encoder` reads the list at launch.

VideoToolbox encodes what it is given: a Main 4:4:4 session fed BGRA or NV12 frames writes plain
Main 4:2:0 (`chroma_format_idc` 1). The encoder therefore converts frames to the profile's source
format first (`'444f'`, `'xf44'`, `'xf20'`) with a `VTPixelTransferSession`, VideoToolbox's own
conversion path, not a CPU loop.

Measured with `cargo run --release -p drift-video --example hevc_bench -- <w> <h> 240` on an M4
Pro while other builds kept the load average around 300 (so CPU-side numbers are pessimistic):

| Profile | 1920×1080 | 2560×1600 | 3840×2160 |
|---|---|---|---|
| Main (from NV12) | 60 fps, p50 6.3 ms | 60 fps, p50 9.5 ms | 60 fps, p50 15.0 ms, 18 Mbit/s |
| Main 10 | 60 fps, p50 8.1 ms | 60 fps, p50 13.0 ms | 60 fps, p50 21.1 ms |
| Main 4:4:4 (from BGRA) | 60 fps, p50 9.4 ms | 60 fps, p50 15.9 ms | 60 fps, p50 24.3 ms, 36 Mbit/s |
| Main 4:4:4 10 | 60 fps, p50 9.9 ms | 60 fps, p50 16.8 ms | 60 fps, p50 26.5 ms |

Low-latency rate control is used up to 2560×1600 only, as for H.264: at 4K it made each encode
call wait for its picture and capped Main at 55 fps and 4:4:4 at 42 fps (p50 118 ms).

Quality (640×368 coloured one-pixel strokes, the AVC444 test's content): HEVC 4:4:4 keeps chroma
at 50.3 dB where HEVC Main gives 17.9 dB; luma (the decoded Y plane) is 42 dB for Main on the Mac
and 34 dB from the simulator's software encoder.

## Decisions

1. **Negotiation (`drift_ext::codec`, types `0x05xx`).** After the Hellos agree on
   `Capabilities::CODEC`, the client sends `Offer { HEVC profiles it decodes, transfer functions
   its display shows }`; the host answers `Select { profile or none, transfer }` from
   `select(offer, host, size)`: full chroma when both have it and the picture fits, 10-bit and HDR
   (PQ before HLG) only when both ends show HDR, otherwise SDR; none in common means H.264.
   The host recomputes the choice when it (re)opens the picture, so the `Select` is a statement of
   intent; the client decodes whatever HEVC arrives (the bitstream names its profile).
2. **Only paired Drift clients.** The client offers `CODEC` only to a paired Drift Host (with
   HOST-8's pairing, so the same connections that get V10 caps and AVC444), and the host offers it
   only with pairing on. Every other client never sees HEVC.
3. **Transport: GFX with a private codec id.** HEVC frames are `WireToSurface1` PDUs with
   `Codec1Type::DriftHevc` (0xD001, outside MS-RDPEGFX's 0x0000–0x000F), carrying the same
   `RFX_AVC420_METABLOCK` as AVC420 followed by one HEVC Annex-B access unit. Surfaces, frame
   acks, backpressure, the QUIC transport and the client's damage regions all work unchanged.
   The alternatives were worse: frames on `Drift::Mac::v1` would duplicate surfaces and acks; a
   GFX capability flag would need a spec change. A non-Drift client could not parse the PDU, which
   is why the host sends it only after the negotiation in decision 1. The vendored change is
   `third_party/ironrdp-patches/0009-feat-ironrdp-egfx-drift-hevc-codec.patch`.
4. **Responders share one chain.** Pairing (HOST-8), codec negotiation, gestures (EXT-2), status
   (EXT-4) and menus (EXT-3) share one channel end through `drift_ext::channel::Chain`, which
   each end holds and `with_responder` extends; each message goes to the first responder that
   answers it. (This branch first added `drift_ext::compose::Responders`; the wave-3 integration
   folded it into `Chain`.)
5. **Host.** `drift_host::codec::host_codecs` probes the encoder at launch (SDR only, see
   decision 7). `LiveRequest::hevc` asks the capture factory for `LiveHevcEncoder`: 4:2:0 from
   NV12 capture, 4:4:4 from BGRA capture. HEVC is preferred over AVC444 when negotiated. An offer
   that arrives after the picture started restarts it with the new codec. `HostStats` reports
   `hevc_frames` and `hevc_profile`. Limits: 4:2:0 and 4:4:4 up to 3840×2160 (the table above).
6. **Client.** `drift_video::hevc::VtHevcDecoder` (`drift_core::video::HevcDecoder`) builds its
   session from VPS/SPS/PPS and picks its output from the SPS: 4:2:0 as IOSurface NV12 (the
   compositor's zero-copy path), 4:4:4 as BGRA (blitted like AVC444's). The Mac offers Main and
   Main 4:4:4; iPhone and iPad offer Main until a device run shows 4:4:4 decoding there.
7. **HDR: the encoder side only, for now.** Main 10 / 4:4:4 10 sessions are tagged BT.2020 with
   PQ or HLG, with HDR metadata insertion and SMPTE ST 2086 / content light level values (P3-D65,
   1000 cd/m²); the decoder reads the transfer function from the stream, and a test shows PQ
   reaching the decoded picture. Not done: capturing the screen in HDR (ScreenCaptureKit's HDR
   dynamic range and 10-bit formats need Screen Recording permission to verify) and showing HDR
   on the client (an EDR `CAMetalLayer` with a 16-bit float or 10-bit drawable and a PQ colour
   space). Until both land, the host offers SDR and the client offers SDR, so no session is HDR.
8. **Shared types.** `HevcProfile`, `Transfer`, `VideoPicture`, `DecodedVideo` and the
   `HevcDecoder` seam live in `drift_core::video`, beside `H264Decoder` and `Avc444Decoder` (ADR
   `M0-5-nv12-frame-location`); `drift-ext` now depends on `drift-core` for them.

## Consequences

- A paired Drift client gets full-chroma HEVC at up to 4K60 on an M4 Pro, where AVC444 stops at
  about 1080p60.
- GNOME, Windows App, `mstsc` and unpaired clients are unchanged (V8.1 or V10 caps, H.264).
- HDR needs two more pieces (capture and EDR rendering) and an HDR display to check; the steps are
  in `docs/acceptance.md`.
- M1 and a quiet-machine rerun of the benchmark are manual steps.
