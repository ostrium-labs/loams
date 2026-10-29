#!/usr/bin/env bash
# Run gateway scenarios in a quiet window (coordinator mechanism, 2026-10-28):
# hold the bench lock, set .he0-shared/quiet so buildslot.sh holds new builds,
# wait for running rustc to finish and the load average to fall below 2, run,
# then clear the flag. Use it under benchslot.sh:
#
#   .he0-shared/benchslot.sh bq1-gw bench/gateway/quiet.sh <label> <scenario>...
set -euo pipefail
label=${1:?label}; shift
shared=${SHARED:-$(cd "$(dirname "$0")/../../.." && pwd)/.he0-shared}
quiet="$shared/quiet"
echo $(( $(date +%s) + ${QUIET_SECONDS:-1200} )) > "$quiet"
trap 'rm -f "$quiet"' EXIT
deadline=$(( $(date +%s) + 600 ))
while pgrep -x rustc >/dev/null || awk '{exit !($1 >= 2)}' /proc/loadavg; do
  if [ "$(date +%s)" -gt "$deadline" ]; then
    echo "quiet.sh: still busy after 10 min (load $(cut -d' ' -f1 /proc/loadavg)); running anyway, the records say so" >&2
    break
  fi
  sleep 5
done
for scenario in "$@"; do
  "$(dirname "$0")/run.sh" "$scenario" "$label"
done
