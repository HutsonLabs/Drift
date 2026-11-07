# HOST-13 — Daemon, agents and the login window

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Task: HOST-13 (`TASKS.md`); follows ADR `IOS-0` decision 10

## Context

The single `drift-host` process (HOST-2) runs in the logged-in user's session. After a restart,
nobody is logged in, so a paired device cannot reach the Mac. IOS-0 decision 10 asks for a
LaunchDaemon, a per-session LaunchAgent and a login-window agent.

## Decisions

1. **The daemon only brokers.** `drift-host daemon` (root, `LaunchDaemons/`) owns the listening
   port, the shared store and the Bonjour advert. It accepts a TCP connection, asks
   `broker::route` who is at the console, and hands the socket to that session's agent over a
   Unix socket (`SCM_RIGHTS`). It never reads a client's bytes: TLS, NLA, pairing checks, RDP
   and everything after run in the agent. The code that runs as root is the accept loop, the
   routing, the agents' IPC frames (`ipc`, strict and capped) and the device-list writes.
2. **Agents serve sessions.** `drift-host agent aqua` runs in each logged-in user's session
   (`LimitLoadToSessionType = Aqua`) with that user's privileges: capture, encode, inject.
   `drift-host agent loginwindow` runs at the login window (`LoginWindow`; launchd runs it as
   root there). Each agent runs the same `Host` as the single process, fed by
   `server::Incoming::Handed` instead of its own listener.
3. **Routing** (`broker`, pure): the console user's newest Aqua agent; the newest login-window
   agent when the console is `loginwindow` or empty; `Wait` up to 10 s when that agent is not
   connected yet (it starts after a login or a user switch); refuse when the console cannot be
   read, rather than guess whose screen to show.
4. **Who may be an agent** (`AgentPolicy`): the daemon checks the socket's peer with
   `getpeereid`. An Aqua agent must run as the uid it claims and never as root; the
   login-window agent must be root. The socket is mode 0666 because every user's agent
   connects; identity comes from the kernel, not the file mode.
5. **Unix socket, not XPC.** Passing an accepted socket is one `sendmsg` with `SCM_RIGHTS`,
   testable in-process and on loopback. XPC would need a Mach service per agent and an
   Objective-C bridge for the descriptor; it adds nothing the peer-uid check does not give.
   The socket is `/var/run/com.example.drift.host.sock`.
6. **One shared store.** Identity, `host.toml` and `devices.toml` live in
   `/Library/Application Support/com.example.drift.host`, so every agent presents the same
   certificate that devices pinned. The daemon creates them and makes the directory 0750 and
   the private key 0640, group `admin`; an agent for a user outside `admin` cannot load the key
   and cannot serve (a limit, below). Agents read `devices.toml` (public keys only) and send
   new pairings to the daemon (`SaveDevices`), which validates the list and writes it
   atomically; only the daemon writes the store.
7. **Installation.** The app registers the daemon and the user agent with `SMAppService`
   (`daemonServiceWithPlistName:` / `agentServiceWithPlistName:`); macOS then asks the user to
   approve the daemon in System Settings › General › Login Items. `SMAppService` agents load
   per user and never at the login window, so the login-window agent must be copied to
   `/Library/LaunchAgents` by an administrator (a `.pkg` postinstall later, STORE-4). The
   plists live in `crates/drift-host-app/launchd` and `cargo xtask host-bundle` puts them in
   `Contents/Library/LaunchDaemons|LaunchAgents`.
8. **QUIC stays single-process.** Handed connections are TCP: a QUIC endpoint's UDP socket
   serves every connection and cannot be handed over per connection. In daemon mode devices
   use TCP; QUIC needs the single process (or a later UDP relay in the daemon).

## The login window: what can be captured

Every CoreGraphics capture API (`CGDisplayCreateImage`, `CGWindowListCreateImage`,
`CGDisplayStream`) is obsoleted in macOS 15 (`SCREEN_CAPTURE_OBSOLETE(…, 15.0)` in the 27 SDK).
ScreenCaptureKit is the only public API left, and it needs a Screen Recording grant for the
process asking. Whether ScreenCaptureKit returns frames to a root agent in the `LoginWindow`
session is not documented; Screen Sharing reaches the login window through private entitlements
third parties cannot get. The login-window agent therefore runs the same capture path as the
user agent and, when capture is refused, shows the HOST-2 test pattern; a paired device then
sees that the Mac is up but cannot type a password. This must be checked on a restarted Mac
(`docs/acceptance.md`).

## Threat model notes

- Root code paths: accept, route, read ≤ 256 KiB IPC frames from peer-checked local agents,
  validate and write `devices.toml`. No TLS or RDP parsing as root.
- The login-window agent parses RDP as root (launchd's choice for that session). Pairing still
  gates everything (HOST-8): an unpaired device is dropped before any picture or input.
- A local admin can read the host key (group `admin`); a local non-admin cannot.

## Consequences

- HOST-13 is `[~]`: brokering, agents, routing, IPC, the shared store and the launchd layout are
  built and tested on loopback (daemon + agent in-process, `drift-rdp` connecting through the
  daemon's port). Installing the services, a real restart and the login-window picture are
  manual steps; pairing a new device in daemon mode (the code window lives in the menu-bar app's
  single process) is a follow-up.
