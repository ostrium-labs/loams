#!/usr/bin/env bash
# Run guard.sh --once every minute from a systemd --user timer, so the guard does not
# depend on any shell or agent session staying alive.
#   guard-timer.sh start | stop | status
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
unit=cfb-guard
case "${1:-status}" in
  start) systemd-run --user --unit "$unit" --on-calendar='*:*:00' --timer-property=AccuracySec=5s \
           --setenv=HOME="$HOME" --setenv=PATH="$PATH" \
           /bin/bash -c "'$here/guard.sh' --once >>'$HOME/.cfb-state/guard.log' 2>&1" ;;
  stop)  systemctl --user stop "$unit.timer" "$unit.service" 2>/dev/null || true ;;
  status) systemctl --user list-timers "$unit.timer" --no-pager; cat "$HOME/.cfb-state/guard.heartbeat" 2>/dev/null ;;
esac
