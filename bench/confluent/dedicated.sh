#!/usr/bin/env bash
# Per-CKU capacity of a Dedicated (or Standard) cluster, driven from inside the
# region by Confluent Flink, in three timed phases; every statement is stopped
# here (and is bounded anyway: flink-setup.sh).
#
#   dedicated.sh POOL CLUSTER CKU [MINUTES_A=12] [MINUTES_B=8] [MINUTES_C=8]
#
#   A  produce 1 KB records:   GEN_PER_CKU x CKU statements  -> ingress MB/s
#   B  produce 100 B records:  GEN_PER_CKU x CKU statements  -> records/s, requests/s
#   C  consume:                windowed COUNT(*) over the 1 KB topic from the earliest
#                              offset (readers in parallel)  -> egress MB/s
#
# Results: results/$CFB_DAY/dedicated/CLUSTER-{phases.tsv,series-*.json}
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
state="${CFB_STATE:-$HOME/.cfb-state}"
day="${CFB_DAY:-2026-10-28}"
pool="$1" c="$2" cku="$3" ma="${4:-12}" mb="${5:-8}" mc="${6:-8}"
per="${GEN_PER_CKU:-8}" readers="${READERS_PER_CKU:-2}"
f="$here/flink.sh"; fs="$here/flink-setup.sh"; short="${c#loams-cfb-}"
out="$here/results/$day/dedicated"; mkdir -p "$out"
id="$(jq -r .id "$state/clusters/$c/cluster.json")"
parts=$(( 24 * cku ))
phase() { printf '%s\t%s\n' "$(date -u +%FT%TZ)" "$1" >>"$out/$c-phases.tsv"; echo "$(date -u +%T) $c phase $1"; }
wait_done() { local i s; for i in $(seq 1 40); do s="$("$f" status "$pool" "$1" 2>/dev/null | jq -r .phase 2>/dev/null || true)"; [[ "$s" == COMPLETED || "$s" == FAILED ]] && { echo "$1 $s"; return; }; sleep 3; done; echo "$1 timeout"; }
run_ddl() { "$f" run "$pool" "$c" "$1" "$2" >/dev/null; wait_done "$1"; }

start="$(date -u -d '-2 min' +%Y-%m-%dT%H:%M:00Z)"
# tables: bounded faker ids, a 1 KB sink, a 100 B sink, a tiny counts sink
"$fs" "$pool" "$c" tables "$parts"
run_ddl "cfb-ddl-small-$short" "CREATE TABLE IF NOT EXISTS cfb_small (val STRING) DISTRIBUTED INTO $parts BUCKETS WITH ('value.format' = 'raw', 'kafka.retention.time' = '30 min', 'kafka.retention.size' = '200 mb')"
run_ddl "cfb-ddl-counts-$short" "CREATE TABLE IF NOT EXISTS cfb_counts (w TIMESTAMP_LTZ(3), r INT, n BIGINT) DISTRIBUTED INTO 1 BUCKETS WITH ('kafka.retention.time' = '1 h')"

n=$(( per * cku ))
# A: 1 KB records
phase "A-start gen=$n size=1000"
"$fs" "$pool" "$c" start "$n" pa 1000 >/dev/null
sleep $(( ma * 60 ))
"$fs" "$pool" "$c" stop "$n" pa >/dev/null
phase "A-stop"
# B: 100 B records (request and record rate)
phase "B-start gen=$n size=100"
for i in $(seq 1 "$n"); do
  "$f" run "$pool" "$c" "cfb-push-$short-pb$i" "INSERT INTO cfb_small SELECT CONCAT(CAST(g.id AS STRING), '-', CAST(t.x AS STRING), REPEAT('y', 80)) FROM cfb_ids AS g CROSS JOIN UNNEST(ARRAY[$(seq -s, 1 3000)]) AS t(x)" >/dev/null
done
sleep $(( mb * 60 ))
for i in $(seq 1 "$n"); do "$f" stop "$pool" "cfb-push-$short-pb$i" >/dev/null; done
phase "B-stop"
# C: consume the 1 KB topic from the earliest offset with parallel readers
r=$(( readers * cku ))
phase "C-start readers=$r"
for i in $(seq 1 "$r"); do
  "$f" run "$pool" "$c" "cfb-read-$short-rc$i" "INSERT INTO cfb_counts SELECT window_start, $i, COUNT(*) FROM TABLE(TUMBLE(TABLE cfb_sink /*+ OPTIONS('scan.startup.mode' = 'earliest-offset') */, DESCRIPTOR(\$rowtime), INTERVAL '10' SECOND)) GROUP BY window_start, window_end" >/dev/null
done
sleep $(( mc * 60 ))
for i in $(seq 1 "$r"); do "$f" stop "$pool" "cfb-read-$short-rc$i" >/dev/null; done
phase "C-stop"
sleep 180  # Metrics API lag
"$here/collect.sh" series "$id" "$start" "$(date -u +%Y-%m-%dT%H:%M:00Z)" PT1M "$out/$c-series-$(date -u +%H%M).json"
phase "done"
