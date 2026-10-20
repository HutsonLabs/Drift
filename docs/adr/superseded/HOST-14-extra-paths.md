# HOST-14 — Extra paths: peer-to-peer Wi-Fi, VPN, USB

> Superseded by ADR GNOME-only (2026-10-20): Drift Host is retired; this is kept for history.

- Status: accepted
- Date: 2026-10-18
- Task: HOST-14 (`TASKS.md`); idea from Sill (ADR `IOS-0`, "Ideas taken from Sill")

## Decisions

1. **Peer-to-peer Wi-Fi (AWDL).** Drift Host's Bonjour advert is registered with
   `kDNSServiceFlagsIncludeP2P` and Drift's browser sets `nw_parameters_set_include_peer_to_peer`,
   so an iPad finds a Mac with no shared network. BSD listeners skip AWDL unless
   `SO_RECV_ANYIF` is set, so the host (and the HOST-13 daemon) set it on every listener that
   is not on loopback. Pairing and pins apply unchanged. QUIC's UDP socket is not changed yet.
2. **VPN, no NAT traversal.** Remote access is the user's own VPN (WireGuard, Tailscale, the
   Mac's own); the host listens on all interfaces once pairing is on (HOST-8), so `utun`
   addresses work as they are. Drift does no hole punching and runs no relay.
3. **Path classification** (`drift_apple::paths`, pure + `getifaddrs`): loopback is `Local`;
   a link-local address follows its scope (`awdl*`/`llw*` → `PeerToPeer`); otherwise the
   longest-prefix interface decides (`utun*`, `ipsec*`, `ppp*`, `tun*`, `tap*` → `Vpn`, others
   `Lan`); anything else is `Remote`. `is_away()` (`Vpn` or `Remote`) is what EXT-4 uses for its
   separate default quality.
4. **No USB cable link.** The only way to tunnel TCP over the cable is `usbmuxd`
   (`/var/run/usbmuxd`), a private protocol with no API on the Mac and none on iOS; the device
   side cannot open USB connections at all, and an iPad on USB-C gets no IP link to the Mac by
   default. Peer-to-peer Wi-Fi covers "no network" without a cable. If Apple ships a public
   path (for example `Network.framework` over the USB-C Ethernet a Mac sees), it plugs into the
   same listener.

## Consequences

- HOST-14 is `[~]`: AWDL and VPN work by construction and are unit- and smoke-tested (a real
  P2P Bonjour round trip on the dev Mac); the over-the-air checks are manual
  (`docs/acceptance.md`); USB is not built.
