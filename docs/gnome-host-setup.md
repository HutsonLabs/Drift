# GNOME host setup

How to set up a Linux machine with GNOME for Drift from a Mac (ADR `GNOME-only`), how to
prepare a GNOME Remote Desktop (g-r-d) 50 host for Drift, and how the homelab test
host (`homelab@10.0.2.40`, plan §5.2) is kept in that state. The homelab sections were verified
on AnduinOS 2.0.3 (Ubuntu 26.04 base, GNOME Shell 50.1, gdm3 50.1, gnome-remote-desktop 50.2).
`host/drift-gnome-setup` was first run on 2026-10-24 against 10.0.2.5 (Ubuntu 26.04.1 server,
GDM 50.1), which is where the root drop-in, `--desktop` and the PipeWire step come from (ADR
`GNOME-WIZARD-headless-setup`); its manual checks are in `docs/acceptance.md` (GNOME-only).

Drift itself can do the same from inside the app, on Mac, iPhone and iPad: Connections →
"Set Up a Linux Machine" signs in over SSH with Drift's own key (kept in the app; the host key
is trusted only when you click "Trust and Continue"), adds that key with the account password
once, runs `host/drift-host-setup.sh` (`--check`, then `--apply`) and opens the prefilled New
Connection form with the RDP password filled in (ADR `GNOME-WIZARD-headless-setup`).

| File | Purpose |
|---|---|
| `host/drift-gnome-setup` | Runs on the Mac: sets up one machine over SSH and opens the prefilled connection in Drift. |
| `host/drift-host-setup.sh` | Idempotent setup script (`--check` by default, `--apply` to change). |
| `host/gnome-headless-session-dropin.conf` | The systemd drop-in that makes `gnome-headless-session@.service` start (runs its launcher as root) and restarts it whenever it ends. |
| `host/drift-host-report.sh` | Read-only state report used by `cargo xtask host-setup-check`. |

## Setting up a machine from your Mac

```bash
host/drift-gnome-setup user@machine
```

One command gives the account a GNOME desktop that Drift can reach without anyone logged in at
the machine. Over SSH it copies `host/drift-host-setup.sh` to the machine and runs it with sudo
(`--apply --install --firewall --headless USER:PORT:persistent`, below), which:

- installs `gnome-remote-desktop` and `openssl` when missing (apt, dnf, zypper or pacman);
- gives the account a persistent headless GNOME session on a fixed RDP port (3390 by default)
  with a TLS certificate and RDP credentials;
- turns off idle suspend and the screen lock for that session;
- opens the port in ufw or firewalld when one of them is active.

Then, on the Mac, it copies a new 24-character RDP password to the clipboard and opens a
`drift-connect:` link, so Drift shows a New Connection form filled in with the machine, port,
mode, user and the certificate's fingerprint already pinned. Paste the password, check the
details and click Save; nothing is saved before that. With `qrencode` installed
(`brew install qrencode`) it also prints the link as a QR code: scan it with the iPhone or iPad
Camera app to open the same form in Drift there.

The password never goes into the link or the script's output. It reaches the machine over SSH
in a file only you and root can read, which is deleted when the setup ends.

Requirements:

- SSH access to the machine from the Mac, and sudo there (`ssh -t`, so sudo can ask for your
  password).
- GNOME 46 or later: older `gnome-remote-desktop` has no headless mode, and the setup stops
  with an error.
- An account nobody logs in to at the machine's screen. GNOME runs one session per account, so
  a headless session and a local login for the same account get in each other's way; create a
  separate account (for example `remote`) and pass it with `--user` if you use the machine
  yourself.

| Option | Effect |
|---|---|
| `--user NAME` | The Linux account that gets the headless session (default: the SSH user). |
| `--port N` | The headless session's RDP port (default 3390). |
| `--name NAME` | The connection's name in Drift (default: the machine's short name). |
| `--rdp-user NAME` | The RDP user name (default: the account name). |
| `--keep-password` | Keep the RDP credentials already set; Drift asks for the password when connecting. |
| `--remote-login` | Also set up Remote Login: GNOME's login screen on port 3389, for any account, with the same RDP user and password. Adds a second link (`<name> login`) with its own pin. |
| `--allow-from NET` | Open the firewall only to one network (an address with a prefix, e.g. `10.0.0.0/21`). |
| `--check` | Show what would change on the machine; change nothing, set no password, open nothing. |
| `--no-open` | Print the link instead of opening Drift. |
| `--print-link` | Only print the `drift-connect:` link for the options and machine (no SSH; pass the fingerprint with `--pin`). |

