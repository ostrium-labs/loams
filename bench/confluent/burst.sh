#!/usr/bin/env bash
# Aggregate records/s burst across several Dedicated clusters, driven by in-region
# Flink: tiny records (SIZE bytes of payload, default 10) fanned out from bounded
# faker ids. Samples every pool's CFU each minute so the report can show the
# generator had headroom (cluster-bound, not generator-bound).
#
#   burst.sh TAG MINUTES SIZE "POOL1 POOL2 ..." CLUSTER:STATEMENTS [CLUSTER:STATEMENTS ...]
#
# Statements for a cluster are spread round-robin over the pools (each pool runs
# statements for any cluster in its region). Every statement is stopped at the end
# (and is bounded anyway: 100k ids x 3000 fan-out). Results:
#   results/$CFB_DAY/burst/TAG-{events.tsv,cfu.tsv,<cluster>-series.json}
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
state="${CFB_STATE:-$HOME/.cfb-state}"; day="${CFB_DAY:-2026-10-28}"
# shellcheck source=lib/cc.sh
source "$here/lib/cc.sh"
tag="$1" mins="$2" size="$3" pools=($4); shift 4
f="$here/flink.sh"; fs="$here/flink-setup.sh"
out="$here/results/$day/burst"; mkdir -p "$out"
ev() { printf '%s\t%s\n' "$(date -u +%FT%TZ)" "$*" | tee -a "$out/$tag-events.tsv"; }
arr="$(seq -s, 1 3000)"
start="$(date -u -d '-2 min' +%Y-%m-%dT%H:%M:00Z)"
names=()
# tables on every cluster (idempotent), through the first pool
for spec in "$@"; do
  c="${spec%%:*}"
  "$fs" "${pools[0]}" "$c" tables 96 >/dev/null 2>&1 || true
  "$f" run "${pools[0]}" "$c" "cfb-ddl-tiny-${c#loams-cfb-}" "CREATE TABLE IF NOT EXISTS cfb_tiny (val STRING) DISTRIBUTED INTO 96 BUCKETS WITH ('value.format' = 'raw', 'kafka.retention.time' = '30 min', 'kafka.retention.size' = '200 mb')" >/dev/null
done
sleep 15
ev "start tag=$tag size=$size clusters=$* pools=${pools[*]}"
# one submission list per pool, submitted in parallel (each flink.sh call is its own process)
declare -A plist
i=0
for spec in "$@"; do
  c="${spec%%:*}" k="${spec##*:}"; short="${c#loams-cfb-}"
  for j in $(seq 1 "$k"); do
    p="${pools[$(( i % ${#pools[@]} ))]}"; i=$((i + 1))
    n="cfb-$tag-$short-$j"
    plist[$p]+="$c|$n "
    names+=("$p|$n")
  done
done
sql="INSERT INTO ${CFB_BURST_TABLE:-cfb_tiny} SELECT CONCAT(CAST(g.id AS STRING), REPEAT('z', $size)) FROM cfb_ids AS g CROSS JOIN UNNEST(ARRAY[$arr]) AS t(x)"
for p in "${!plist[@]}"; do
  ( for cn in ${plist[$p]}; do "$f" run "$p" "${cn%%|*}" "${cn##*|}" "$sql" >/dev/null 2>&1 || true; done ) &
done
wait
ev "submitted ${#names[@]} statements"
end=$(( $(date +%s) + mins * 60 ))
while (( $(date +%s) < end )); do
  for p in "${pools[@]}"; do
    pid="$(jq -r .id "$state/pools/$p/pool.json")"
    printf '%s\t%s\t%s\n' "$(date -u +%FT%TZ)" "$p" \
      "$(cc GET "/fcpm/v2/compute-pools/$pid?environment=$CFB_ENV" | jq -c '{cfu:.status.current_cfu, max:.spec.max_cfu}')" >>"$out/$tag-cfu.tsv"
  done
  sleep 60
done
ev "stopping"
for pn in "${names[@]}"; do "$f" stop "${pn%%|*}" "${pn##*|}" >/dev/null 2>&1 || true; done
ev "stopped"
sleep 180
for spec in "$@"; do
  c="${spec%%:*}"
  "$here/collect.sh" series "$(jq -r .id "$state/clusters/$c/cluster.json")" "$start" "$(date -u +%Y-%m-%dT%H:%M:00Z)" PT1M "$out/$tag-$c-series.json" || true
done
ev "done"
