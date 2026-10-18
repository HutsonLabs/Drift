# HOST-2 — The Drift Host skeleton

- Status: accepted
- Date: 2026-10-18
- Task: HOST-2 (`TASKS.md`); builds on ADR `HOST-1-server-spike`

## Context

HOST-1 showed that the vendored `ironrdp-server` with EGFX streams AVC420 to Drift's client.
HOST-2 turns that into a crate and a binary that later tasks fill in: capture (HOST-3), the
real-time encoder (HOST-4), injection (HOST-6), pairing (HOST-8) and the menu-bar app (HOST-9).

## Decisions

1. **`drift-host`: a library and a `drift-host` binary.** The library holds the configuration,
   the identity, the session gate, the layout policy, the input sinks, the frame sources and the
   server loop. The binary reads the configuration and runs the host from a terminal until the
   menu-bar app exists.
2. **Loopback by default.** `host.toml` defaults to `listen = "127.0.0.1:3389"`. Until pairing
   (HOST-8) refuses unpaired devices, reaching the host from the network is an explicit edit, and
   the binary warns when the listener is wider than loopback (ADR `IOS-0` consequences). 3389 is
   the RDP port and macOS uses nothing on it; Screen Sharing uses 5900.
3. **Authentication for now: one NLA account.** `ironrdp-server` checks CredSSP against one
   preloaded account (HOST-1). The user name is in `host.toml`; the password is in the Keychain
   (`drift-apple`, service `com.hutsonlabs.drift.host`), set with `drift-host set-password`. The
   host never stores it in a file. HOST-8 replaces this with paired device keys.
4. **A self-signed certificate created once.** `host-cert.der` and `host-key.der` (mode 0600) in
   `~/Library/Application Support/com.hutsonlabs.drift.host`. The binary prints the SHA-256
   fingerprint Drift pins. The certificate never rotates by itself, because paired devices pin it.
   It is generated with `rcgen`, already a workspace dependency (until now only for tests).
5. **One session at a time, refused politely.** The host owns its accept loop instead of
   `RdpServer::run`, so it knows the bound port and can close a second connection at once rather
   than leave it hanging in the backlog. The running session is never interrupted
   (`SessionGate`).
6. **`RdpServer` on its own thread.** It is not `Send` (HOST-1), so the host runs it on a thread
   with a current-thread runtime. `Host::start` is synchronous and returns the bound address.
7. **Pictures behind a `FrameSource`.** A source turns a desktop size into one AVC420 keyframe.
   The skeleton's `TestPattern` encodes colour bars with the existing VideoToolbox encoder
   (`drift-video`'s `recording` feature); HOST-3 and HOST-4 replace it with the screen. When the
   graphics channel is ready the host creates one surface the size of the desktop and sends a
   keyframe.
8. **Display Control resizes the desktop.** A client layout is clamped to 200–8192 pixels per
   side and rounded down to even numbers (H.264 4:2:0), then `resize_with_monitors` resets the
   client's output with one primary monitor and a fresh surface and keyframe follow.
9. **Input is counted, not injected.** Input reaches an `InputSink`. The skeleton's
   `RecordingInput` counts events and logs only their kind: a client typing a password must not
   leave it in the host's log. `drift-inject` (HOST-6) becomes another sink behind
   `inject_input`.
10. **`Drift::Mac::v1` on every connection.** A small vendored patch
    (`0008-feat-ironrdp-server-per-connection-dvc-factories.patch`) adds
    `RdpServer::add_dvc_factory`, so an embedder attaches its own dynamic channels to each
    connection; the host offers `DriftMacServer` (EXT-1).

## Consequences

- `crates/drift-host/tests/loopback.rs` connects `drift-rdp` to the host: the pattern decodes,
  `Drift::Mac::v1` negotiates, input arrives, a second client is refused, and a 1001×701 layout
  becomes a 1000×700 desktop with a new picture.
- The host does not yet capture the screen, inject input, pair devices, listen on QUIC (HOST-12)
  or run as an app; each has its task.
