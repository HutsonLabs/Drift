#!/usr/bin/env bash
# Drift GNOME 50 host setup (plan §5.2, task M0-4). See docs/gnome-host-setup.md.
#
# Converges a GNOME Remote Desktop 50 host to the state Drift is tested against:
#   --system                    Remote Login: the system g-r-d daemon (:3389) is enabled,
#                               has a TLS certificate and RDP credentials.
#   --headless USER:PORT[:persistent]
#                               A headless g-r-d daemon for USER on a fixed PORT (port
#                               negotiation off) with a TLS certificate and credentials.
#                               "persistent" also enables gnome-headless-session@USER with
#                               the systemd drop-in that makes it start (DynamicUser=no,
#                               User=root: as gdm, polkit refuses it on Ubuntu 26.04; ADR
#                               GNOME-WIZARD-headless-setup).
#   --install                   Install gnome-remote-desktop and openssl first when missing
#                               (apt, dnf, zypper or pacman).
#   --desktop                   Install the smallest GNOME that runs a headless session when
#                               GNOME Shell or GDM is missing (no recommends, so no
#                               NetworkManager), keep GDM enabled and boot graphical.target.
#   --firewall                  Open every configured port (TCP) in ufw or firewalld, when
#                               one of them is active.
#   --allow-from CIDR           With --firewall: allow the ports only from this network.
#
# For every configured daemon whose certificate is known, a machine-readable line
#   fingerprint: <system|USER> <sha256, grdctl format>
# is printed, so a client can pin it up front (host/drift-gnome-setup does).
#
# Idempotent: every step reads the current state first and only acts when it differs.
# Modes:
#   --check    (default) change nothing; print what is ok and what would change.
#              Exit 0 when nothing would change, 2 when changes are pending.
#   --dry-run  same as --check.
#   --apply    make the changes. Exit 0 on success.
# Exit 1 on errors (for example missing credentials that the script cannot invent).
#
# Credentials are never printed. They are only written when given through the
# environment and different from the current value:
#   DRIFT_SYSTEM_RDP_USER / DRIFT_SYSTEM_RDP_PASS
#   DRIFT_HEADLESS_RDP_USER_<USER> / DRIFT_HEADLESS_RDP_PASS_<USER>
#     (<USER> upper-cased, non-alphanumerics replaced by "_", e.g. ..._DRIFTTEST2)
# Desktop Sharing credentials live in the user's login keyring and can only be set from
# GNOME Settings in an unlocked session (plan §1.9); this script does not touch them.
#
# host/drift-gnome-setup runs it from a Mac for one machine and adds the connection to Drift.
# Run it as root on the host, e.g. from the dev Mac:
#   ssh homelab@10.1.2.40 'sudo -n bash -s -- --check --system \
#       --headless drifttest:3391 --headless drifttest2:3392:persistent' < host/drift-host-setup.sh
set -euo pipefail

readonly DROPIN_DIR=/etc/systemd/system/gnome-headless-session@.service.d
readonly DROPIN_NAME=drift.conf
# Keep identical to host/gnome-headless-session-dropin.conf (an xtask test enforces it).
readonly DROPIN_CONTENT='[Service]
DynamicUser=no
User=root
Group=root'
readonly SYSTEM_CERT_DIR=/var/lib/gnome-remote-desktop/.local/share/gnome-remote-desktop/certificates

MODE=check
SYSTEM=0
INSTALL=0
DESKTOP=0
FIREWALL=0
ALLOW_FROM=
HEADLESS=()
CHANGES=0
ERRORS=0

usage() {
  sed -n '2,/^set -euo/p' "$0" 2>/dev/null | sed -e '$d' -e 's/^# \{0,1\}//' || true
}

ok() { printf 'ok:     %s\n' "$*"; }
err() {
  printf 'error:  %s\n' "$*"
  ERRORS=$((ERRORS + 1))
}

# change DESCRIPTION CMD...: runs CMD in --apply mode, otherwise prints it.
change() {
  local what=$1
  shift
  CHANGES=$((CHANGES + 1))
  if [[ $MODE == apply ]]; then
    printf 'change: %s\n' "$what"
    "$@"
  else
    printf 'would:  %s\n        $ %s\n' "$what" "$*"
  fi
}

