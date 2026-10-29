#!/usr/bin/env bash
# The TiKV phases of BQ1-kv: starts Loams's TiKV playground (PD + one TiKV
# v8.5.8, API v2, tag loams-bq1kv), pins PD and TiKV to cores 4-9, runs
# `suite.sh <phase>` with their CPU accounted, and stops the playground.
#   .he0-shared/benchslot.sh bq1-kv bench/kv/tikv.sh tikv tikv-scale
set -euo pipefail
H=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$H/../.." && pwd)
TAG=loams-bq1kv
PG=$ROOT/scripts/tikv/playground.sh
stop() { "$PG" stop --tag "$TAG" >/dev/null 2>&1 || true; }
trap stop EXIT
"$PG" start --tag "$TAG" --force
tikv=$(pgrep -f -- "tikv-server .*/\.tiup/data/$TAG/" | head -1)
pd=$(pgrep -f -- "pd-server .*/\.tiup/data/$TAG/" | head -1)
[ -n "$tikv" ] && [ -n "$pd" ] || { echo "no tikv/pd pid" >&2; exit 1; }
taskset -a -p -c 4-9 "$tikv" >/dev/null
taskset -a -p -c 4-9 "$pd" >/dev/null
echo "tikv=$tikv pd=$pd pinned to 4-9"
export EXTRA_PIDS="$tikv $pd"
for phase in "$@"; do "$H/suite.sh" "$phase"; done
