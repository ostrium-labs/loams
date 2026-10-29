#!/usr/bin/env bash
# Run guard.sh --once every minute from a systemd --user timer, so the guard does not
# depend on any shell or agent session staying alive.
#   guard-timer.sh install | start | stop | status
# install: persistent units in ~/.config/systemd/user (enabled, so the guard comes back
# after a reboot; a transient start does not: 2026-10-28 reboot).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
unit=cfb-guard
case "${1:-status}" in
  start) systemd-run --user --unit "$unit" --on-calendar='*:*:00' --timer-property=AccuracySec=5s \
           --setenv=HOME="$HOME" --setenv=PATH="$PATH" \
           /bin/bash -c "'$here/guard.sh' --once >>'$HOME/.cfb-state/guard.log' 2>&1" ;;
  install)
    d="$HOME/.config/systemd/user"; mkdir -p "$d"
    cat >"$d/$unit.service" <<EOF
[Unit]
Description=CFB Confluent budget guard (one tick)
[Service]
Type=oneshot
Environment=HOME=$HOME
Environment=PATH=$PATH
ExecStart=/bin/bash -c '"$here/guard.sh" --once >>"$HOME/.cfb-state/guard.log" 2>&1'
EOF
    cat >"$d/$unit.timer" <<EOF
[Unit]
Description=CFB Confluent budget guard, every minute
[Timer]
OnCalendar=*:*:00
AccuracySec=5s
Persistent=true
[Install]
WantedBy=timers.target
EOF
    systemctl --user daemon-reload && systemctl --user enable --now "$unit.timer" ;;
  stop)  systemctl --user disable --now "$unit.timer" 2>/dev/null || systemctl --user stop "$unit.timer" "$unit.service" 2>/dev/null || true ;;
  status) systemctl --user list-timers "$unit.timer" --no-pager; cat "$HOME/.cfb-state/guard.heartbeat" 2>/dev/null ;;
esac