# change_secret DESCRIPTION CMD...: like change, but never prints the command line.
change_secret() {
  local what=$1
  shift
  CHANGES=$((CHANGES + 1))
  if [[ $MODE == apply ]]; then
    printf 'change: %s\n' "$what"
    "$@"
  else
    printf 'would:  %s (command not shown: contains credentials)\n' "$what"
  fi
}

# field STATUS_TEXT NAME: value of a "NAME: value" line in `grdctl status` output.
field() {
  printf '%s\n' "$1" | sed -n "s/^[[:space:]]*$2: //p" | head -n1
}

is_unset() { [[ -z "$1" || "$1" == "(null)" ]]; }

# as_user USER CMD...: runs CMD as USER inside the user's session bus.
as_user() {
  local user=$1 uid
  shift
  uid=$(id -u "$user")
  runuser -u "$user" -- env "XDG_RUNTIME_DIR=/run/user/$uid" \
    "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$uid/bus" "$@"
}

# gen_cert OWNER DIR: self-signed RSA certificate rdp-tls.{crt,key} in DIR owned by OWNER.
gen_cert() {
  local owner=$1 dir=$2
  install -d -o "$owner" -g "$(id -gn "$owner")" -m 0755 "$dir"
  runuser -u "$owner" -- openssl req -new -x509 -newkey rsa:4096 -nodes -days 3650 \
    -subj "/CN=$(hostname)" -keyout "$dir/rdp-tls.key" -out "$dir/rdp-tls.crt" 2>/dev/null
  chmod 0600 "$dir/rdp-tls.key"
}

# fingerprint OWNER CERT: prints CERT's SHA-256 as `fingerprint: OWNER aa:bb:…` (lowercase hex
# pairs, the format of `grdctl status` and Drift's certificate pins).
fingerprint() {
  local owner=$1 cert=$2 fp
  [[ -r "$cert" ]] || return 0
  fp=$(openssl x509 -in "$cert" -noout -fingerprint -sha256 2>/dev/null | sed 's/^.*=//' | tr '[:upper:]' '[:lower:]')
  [[ -n "$fp" ]] && printf 'fingerprint: %s %s\n' "$owner" "$fp"
  return 0
}

# wait_for_bus USER UID: after starting a persistent session, gives its bus up to 30 s.
wait_for_bus() {
  local user=$1 uid=$2 i
  [[ $MODE == apply ]] || return 0
  for i in $(seq 30); do
    [[ -S "/run/user/$uid/bus" ]] && return 0
    [[ $i -eq 1 ]] && printf 'wait:   %s: waiting for the session to start\n' "$user"
    sleep 1
  done
}

# The package manager's install command, or nothing.
pkg_install() {
  if command -v apt-get >/dev/null; then
    apt-get update -q && DEBIAN_FRONTEND=noninteractive apt-get install -y "$@"
  elif command -v dnf >/dev/null; then
    dnf install -y "$@"
  elif command -v zypper >/dev/null; then
    zypper --non-interactive install "$@"
  elif command -v pacman >/dev/null; then
    pacman -S --needed --noconfirm "$@"
  else
    return 1
  fi
}

