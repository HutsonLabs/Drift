# GNOME-WIZARD — Set up a headless GNOME machine from inside Drift

- Status: accepted
- Date: 2026-10-24
- Task: GNOME-WIZARD (WIZ-1 … WIZ-4)
- Builds on: `GNOME-only` (decision 7, `host/drift-gnome-setup`), `M0-4-host-setup`,
  `M9-3-sandbox-hardening-and-secrets`

## Context

`host/drift-gnome-setup` sets up a Linux machine from a Mac terminal with the system `ssh`.
Users of Drift for Mac, iPhone and iPad have no terminal step: the app is sandboxed (ADR M9-3,
`network.client` only, enforced by `src-tauri/tests/m9_3_hardening.rs`), so it can neither run
`/usr/bin/ssh` nor read `~/.ssh`, and iOS cannot start processes at all.

term.hut already connects to machines over SSH from a Tauri app. Its iOS path is sandbox-safe:
russh in-process, an ed25519 device key generated in the app, a known_hosts file in the app's
own directory, a strict host-key policy with an explicit "Trust and connect", a one-time
password used only to install the device key, and a "Copy public key" fallback.

Setting up 10.1.2.5 (Ubuntu 26.04.1, GDM 50.1, a server install) by hand on 2026-10-24 also
showed four gaps in `host/drift-host-setup.sh`:

- With the drop-in's `User=gdm`, `gdm-new-session` is refused by polkit ("Not authorized for
  action org.gnome.displaymanager.displayfactory.manage-user-displays", exit 70). As root it
  starts the session. The stock unit (DynamicUser, `Group=gdm`) fails with "The connection is
  closed".
- `--install` installs only `gnome-remote-desktop` and `openssl`; a server has no GNOME Shell,
  no GDM, and `gnome-headless-session@.service` has `Requires=gdm.service`.
- The full desktop metapackage pulls in about 860 packages including NetworkManager, which
  would compete with systemd-networkd for the only NIC of a machine reached over the network.
- Screen blanking (`org.gnome.desktop.session idle-delay`) stays on.

## Decisions

1. **SSH runs inside the app, the term.hut way.** A new crate `drift-ssh` wraps russh 0.64
   (`default-features = false`, `ring`, like the RDP TLS in M1-1; no `rsa`: the `rsa` crate's
   advisory, and ed25519/ECDSA host keys cover current servers). Nothing calls a system tool or
   reads `~/.ssh`, on macOS as on iOS. The M9-3 entitlements do not change.
2. **One device key.** `drift-ssh` generates an ed25519 key in-process on first use and keeps it
   in the app's data directory (`ssh/id_ed25519`, mode 0600, written to a temp file and renamed),
   unencrypted: the app sandbox is its lock, as in term.hut. Keychain or Secure Enclave storage
   is a later ADR.
3. **Host keys are strict.** known_hosts lives in the app's data directory. An unknown key stops
   the connection and the wizard shows its `SHA256:` fingerprint with "Trust and connect"; a
   changed key is always refused and can only be forgotten explicitly.
4. **Password once.** When the server does not know the device key and offers password
   authentication, the wizard asks for the account password once, connects with it, appends the
   public key to `~/.ssh/authorized_keys` idempotently, and forgets the password. "Copy public
   key" is always offered for servers without password logins.
5. **sudo.** The setup runs `sudo -n` when the account has passwordless sudo. Otherwise the
   wizard asks for the sudo password, which is written to `sudo -S -p ''` on stdin, never put on
   a command line, in a log or in a profile.
6. **The remote work stays `host/drift-host-setup.sh`.** The app embeds it (`include_str!`) and
   streams it to `sudo bash -s` on stdin; the credentials travel as environment assignments in
   the same stdin stream, never as arguments. The wizard runs `--check` first, then `--apply`.
   Its line format (`ok:`, `would:`, `change:`, `error:`, `skip:`, `wait:`, `fingerprint:`) is
   parsed by a pure `drift_core::setup` module that also turns the wizard's options into
   arguments and validates them.
7. **The wizard** (Connections → "Set Up a Linux Machine…") has five steps: *Machine* (host, SSH
   port, account), *Sign in* (trust, password once or copy key, sudo password if needed), *Options*,
   *Set up* (the `--check` list, then live `--apply` progress) and *Done*, which opens the New
   Connection form prefilled like a `drift-connect:` link, with the certificate pinned and the RDP
   password filled in; nothing is saved until the user clicks Save. Options and defaults:
   - session account: the SSH account;
   - RDP port: 3390; Remote Login (system daemon on 3389): off;
   - RDP user name: the account; RDP password: generated (24 characters), or typed;
   - install a minimal GNOME when missing: on;
   - open the port in the firewall: on, optionally only from one network (CIDR).

   Keeping the session awake (no suspend, no screen blanking, no lock) is not an option: nobody
   sits at a headless session, a locked one swallows input, and the RDP credentials guard it.
8. **Script fixes.** The drop-in becomes `DynamicUser=no`, `User=root`, `Group=root` (root passes
   the polkit check on Ubuntu and AnduinOS alike). New `--desktop` installs the smallest GNOME
   that runs a headless session — apt `gdm3 ubuntu-session gnome-remote-desktop ptyxis openssl
   fonts-cantarell` with `--no-install-recommends` (623 packages on a bare 26.04 image, no
   NetworkManager), dnf `gdm gnome-shell gnome-remote-desktop ptyxis openssl` without weak
   deps, pacman and zypper the same names — and makes sure `gdm.service` is enabled and
   `graphical.target` is the default. `--allow-from CIDR` limits the firewall rules. Keeping the
   session awake also sets `idle-delay 0`.

## Consequences

- Setting up a machine no longer needs a Mac terminal, and works from iPhone and iPad.
- `host/drift-gnome-setup` keeps working and gains `--desktop` and `--allow-from`.
- The device key is shared by every machine the wizard sets up; removing it from one machine is
  a line in that machine's `authorized_keys`.
- The Fedora 44 and Arch package sets were resolved in containers, not run on a real machine;
  the openSUSE set was not tried. The manual checks are in `docs/acceptance.md`.
