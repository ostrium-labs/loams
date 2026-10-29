#!/usr/bin/env bash
# BQ1-ALL's entry point for Loams SQL: brings up a one-store cluster on the
# given cores, loads a small data set, drives the native APIs, prints one
# JSON line per operation on stdout, and tears the cluster down.
#
#   bench/sql/native-load.sh --duration 30 --cores 0-5 --depth 16 [--ops point,raw,txn]
#
#   --duration S   measured seconds per operation (default 30)
#   --cores LIST   cpuset for everything (servers and load generator), at
#                  least 4 cores: PD gets the first, then TiKV about a
#                  quarter, TiDB about two fifths, the load generator the rest
#   --depth N      keys per request: 1 = a point select / Get; N > 1 = one
#                  batched point get (WHERE id IN (N)) / BatchGet (default 1)
#   --ops LIST     point (MySQL wire to TiDB, the headline), raw (TiKV raw
#                  API), txn (TiKV transactional snapshot reads); run one
#                  after the other (default: point)
#
# Output, one line per op:
#   {"engine","op","qps","requests_per_s","p50_ms","p99_ms","p999_ms","depth","cores","errors"}
# qps counts keys read (rows for SQL), requests_per_s counts round trips.
# The caller holds the bench lock; this script does not take it.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export BENCH_DATA="${BENCH_DATA:-/mnt/Projects/bench-data/bq1-sql}"
loadgen="${LOADGEN:-$BENCH_DATA/loadgen}"
tikvload="${TIKVLOAD:-$BENCH_DATA/tikvload}"
duration=30 cores="" depth=1 ops=point threads=64
while (( $# )); do
  case "$1" in
    --duration) duration=$2; shift 2 ;;
    --cores) cores=$2; shift 2 ;;
    --depth) depth=$2; shift 2 ;;
    --ops) ops=$2; shift 2 ;;
    --threads) threads=$2; shift 2 ;;
    *) echo "native-load.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
[[ -n "$cores" ]] || { echo "native-load.sh: --cores is required" >&2; exit 2; }
for b in "$loadgen" "$tikvload"; do
  [[ -x "$b" ]] || { echo "native-load.sh: build $b first (see bench/sql/README.md)" >&2; exit 2; }
done

# Expand the cpuset into a list of cores.
list=()
IFS=, read -r -a parts <<< "$cores"
for p in "${parts[@]}"; do
  if [[ "$p" == *-* ]]; then for ((c = ${p%-*}; c <= ${p#*-}; c++)); do list+=("$c"); done
  else list+=("$p"); fi
done
k=${#list[@]}
(( k >= 4 )) || { echo "native-load.sh: need at least 4 cores, got $k" >&2; exit 2; }
rem=$((k - 1))
ntikv=$(( rem / 4 > 0 ? rem / 4 : 1 ))
ntidb=$(( rem * 2 / 5 > 0 ? rem * 2 / 5 : 1 ))
(( rem - ntikv - ntidb >= 1 )) || ntidb=$((ntidb - 1))
join() { local IFS=,; echo "$*"; }
export NTIKV=1 NTIDB=1 PD_CPUS="${list[0]}"
TIKV_CPUS="$(join "${list[@]:1:ntikv}")"
TIDB_CPUS="$(join "${list[@]:1+ntikv:ntidb}")"
lg="$(join "${list[@]:1+ntikv+ntidb}")"
export TIKV_CPUS TIDB_CPUS
echo "native-load: PD $PD_CPUS, TiKV $TIKV_CPUS, TiDB $TIDB_CPUS, load generator $lg" >&2

trap '"$here/cluster.sh" down >&2 || true' EXIT
"$here/cluster.sh" down >/dev/null 2>&1 || true
"$here/cluster.sh" up >&2

line() { # <engine> <result file>
  jq -c --arg e "$1" --argjson cores "$k" \
    '{engine: $e, op: (.op // (if .depth > 1 then "batch_point_select" else "point_select" end)),
      qps: .ops_per_s, requests_per_s, p50_ms: (.latency_us.p50 / 1000),
      p99_ms: (.latency_us.p99 / 1000), p999_ms: (.latency_us.p999 / 1000),
      depth, cores: $cores, errors}' "$2"
}

res="$BENCH_DATA/native-load.json"
IFS=, read -r -a want <<< "$ops"
for op in "${want[@]}"; do
  case "$op" in
    point)
      taskset -c "$lg" "$loadgen" -mode prepare -addrs 127.0.0.1:44000 -tables 10 -rows 20000 -split 4 >&2
      taskset -c "$lg" "$loadgen" -mode point -addrs 127.0.0.1:44000 -tables 10 -rows 20000 \
        -threads "$threads" -depth "$depth" -warmup 5s -duration "${duration}s" -out "$res" >/dev/null
      line sql "$res" ;;
    raw|txn)
      curl -fsS -X POST http://127.0.0.1:39379/pd/api/v2/keyspaces -H 'content-type: application/json' \
        -d "{\"name\":\"bq1$op\"}" >/dev/null
      taskset -c "$lg" "$tikvload" -mode load -api "$op" -keyspace "bq1$op" -keys 200000 >&2
      taskset -c "$lg" "$tikvload" -mode get -api "$op" -keyspace "bq1$op" -keys 200000 \
        -threads "$threads" -depth "$depth" -warmup 5s -duration "${duration}s" -out "$res" >/dev/null
      line tikv "$res" ;;
    *) echo "native-load.sh: unknown op $op" >&2; exit 2 ;;
  esac
done
rm -f "$res"
