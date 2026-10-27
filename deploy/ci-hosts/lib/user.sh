# shellcheck shell=bash
# The unprivileged `ci` user that runs NativeLink, sccache's store and the
# GitHub runners. It has no sudo, no access to the owner's home and its own
# rootless podman.

CI_USER=${CI_USER:-ci}

ensure_ci_user() {
  if ! id "$CI_USER" >/dev/null 2>&1; then
    log "creating user $CI_USER"
    useradd --create-home --home-dir "/home/$CI_USER" --shell /bin/bash --user-group "$CI_USER"
  fi
  chmod 700 "/home/$CI_USER"
  # Never an administrator: drop it from the groups that grant sudo or system access.
  local g
  for g in wheel sudo adm admin docker; do
    if id -nG "$CI_USER" | tr ' ' '\n' | grep -qx "$g"; then
      gpasswd -d "$CI_USER" "$g" >/dev/null
    fi
  done
  passwd -l "$CI_USER" >/dev/null 2>&1 || true
  ensure_subids
  # Lingering starts the user's systemd manager at boot (rootless podman, user units).
  loginctl enable-linger "$CI_USER"
  check_owner_home
}

# Rootless podman needs subordinate uid and gid ranges. Picks a free 65536 block
# above every range already allocated.
ensure_subids() {
  local f
  for f in /etc/subuid /etc/subgid; do
    touch "$f"
    grep -q "^$CI_USER:" "$f" && continue
    local start
    start=$(awk -F: 'BEGIN{m=100000} {e=$2+$3; if (e>m) m=e} END{print m}' "$f")
    start=$(((start + 65535) / 65536 * 65536))
    echo "$CI_USER:$start:65536" >>"$f"
    log "allocated $f range $start+65536 for $CI_USER"
  done
}

# The owner's home must not be readable by the ci user (credential files live there).
check_owner_home() {
  local owner_home=${OWNER_HOME:-}
  [[ -n $owner_home && -d $owner_home ]] || return 0
  local mode
  mode=$(stat -c '%a' "$owner_home")
  if [[ ${mode: -1} != 0 ]]; then
    warn "$owner_home is mode $mode: other users (including $CI_USER) can enter it. Run: chmod 700 $owner_home"
  fi
}

# DATA_ROOT holds every cache and the runners' work directories.
ensure_data_root() {
  install -d -o "$CI_USER" -g "$CI_USER" -m 750 "$DATA_ROOT"
  install -d -o root -g "$CI_USER" -m 750 /etc/loams-ci
  install -d -o root -g root -m 755 /opt/loams-ci /opt/loams-ci/bin
}

# Runs a command as the ci user with its user systemd/podman environment.
as_ci() {
  local uid
  uid=$(id -u "$CI_USER")
  sudo -u "$CI_USER" -H env XDG_RUNTIME_DIR="/run/user/$uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus" "$@"
}
