#!/usr/bin/env bash
# Read-only state report for `cargo xtask host-setup-check` (task M0-4).
#
# Runs on the GNOME host as a user with passwordless sudo and prints `### <section> <args>`
# blocks that xtask parses (xtask/src/host_check.rs). It changes nothing and never prints
# credentials: plain `grdctl status` only prints "(hidden)" or "(empty)" for them.
#
# usage: ssh <host> 'bash -s -- USER:PORT[:persistent]...' < host/drift-host-report.sh
set -uo pipefail

section() { printf '### %s\n' "$*"; }

# Runs a command as <user> with the user's session bus (needed by grdctl --headless and
# systemctl --user).
as_user() {
  local user=$1 uid
  shift
  uid=$(id -u "$user") || return 1
  sudo -n -u "$user" env "XDG_RUNTIME_DIR=/run/user/$uid" \
    "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$uid/bus" "$@"
}

# Prints a `cert <owner>` section: unset | valid | expired | missing <path>.
cert_state() {
  local owner=$1 status_text=$2 cert
  cert=$(printf '%s\n' "$status_text" | sed -n 's/^[[:space:]]*TLS certificate: //p' | head -n1)
  section "cert $owner"
  if [[ -z "$cert" || "$cert" == "(null)" ]]; then
    echo "unset"
  else
    # Ask openssl, not `test -r`: on AnduinOS 2.0.3 `sudo test -r` is false for root-readable 0600 files.
    local out
    if out=$(sudo -n openssl x509 -in "$cert" -noout -checkend 0 2>&1); then
      echo "valid"
    elif [[ "$out" == *"will expire"* ]]; then
      echo "expired"
    else
      echo "missing $cert"
    fi
  fi
}

section "ss-ltnp"
sudo -n ss -ltnp 2>&1

section "ps"
pids=$(pgrep -d, -f gnome-remote-desktop-daemon || true)
if [[ -n "$pids" ]]; then
  ps -o pid=,user:32=,args= -p "$pids"
fi

section "unit gnome-remote-desktop.service"
systemctl is-active gnome-remote-desktop.service 2>&1 || true
systemctl is-enabled gnome-remote-desktop.service 2>&1 || true

section "grdctl-system"
system_status=$(sudo -n grdctl --system status 2>&1)
printf '%s\n' "$system_status"
cert_state system "$system_status"

for spec in "$@"; do
  IFS=: read -r user _port persistent <<<"$spec"
  section "grdctl-headless $user"
  headless_status=$(as_user "$user" grdctl --headless status 2>&1)
  printf '%s\n' "$headless_status"
  cert_state "$user" "$headless_status"

  section "user-unit $user gnome-remote-desktop-headless.service"
  as_user "$user" systemctl --user is-active gnome-remote-desktop-headless.service 2>&1 || true
  as_user "$user" systemctl --user is-enabled gnome-remote-desktop-headless.service 2>&1 || true

  if [[ "${persistent:-}" == "persistent" ]]; then
    section "unit gnome-headless-session@$user.service"
    systemctl is-active "gnome-headless-session@$user.service" 2>&1 || true
    systemctl is-enabled "gnome-headless-session@$user.service" 2>&1 || true
    section "unit-props gnome-headless-session@$user.service"
    systemctl show "gnome-headless-session@$user.service" -p DynamicUser -p User -p Restart 2>&1 || true
  fi
done
section "end"
