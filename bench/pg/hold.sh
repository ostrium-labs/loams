#!/usr/bin/env bash
# hold.sh <script> [args...]: waits until the laptop is quiet (no rustc,
# 1-minute load under QUIET_LOAD), then takes the exclusive bench slot and
# runs the script. Builds by other agents are not stopped by the bench slot,
# and a build on the bench's cores ruins the numbers (the first paths run,
# 2026-10-28 12:12Z, saw load 35 and kswapd busy). Gives up waiting after
# QUIET_WAIT_S and runs anyway; every result records the background load.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
slot=${BENCHSLOT:-$HOME/Documents/Ostriumlabs/loams-wt/.he0-shared/benchslot.sh}
: "${QUIET_LOAD:=3}" "${QUIET_WAIT_S:=2700}"
quiet() {
  ! pgrep -x rustc >/dev/null && ! pgrep -x cargo >/dev/null &&
    awk -v max="$QUIET_LOAD" '{exit !($1 < max)}' /proc/loadavg
}
start=$(date +%s)
until quiet; do
  if [ $(($(date +%s) - start)) -ge "$QUIET_WAIT_S" ]; then
    echo "hold.sh: not quiet after ${QUIET_WAIT_S}s ($(cut -d' ' -f1-3 /proc/loadavg)); running anyway" >&2
    break
  fi
  sleep 15
done
echo "hold.sh: load $(cut -d' ' -f1-3 /proc/loadavg) after $(($(date +%s) - start))s" >&2
exec "$slot" bq1-pg "$here/$1" "${@:2}"