The setup is idempotent: run again with `--keep-password`, it reports `0 change(s) applied`;
without it, the only change is the new password. If the machine reports no fingerprint, Drift
asks you to trust the certificate on the first connect instead.

## What Drift needs on the host

| Mode | Server side | Set up by |
|---|---|---|
| Remote Login | System daemon (`gnome-remote-desktop.service`, `grdctl --system`) on :3389 with a TLS certificate and RDP credentials | `--system` |
| Headless | Per-user headless daemon (`grdctl --headless`) on a **fixed** port, TLS certificate, RDP credentials; optionally a persistent `gnome-headless-session@USER.service` | `--headless USER:PORT[:persistent]` |
| Desktop Sharing | The user's own daemon, configured in GNOME Settings → System → Remote Desktop → Desktop Sharing | By hand (credentials live in the login keyring and can't be set over SSH, plan §1.9) |

## The setup script

`host/drift-gnome-setup` runs it for you. To run it by hand, run it as root on the host. From
the dev Mac:

```bash
# 1. See what would change (changes nothing; exit 0 = already converged, 2 = changes pending).
ssh homelab@10.0.2.40 'sudo -n bash -s -- --check --system \
    --headless drifttest:3391 --headless drifttest2:3392:persistent' < host/drift-host-setup.sh

# 2. Apply.
ssh homelab@10.0.2.40 'sudo -n bash -s -- --apply --system \
    --headless drifttest:3391 --headless drifttest2:3392:persistent' < host/drift-host-setup.sh

# 3. Verify from the Mac (read-only, over SSH).
cargo xtask host-setup-check
```

Each step reads the current state first and acts only when it differs, so re-running
`--apply` reports `0 change(s) applied`. The steps:

**`--desktop`** (first; `drift-gnome-setup` always passes it): when GNOME Shell or GDM is
missing, the smallest GNOME that runs a headless session is installed, without recommends —
apt `gdm3 ubuntu-session gnome-remote-desktop ptyxis openssl fonts-cantarell` (`gnome-session`
instead of `ubuntu-session` on Debian; 623 packages on a bare Ubuntu 26.04 image), dnf `gdm
gnome-shell gnome-remote-desktop ptyxis openssl` without weak dependencies, zypper and pacman
the same names. The desktop metapackages are avoided on purpose: they pull in NetworkManager,
which would compete with systemd-networkd for the NIC of a machine reached only over the
network. Then `gdm.service` is enabled and started (`gnome-headless-session@.service` has
`Requires=gdm.service`) and `graphical.target` is made the default.

**`--install`**: `gnome-remote-desktop` and `openssl` are installed
when `grdctl` or `openssl` is missing, with apt, dnf, zypper or pacman. With none of those it is
an error that names the packages to install by hand.

Whenever `--headless` is given, the script also checks that `grdctl` has `--headless` (GNOME 46
or later) and stops with an error otherwise.

**`--system` (Remote Login)**
1. TLS certificate and key: kept if `grdctl --system status` shows them; otherwise a
   self-signed RSA-4096 certificate is generated in
   `/var/lib/gnome-remote-desktop/.local/share/gnome-remote-desktop/certificates/` and set with
   `grdctl --system rdp set-tls-key/set-tls-cert`.
2. Credentials: kept if set. Written only when `DRIFT_SYSTEM_RDP_USER` and
   `DRIFT_SYSTEM_RDP_PASS` are exported *and* differ from the current ones. Empty credentials
   without those variables are an error.
3. `grdctl --system rdp enable`, then `systemctl enable --now gnome-remote-desktop.service`.

**`--no-system`** (a headless-only machine): Remote Login off. `grdctl --system rdp disable`
when it is enabled, and `systemctl disable --now gnome-remote-desktop.service` when the unit is
enabled or running. Refused together with `--system`. The Remote Login greeter and a persistent
headless session for the same account compete for that account's one GNOME session.

**`--headless USER:PORT[:persistent]`**

Before the users: when `polkit-agent-helper.socket` exists and is *failed* (it hits its trigger
limit and stays down), it is reset and restarted. With it down, gnome-shell's polkit agent logs
"polkit-agent-helper-1: needs to be setuid root" in a tight loop (over 6 million lines on
10.0.2.5) and no authentication dialog works.

