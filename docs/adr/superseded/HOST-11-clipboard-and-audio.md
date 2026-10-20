# HOST-11 — Clipboard and sound between Drift Host and Drift

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Task: HOST-11 (`TASKS.md`); builds on ADRs `M5-1-clipboard-format-mapping`, `HOST-2-host-skeleton`

## Context

Drift Host (HOST-2) showed a picture and logged input. A Mac session also needs the clipboard
in both directions and the Mac's sound. `ironrdp-server` already has CLIPRDR and rdpsnd
server roles (HOST-1); `drift-clipboard` has the client's sync state, which does not depend on
which end runs it. Drift's client had no sound at all.

## Decisions

1. **The host runs the client's clipboard state.** `drift_host::clipboard` feeds IronRDP's
   CLIPRDR server callbacks into the same `ClipboardSync` the client uses: newest wins, our own
   pasteboard writes are not echoed, text and images (PNG/DIB/TIFF mapping from M5-1), the
   32 MiB cap. The host is always "focused": there is one session and it is the user's.
2. **One clipboard thread.** It owns the pasteboard (made by a factory on that thread, because
   `NSPasteboard` is not `Send`), polls `changeCount` every 250 ms like the client (M5-3), runs
   the sync state, and sends PDUs through `ServerEvent::Clipboard`. A new connection resets the
   state (CLIPRDR restarts per connection).
3. **No advertisement at connect.** When the client's first Format List arrives, the host
   answers with what it has seen change since it started, which is usually nothing; so on
   connect the device's clipboard goes to the Mac and the Mac's does not overwrite the
   device's. After that, every copy on either side crosses.
4. **Sound is 16-bit stereo PCM.** The host offers 48 and 44.1 kHz, prefers the source's rate,
   and sends whatever the client accepts. On a local network ~1.5 Mbit/s is small next to the
   picture, and neither end needs a codec (no Opus/AAC dependency). The formats live in
   `drift_core::audio`.
5. **The host's source is ScreenCaptureKit.** `drift_capture::audio::AudioCapture` is an
   audio-only `SCStream` (48 kHz, stereo, Drift Host's own sounds excluded, a 2×2 picture it
   ignores). Its float samples, interleaved or planar, become 16-bit interleaved PCM in a pure,
   tested function. It needs Screen Recording, like the picture; without it the host declines
   the format and the session has no sound. Tests use a synthetic `ToneSource`.
6. **The client plays through `AVAudioEngine`.** `drift-rdp` offers rdpsnd only when the app
   passes an `AudioOutput` (`SessionOptions::audio`); `drift_apple::audio_out::Player` converts
   to float and schedules buffers on an `AVAudioPlayerNode`, on macOS and iOS. On iOS the audio
   session is `Playback` with `MixWithOthers`. Volume and pitch PDUs are ignored for now.
7. **A 200 ms latency cap.** Sound that would sit behind more than 200 ms of queued audio is
   dropped (`PlayoutBudget`), so after a stall the sound catches up with the picture instead of
   lagging behind it. An empty queue always takes the next chunk.
8. **Every session plays sound, GNOME included.** g-r-d also offers rdpsnd; Drift used to
   leave the channel out. There is no per-profile switch yet.

## Consequences

- Loopback tests cover both clipboard directions (text and an image) and the tone reaching the
  client's sink intact. The real pasteboard and real system audio are manual steps
  (`docs/acceptance.md`): the tests never touch the general pasteboard, and this Mac has no
  Screen Recording grant.
- A client copy within 500 ms of the host advertising the same kinds of data is taken as an
  echo (M5-1's rule); the test waits that window out.
- The iOS app links `AVFAudio.framework`.