ensure_packages() {
  local missing=()
  command -v grdctl >/dev/null || missing+=(gnome-remote-desktop)
  command -v openssl >/dev/null || missing+=(openssl)
  if [[ ${#missing[@]} -eq 0 ]]; then
    ok "gnome-remote-desktop and openssl are installed"
  elif command -v apt-get >/dev/null || command -v dnf >/dev/null ||
    command -v zypper >/dev/null || command -v pacman >/dev/null; then
    change "install ${missing[*]}" pkg_install "${missing[@]}"
  else
    err "install ${missing[*]} with this system's package manager, then run again"
  fi
}

# install_desktop: the smallest GNOME that runs a headless session (ADR GNOME-WIZARD-headless-setup).
# Without recommends: the desktop metapackages pull in NetworkManager, which would compete with
# systemd-networkd for the NIC of a machine that is only reached over the network.
install_desktop() {
  if command -v apt-get >/dev/null; then
    apt-get update -q
    if apt-cache show ubuntu-session >/dev/null 2>&1; then
      DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        gdm3 ubuntu-session gnome-remote-desktop ptyxis openssl fonts-cantarell
    else
      DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        gdm3 gnome-session gnome-remote-desktop ptyxis openssl fonts-cantarell
    fi
  elif command -v dnf >/dev/null; then
    dnf install -y --setopt=install_weak_deps=False gdm gnome-shell gnome-remote-desktop ptyxis openssl
  elif command -v zypper >/dev/null; then
    zypper --non-interactive install --no-recommends gdm gnome-shell gnome-remote-desktop ptyxis openssl
  elif command -v pacman >/dev/null; then
    pacman -S --needed --noconfirm gdm gnome-shell gnome-remote-desktop ptyxis openssl
  else
    return 1
  fi
}

ensure_desktop() {
  if command -v gnome-shell >/dev/null && systemctl cat gdm.service >/dev/null 2>&1; then
    ok "GNOME Shell and GDM are installed"
  elif command -v apt-get >/dev/null || command -v dnf >/dev/null ||
    command -v zypper >/dev/null || command -v pacman >/dev/null; then
    change "install a minimal GNOME (GNOME Shell, GDM, gnome-remote-desktop)" install_desktop
  else
    err "install GNOME Shell and GDM with this system's package manager, then run again"
    return
  fi
  # gnome-headless-session@.service has Requires=gdm.service.
  ensure_unit gdm.service
  if [[ "$(systemctl get-default 2>/dev/null)" == "graphical.target" ]]; then
    ok "boots into graphical.target"
  else
    change "boot into graphical.target" systemctl set-default graphical.target
  fi
}

# Headless sessions need gnome-remote-desktop 46 or later (grdctl --headless).
check_headless_support() {
  if ! command -v grdctl >/dev/null; then
    if [[ $MODE == apply ]]; then
      err "grdctl is missing; pass --install or install gnome-remote-desktop"
    else
      printf 'skip:   headless support is checked once gnome-remote-desktop is installed\n'
    fi
    return
  fi
  if grdctl --help 2>&1 | grep -q -- '--headless'; then
    ok "grdctl supports headless sessions"
  else
    err "this gnome-remote-desktop has no headless mode; GNOME 46 or later is needed"
  fi
}

# open_port PORT: allows PORT/tcp in an active ufw or firewalld (only from ALLOW_FROM when set).
open_port() {
  local port=$1
  if [[ -n "$ALLOW_FROM" ]]; then
    open_port_from "$port"
    return
  fi
  if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q '^Status: active'; then
    if ufw status 2>/dev/null | grep -Eq "^${port}/tcp[[:space:]]+ALLOW"; then
      ok "ufw allows ${port}/tcp"
    else
      change "allow ${port}/tcp in ufw" ufw allow "${port}/tcp"
    fi
  elif command -v firewall-cmd >/dev/null && firewall-cmd --state >/dev/null 2>&1; then
    if firewall-cmd --query-port="${port}/tcp" >/dev/null 2>&1; then
      ok "firewalld allows ${port}/tcp"
    else
      change "allow ${port}/tcp in firewalld" firewalld_add "$port"
    fi
  else
    ok "no active ufw or firewalld; ${port}/tcp is not filtered here"
  fi
}

# The firewalld rich rule that allows PORT/tcp from ALLOW_FROM.
rich_rule() {
  local family=ipv4
  [[ "$ALLOW_FROM" == *:* ]] && family=ipv6
  printf 'rule family="%s" source address="%s" port port="%s" protocol="tcp" accept' \
    "$family" "$ALLOW_FROM" "$1"
}

open_port_from() {
  local port=$1
  if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q '^Status: active'; then
    if ufw status 2>/dev/null |
      awk -v p="${port}/tcp" -v f="$ALLOW_FROM" '$1 == p && $2 == "ALLOW" && $3 == f { m = 1 } END { exit !m }'; then
      ok "ufw allows ${port}/tcp from $ALLOW_FROM"
    else
      change "allow ${port}/tcp from $ALLOW_FROM in ufw" \
        ufw allow from "$ALLOW_FROM" to any port "$port" proto tcp
    fi
  elif command -v firewall-cmd >/dev/null && firewall-cmd --state >/dev/null 2>&1; then
    if firewall-cmd --query-rich-rule="$(rich_rule "$port")" >/dev/null 2>&1; then
      ok "firewalld allows ${port}/tcp from $ALLOW_FROM"
    else
      change "allow ${port}/tcp from $ALLOW_FROM in firewalld" firewalld_add_from "$port"
    fi
  else
    ok "no active ufw or firewalld; ${port}/tcp is not filtered here"
  fi
}

firewalld_add_from() {
  firewall-cmd --add-rich-rule="$(rich_rule "$1")" &&
    firewall-cmd --permanent --add-rich-rule="$(rich_rule "$1")"
}

firewalld_add() {
  firewall-cmd --add-port="$1/tcp" && firewall-cmd --permanent --add-port="$1/tcp"
}

write_dropin() {
  install -d -m 0755 "$DROPIN_DIR"
  printf '%s\n' "$DROPIN_CONTENT" >"$DROPIN_DIR/$DROPIN_NAME"
  chmod 0644 "$DROPIN_DIR/$DROPIN_NAME"
  systemctl daemon-reload
}

ensure_unit() {
  local unit=$1
  if systemctl is-enabled --quiet "$unit" 2>/dev/null && systemctl is-active --quiet "$unit"; then
    ok "$unit enabled and active"
  else
    change "enable and start $unit" systemctl enable --now "$unit"
  fi
}

ensure_user_unit() {
  local user=$1 unit=$2
  if as_user "$user" systemctl --user is-enabled --quiet "$unit" 2>/dev/null &&
    as_user "$user" systemctl --user is-active --quiet "$unit"; then
    ok "$user: $unit enabled and active"
  else
    change "$user: enable and start $unit" as_user "$user" systemctl --user enable --now "$unit"
  fi
}

# Upper-cased, shell-safe form of a user name for environment variable names.
env_suffix() {
  printf '%s' "$1" | tr '[:lower:]' '[:upper:]' | tr -c 'A-Z0-9' '_'
}

setup_system() {
  local st cert key current
  if ! st=$(grdctl --system status 2>/dev/null); then
    err "grdctl --system status failed; is gnome-remote-desktop 50 installed?"
    return
  fi

  cert=$(field "$st" "TLS certificate")
  key=$(field "$st" "TLS key")
  if is_unset "$cert" || is_unset "$key"; then
    change "generate the system TLS certificate in $SYSTEM_CERT_DIR" \
      gen_cert gnome-remote-desktop "$SYSTEM_CERT_DIR"
    change "set the system TLS key" grdctl --system rdp set-tls-key "$SYSTEM_CERT_DIR/rdp-tls.key"
    change "set the system TLS certificate" grdctl --system rdp set-tls-cert "$SYSTEM_CERT_DIR/rdp-tls.crt"
  else
    ok "system TLS certificate configured ($cert)"
  fi
  [[ $MODE == apply ]] && is_unset "$cert" && cert="$SYSTEM_CERT_DIR/rdp-tls.crt"
  is_unset "$cert" || fingerprint system "$cert"

  if [[ -n "${DRIFT_SYSTEM_RDP_USER:-}" && -n "${DRIFT_SYSTEM_RDP_PASS:-}" ]]; then
    current=$(grdctl --system status --show-credentials 2>/dev/null || true)
    if [[ "$(field "$current" Username)" == "$DRIFT_SYSTEM_RDP_USER" &&
      "$(field "$current" Password)" == "$DRIFT_SYSTEM_RDP_PASS" ]]; then
      ok "system RDP credentials match DRIFT_SYSTEM_RDP_USER/PASS"
    else
      change_secret "set the system RDP credentials" \
        grdctl --system rdp set-credentials "$DRIFT_SYSTEM_RDP_USER" "$DRIFT_SYSTEM_RDP_PASS"
    fi
  elif [[ "$(field "$st" Username)" == "(hidden)" && "$(field "$st" Password)" == "(hidden)" ]]; then
    ok "system RDP credentials are set (unchanged)"
  else
    err "system RDP credentials are empty; export DRIFT_SYSTEM_RDP_USER and DRIFT_SYSTEM_RDP_PASS"
  fi

  if [[ "$(field "$st" Status)" == "enabled" ]]; then
    ok "system RDP enabled"
  else
    change "enable system RDP" grdctl --system rdp enable
  fi

  ensure_unit gnome-remote-desktop.service
}

ensure_dropin() {
  local user=$1 props
  props=$(systemctl show "gnome-headless-session@$user.service" -p DynamicUser -p User 2>/dev/null || true)
  if grep -qx 'DynamicUser=no' <<<"$props" && grep -qx 'User=root' <<<"$props"; then
    ok "gnome-headless-session@.service runs with DynamicUser=no, User=root"
  else
    change "install $DROPIN_DIR/$DROPIN_NAME (DynamicUser=no, User=root)" write_dropin
  fi
}

setup_headless() {
  local user=$1 port=$2 persistent=$3 uid st cert key current var_user var_pass
  if ! uid=$(id -u "$user" 2>/dev/null); then
    err "user $user does not exist"
    return
  fi

  if [[ "$persistent" == "persistent" ]]; then
    ensure_dropin "$user"
    ensure_unit "gnome-headless-session@$user.service"
    wait_for_bus "$user" "$uid"
  fi

  if [[ ! -S "/run/user/$uid/bus" ]]; then
    if [[ $MODE == apply && "$persistent" != "persistent" ]]; then
      err "$user has no running session (/run/user/$uid/bus); use --headless $user:$port:persistent"
    else
      printf 'skip:   %s has no session bus yet; re-run after the session has started\n' "$user"
      [[ $MODE == apply ]] || CHANGES=$((CHANGES + 1))
    fi
    return
  fi

  if ! st=$(as_user "$user" grdctl --headless status 2>/dev/null); then
    err "$user: grdctl --headless status failed"
    return
  fi

  cert=$(field "$st" "TLS certificate")
  key=$(field "$st" "TLS key")
  local dir
  dir="$(getent passwd "$user" | cut -d: -f6)/.local/share/gnome-remote-desktop/certificates"
  if is_unset "$cert" || is_unset "$key"; then
    change "$user: generate a TLS certificate in $dir" gen_cert "$user" "$dir"
    change "$user: set the headless TLS key" as_user "$user" grdctl --headless rdp set-tls-key "$dir/rdp-tls.key"
    change "$user: set the headless TLS certificate" as_user "$user" grdctl --headless rdp set-tls-cert "$dir/rdp-tls.crt"
  else
    ok "$user: TLS certificate configured ($cert)"
  fi
  [[ $MODE == apply ]] && is_unset "$cert" && cert="$dir/rdp-tls.crt"
  is_unset "$cert" || fingerprint "$user" "$cert"

  var_user="DRIFT_HEADLESS_RDP_USER_$(env_suffix "$user")"
  var_pass="DRIFT_HEADLESS_RDP_PASS_$(env_suffix "$user")"
  if [[ -n "${!var_user:-}" && -n "${!var_pass:-}" ]]; then
    current=$(as_user "$user" grdctl --headless status --show-credentials 2>/dev/null || true)
    if [[ "$(field "$current" Username)" == "${!var_user}" && "$(field "$current" Password)" == "${!var_pass}" ]]; then
      ok "$user: RDP credentials match $var_user/$var_pass"
    else
      change_secret "$user: set the headless RDP credentials" \
        as_user "$user" grdctl --headless rdp set-credentials "${!var_user}" "${!var_pass}"
    fi
  elif [[ "$(field "$st" Username)" == "(hidden)" && "$(field "$st" Password)" == "(hidden)" ]]; then
    ok "$user: RDP credentials are set (unchanged)"
  else
    err "$user: RDP credentials are empty; export $var_user and $var_pass"
  fi

  if [[ "$(field "$st" Port)" == "$port" ]]; then
    ok "$user: RDP port $port"
  else
    change "$user: set the RDP port to $port (effective at the next daemon start)" \
      as_user "$user" grdctl --headless rdp set-port "$port"
  fi
  if [[ "$(field "$st" "Negotiate port")" == "no" ]]; then
    ok "$user: port negotiation disabled"
  else
    change "$user: disable port negotiation" as_user "$user" grdctl --headless rdp disable-port-negotiation
  fi

  if [[ "$(field "$st" Status)" == "enabled" ]]; then
    ok "$user: headless RDP enabled"
  else
    change "$user: enable headless RDP" as_user "$user" grdctl --headless rdp enable
  fi

  ensure_user_unit "$user" gnome-remote-desktop-headless.service

  # A headless session must not suspend the host because nobody touches it.
  current=$(as_user "$user" gsettings get org.gnome.settings-daemon.plugins.power sleep-inactive-ac-type 2>/dev/null || true)
  if [[ "$current" == "'nothing'" ]]; then
    ok "$user: automatic suspend disabled"
  else
    change "$user: disable automatic suspend" \
      as_user "$user" gsettings set org.gnome.settings-daemon.plugins.power sleep-inactive-ac-type nothing
  fi

  # Nobody sits at a headless session: a locked one only swallows input (docs/gnome-host-setup.md),
  # and RDP credentials already guard it.
  # …nor blank its screen.
  current=$(as_user "$user" gsettings get org.gnome.desktop.session idle-delay 2>/dev/null || true)
  if [[ "$current" == "uint32 0" ]]; then
    ok "$user: screen blanking disabled"
  else
    change "$user: disable screen blanking" \
      as_user "$user" gsettings set org.gnome.desktop.session idle-delay 0
  fi

  current=$(as_user "$user" gsettings get org.gnome.desktop.screensaver lock-enabled 2>/dev/null || true)
  if [[ "$current" == "false" ]]; then
    ok "$user: screen lock disabled"
  else
    change "$user: disable the screen lock" \
      as_user "$user" gsettings set org.gnome.desktop.screensaver lock-enabled false
  fi
}

# valid_cidr NET: an IPv4 or IPv6 address with an optional prefix length.
valid_cidr() {
  local net=$1 addr prefix o
  addr=${net%%/*}
  prefix=
  [[ "$net" == */* ]] && prefix=${net#*/}
  if [[ "$addr" =~ ^([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})$ ]]; then
    for o in "${BASH_REMATCH[@]:1}"; do
      ((10#$o <= 255)) || return 1
    done
    [[ -z "$prefix" ]] || { [[ "$prefix" =~ ^[0-9]{1,2}$ ]] && ((10#$prefix <= 32)); }
  elif [[ "$addr" == *:* && "$addr" =~ ^[0-9A-Fa-f:]+$ ]]; then
    [[ -z "$prefix" ]] || { [[ "$prefix" =~ ^[0-9]{1,3}$ ]] && ((10#$prefix <= 128)); }
  else
    return 1
  fi
}

main() {
  while [[ $# -gt 0 ]]; do
    case $1 in
    --check | --dry-run) MODE=check ;;
    --apply) MODE=apply ;;
    --system) SYSTEM=1 ;;
    --install) INSTALL=1 ;;
    --desktop) DESKTOP=1 ;;
    --firewall) FIREWALL=1 ;;
    --allow-from)
      if [[ $# -lt 2 ]] || ! valid_cidr "$2"; then
        echo "bad --allow-from '${2:-}' (want a network such as 10.1.0.0/21 or fd00::/64)" >&2
        exit 1
      fi
      ALLOW_FROM=$2
      shift
      ;;
    --headless)
      [[ $# -ge 2 ]] || {
        echo "--headless needs USER:PORT[:persistent]" >&2
        exit 1
      }
      HEADLESS+=("$2")
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1 (see --help)" >&2
      exit 1
      ;;
    esac
    shift
  done

  if [[ $SYSTEM -eq 0 && ${#HEADLESS[@]} -eq 0 ]]; then
    echo "nothing to do: pass --system and/or --headless USER:PORT[:persistent]" >&2
    exit 1
  fi
  if [[ $EUID -ne 0 ]]; then
    echo "run as root (sudo)" >&2
    exit 1
  fi

  printf 'drift-host-setup: mode=%s\n' "$MODE"
  if [[ $DESKTOP -eq 1 ]]; then
    ensure_desktop
  fi
  if [[ $INSTALL -eq 1 ]]; then
    ensure_packages
  fi
  if [[ ${#HEADLESS[@]} -gt 0 ]]; then
    check_headless_support
  fi
  if [[ $SYSTEM -eq 1 ]]; then
    setup_system
    if [[ $FIREWALL -eq 1 ]]; then
      open_port 3389
    fi
  fi
  local spec user port persistent
  for spec in "${HEADLESS[@]}"; do
    IFS=: read -r user port persistent <<<"$spec"
    if [[ -z "$user" || ! "$port" =~ ^[0-9]+$ ]]; then
      err "bad --headless spec '$spec' (want USER:PORT[:persistent])"
      continue
    fi
    setup_headless "$user" "$port" "${persistent:-}"
    if [[ $FIREWALL -eq 1 ]]; then
      open_port "$port"
    fi
  done

  if [[ $MODE == apply ]]; then
    printf 'drift-host-setup: %d change(s) applied, %d error(s)\n' "$CHANGES" "$ERRORS"
  else
    printf 'drift-host-setup: %d change(s) pending, %d error(s)\n' "$CHANGES" "$ERRORS"
  fi
  if [[ $ERRORS -gt 0 ]]; then
    exit 1
  fi
  if [[ $MODE == check && $CHANGES -gt 0 ]]; then
    exit 2
  fi
}

main "$@"