1. With `persistent`: GNOME Initial Setup is marked done for USER
   (`~USER/.config/gnome-initial-setup-done` = `yes`); otherwise the session opens
   `gnome-initial-setup --existing-user` over the desktop and nobody is there to finish it. Then
   the drop-in is installed as
   `/etc/systemd/system/gnome-headless-session@.service.d/drift.conf` unless the file already
   has exactly the committed content and `systemctl show` reports `DynamicUser=no`, `User=root`
   and `Restart=always`, then `systemctl enable --now gnome-headless-session@USER.service`.
   The drop-in:

   ```ini
   [Unit]
   StartLimitIntervalSec=0

   [Service]
   DynamicUser=no
   User=root
   Group=root
   Restart=always
   RestartSec=5
   ```

   Without `User=root` the shipped unit fails with "Failed: The connection is closed" (exit 70,
   plan §1.9). The earlier `User=gdm` drop-in works on AnduinOS but not on Ubuntu 26.04, where
   polkit refuses it ("Not authorized for action
   org.gnome.displaymanager.displayfactory.manage-user-displays"); root passes on both.
   `Restart=always` is what makes the session *persistent*: `gdm-new-session` exits 0 whenever
   the session ends (log out from the desktop, a gnome-shell crash, or a `systemctl restart` that
   races the old session's teardown and ends at once with "Session finished"), so the shipped
   `Restart=on-failure` left the unit dead, and the per-user `gnome-remote-desktop-headless`
   daemon, which belongs to `gnome-session.target`, stopped with it. Nothing listened on the
   port until the next reboot (10.0.2.5, 2026-10-24). With the drop-in the session and its RDP
   daemon are back about 7 s after the session ends, and `StartLimitIntervalSec=0` keeps systemd
   retrying.
2. The remaining steps run as USER on the user's session bus (`/run/user/UID/bus`); a
   non-persistent user must already have a session.
3. TLS certificate and key, as for the system daemon, under
   `~USER/.local/share/gnome-remote-desktop/certificates/`.
4. Credentials from `DRIFT_HEADLESS_RDP_USER_<USER>` / `DRIFT_HEADLESS_RDP_PASS_<USER>`
   (upper-cased, non-alphanumerics as `_`, e.g. `..._DRIFTTEST2`); same rules as above. They
   are stored in a GKeyFile on hosts without a TPM, which works over SSH.
5. `grdctl --headless rdp set-port PORT` and `disable-port-negotiation`, so the daemon never
   moves to another port (with negotiation on, the port depends on start order). g-r-d reads the
   port when the RDP server starts, so this takes effect at the next daemon start and does not
   drop live connections.
6. `grdctl --headless rdp enable`; `systemctl --user enable --now pipewire.socket`, then
   `gnome-remote-desktop-headless.service`. g-r-d streams the screen through PipeWire; a user
   manager that was already running when GNOME was installed never starts the socket, and every
   login then ends with "Couldn't connect pipewire context".
7. Automatic suspend off for the user
   (`org.gnome.settings-daemon.plugins.power sleep-inactive-ac-type 'nothing'`): an idle
   headless session otherwise shows "Suspending soon because of inactivity".
8. Screen blanking off (`org.gnome.desktop.session idle-delay 0`).
9. Screen lock off for the user (`org.gnome.desktop.screensaver lock-enabled false`): nobody
   sits at a headless session, a locked one swallows input (see the E2E section below), and
   the RDP credentials already guard it.

**`--firewall`**: after each daemon is set up, its port (3389 for `--system`, PORT for each
`--headless`) is allowed as TCP in ufw or firewalld, whichever is active (`ufw allow PORT/tcp`;
`firewall-cmd --add-port` for the running and the permanent configuration). With
`--allow-from NET` the rule only admits that network (`ufw allow from NET to any port PORT
proto tcp`; a firewalld rich rule). With neither firewall active it does nothing.

**Fingerprints.** For every daemon whose certificate is known, the script prints a
machine-readable line

```
fingerprint: <system|USER> <sha256 as aa:bb:…, lowercase>
```

which `host/drift-gnome-setup` reads to pin the certificate in the `drift-connect:` link.

Headless daemons are always read-write (`grd_settings_headless_new` forces `rdp-view-only`
to false), so the plan's `disable-view-only` step is not needed.

Credentials are never printed: plain `grdctl status` shows only `(hidden)`/`(empty)`, and
credential changes are reported without their command line.

## `cargo xtask host-setup-check`

Pipes `host/drift-host-report.sh` to `ssh homelab@10.0.2.40 bash -s` (override with
`--host` or `DRIFT_E2E_SSH`, and `--headless USER:PORT[:persistent]` for other users), parses
the report and checks:

- the system daemon: unit enabled and active, RDP enabled, port 3389, TLS certificate
  valid (not expired), credentials set, and a `--system` g-r-d process listening on :3389;
- each headless user: daemon active, RDP enabled, port pinned (configured port = expected
  and negotiation off), TLS certificate valid, credentials set, a `--headless` g-r-d
  process owned by that user listening on the port, and the user unit enabled and active;
- for persistent users: `gnome-headless-session@USER.service` enabled and active, with
  `DynamicUser=no` and `User=root` (or the older `User=gdm`, which works on AnduinOS), and
  `Restart=always`.

With `--no-system` (a machine set up with `drift-host-setup.sh --no-system`) the system daemon
checks become: `gnome-remote-desktop.service` disabled and stopped, and system RDP disabled.

## 10.0.2.5: a persistent headless session only (2026-10-26)

The dev runner (Ubuntu 26.04.1, GDM 50.1, g-r-d 50.2) has one persistent Wayland headless session
for `alice` on :3390 and no Remote Login. It is converged and verified with:

```bash
ARGS='--no-system --headless alice:3390:persistent --firewall --allow-from 10.0.0.0/21'
ssh alice@10.0.2.5 "sudo -n bash -s -- --check $ARGS" < host/drift-host-setup.sh  # 0 pending
cargo xtask host-setup-check --host alice@10.0.2.5 --headless alice:3390:persistent --no-system
```

The RDP credentials are set with `DRIFT_HEADLESS_RDP_USER_ALICE` /
`DRIFT_HEADLESS_RDP_PASS_ALICE` on the `--apply` run. Port 3390 is additionally allowed on
`tailscale0` (`ufw allow in on tailscale0 to any port 3390 proto tcp`), which the script does not
manage. Ending the session (`loginctl terminate-session`) brings a new Wayland session and the
:3390 listener back in about 7 s.

The system daemon stops listening for a moment while it hands a connection over, so a
failing live check is retried up to three times. `--report FILE` evaluates a saved report
and `--save-report FILE` keeps the raw report. The parsers are unit-tested on reports
captured from the homelab (`xtask/tests/data/host/`).

## Homelab state (2026-09-22)

`--check` before the first `--apply` reported 7 pending changes: pin :3391 and :3392 and
disable negotiation for both users, enable `gnome-headless-session@drifttest2.service` at
boot (it was running but not enabled), and disable automatic suspend for both users. After
one `--apply`, `--check` and a second `--apply` both report 0 changes, and
`cargo xtask host-setup-check` passes all 24 checks.

Things the script deliberately does not do:
- set or change credentials unless they are given in the environment;
- touch Desktop Sharing (:3390, the `homelab` user's daemon);
- clean up greeter sessions that clients left behind by disconnecting at the GDM greeter
  (plan §1.9; Drift itself closes greeter tabs gracefully, M3-4).

## E2E helpers and session state (stream A, M4-2/M5-2/M6-3/M7-3)

`cargo xtask e2e` needs a little more than the daemons:

- **The test session must be unlocked.** A locked GNOME session swallows input events and stops
  propagating clipboard changes, which looks exactly like a broken client. The tests call
  `drift_e2e::host::ensure_unlocked_session`, which turns the idle lock off
  (`org.gnome.desktop.screensaver lock-enabled false`, `idle-activation-enabled false`,
  `org.gnome.desktop.session idle-delay 0`) and restarts
  `gnome-headless-session@<user>.service` when the session is already locked — GNOME cannot be
  unlocked over D-Bus without the user's password.
- **Helpers live in the repository.** `host/cliptool.py` (clipboard read/write) and
  `host/scrolltool.py` (scroll, key and motion counters) are uploaded into the test user's home
  by the tests themselves; `anim.py` and `clip_in.png` are expected in that home directory
  (`/home/<user>/`), as set up during the spike.
- **The tests run one at a time** (`--test-threads 1`): they share one host and one daemon per
  mode.
