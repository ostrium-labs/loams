#!/usr/bin/env bash
# Stepped in-region push: hold an in-region Flink load on CLUSTER while max eCKU
# steps up, then record the per-minute Metrics API series. Finds where
# Confluent throttles produce for each cap (design 80 hard cap: max_ecku).
#
#   push.sh CLUSTER ECKU:MINUTES [ECKU:MINUTES ...]
#   e.g. push.sh loams-cfb-aws-1 1:5 4:6 10:6 1:0
#
# The Flink statements that produce the load are started separately (flink.sh run);
# a final step with 0 minutes sets the cap back down. Results:
#   results/$CFB_DAY/push/CLUSTER-steps.tsv   utc, max_ecku
#   results/$CFB_DAY/push/CLUSTER-series.json Metrics API, PT1M
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
state="${CFB_STATE:-$HOME/.cfb-state}"
day="${CFB_DAY:-2026-10-28}"
c="$1"; shift
out="$here/results/$day/push"; mkdir -p "$out"
id="$(jq -r .id "$state/clusters/$c/cluster.json")"
start="$(date -u -d "-2 min" +%Y-%m-%dT%H:%M:00Z)"
for step in "$@"; do
  e="${step%%:*}" m="${step##*:}"
  "$here/cfb.sh" ecku "$c" "$e"
  printf '%s\t%s\n' "$(date -u +%FT%TZ)" "$e" >>"$out/$c-steps.tsv"
  (( m > 0 )) && sleep $(( m * 60 ))
done
sleep 180  # the Metrics API lags by a few minutes
"$here/collect.sh" series "$id" "$start" "$(date -u +%Y-%m-%dT%H:%M:00Z)" PT1M "$out/$c-series-$(date -u +%H%M).json"
