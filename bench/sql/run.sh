#!/usr/bin/env bash
# One BQ1-sql scenario end to end, in one bench-lock hold (< 15 minutes):
# quiet window, cluster up, data load, measurements, cluster down.
#
#   .he0-shared/benchslot.sh bq1-sql bench/sql/run.sh single
#   .he0-shared/benchslot.sh bq1-sql bench/sql/run.sh single-tikv
#   .he0-shared/benchslot.sh bq1-sql bench/sql/run.sh units-2
#
# Scenarios (14-core host; servers and load generator on disjoint cores):
#   single       PD core 0, one TiKV on cores 1-2, one TiDB on 3-5 (6 server
#                cores), load generator on 6-11. MySQL wire to TiDB: point
#                selects at depth 1 (several concurrencies) and depth 16
#                (a batched point get), a TiDB CPU profile, and the oltp mix
#                with TSO batching off and on (tidb_tso_client_batch_max_wait_time).
#   single-tikv  PD core 0, one TiKV on cores 1-2, no TiDB; TiKV's native
#                raw and transactional APIs at depth 1 and 16 (client-go).
#   units-N      N = 1, 2, 4 scaling units of one TiDB core + one TiKV core,
#                PD on core 0, load generator on 9-13; point selects at
#                depth 1 and 16, and the oltp mix.
#
# Every result records the load average, iowait and the servers' CPU-seconds
# over the measured window, and QPS per busy server core. Results:
# bench/results/bq1/sql/<scenario>-<run>.json
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
shared="$(cd "$root/.." && pwd)/.he0-shared"
export BENCH_DATA="${BENCH_DATA:-/mnt/Projects/bench-data/bq1-sql}"
loadgen="${LOADGEN:-$BENCH_DATA/loadgen}"
tikvload="${TIKVLOAD:-$BENCH_DATA/tikvload}"
out="$root/bench/results/bq1/sql"
scenario="${1:?usage: run.sh single|single-tikv|units-1|units-2|units-4}"
TABLES="${TABLES:-10}" ROWS="${ROWS:-50000}" TIKV_KEYS="${TIKV_KEYS:-500000}"
WARMUP="${WARMUP:-5}" DURATION="${DURATION:-20}"
DEPTH="${DEPTH:-16}"
mkdir -p "$out"

sql_runs=1 tikv_runs=0
case "$scenario" in
  single)
    export NTIKV=1 NTIDB=1 PD_CPUS=0 TIKV_CPUS="1-2" TIDB_CPUS="3-5"
    LG_CPUS=6-11 POINT_THREADS="32 64 128" BATCH_THREADS=64 OLTP_THREADS="16 32" PROFILE=1 TSO_BATCH=1 ;;
  single-tikv)
    export NTIKV=1 NTIDB=0 PD_CPUS=0 TIKV_CPUS="1-2" TIDB_CPUS=""
    LG_CPUS=6-11 sql_runs=0 tikv_runs=1 ;;
  units-1)
    export NTIKV=1 NTIDB=1 PD_CPUS=0 TIKV_CPUS="1" TIDB_CPUS="2"
    LG_CPUS=9-13 POINT_THREADS="32" BATCH_THREADS=32 OLTP_THREADS="16" ;;
  units-2)
    export NTIKV=2 NTIDB=2 PD_CPUS=0 TIKV_CPUS="1 2" TIDB_CPUS="3 4"
    LG_CPUS=9-13 POINT_THREADS="64" BATCH_THREADS=64 OLTP_THREADS="32" ;;
  units-4)
    export NTIKV=4 NTIDB=4 PD_CPUS=0 TIKV_CPUS="1 2 3 4" TIDB_CPUS="5 6 7 8"
    export TIKV_MEM=1000m TIDB_MEM=800m TIKV_MEM_LIMIT=850MB TIKV_BLOCK_CACHE=256MB
    LG_CPUS=9-13 POINT_THREADS="128" BATCH_THREADS=128 OLTP_THREADS="64" ;;
  *) echo "unknown scenario $scenario" >&2; exit 2 ;;
esac
POINT_THREADS="${POINT_THREADS_OVERRIDE:-${POINT_THREADS:-}}"

log() { printf '[%s] %s\n' "$(date +%T)" "$*" >&2; }

# ---- quiet window (bench-1b-qps.md): no new builds, load average < 2 ------
quiet_file="$shared/quiet"
quiet=false
quiet_begin() {
  echo $(( $(date +%s) + 1200 )) > "$quiet_file"
  for _ in $(seq 60); do
    if ! pgrep -x rustc >/dev/null && awk '{ exit !($1 < 2) }' /proc/loadavg; then
      quiet=true; return
    fi
    sleep 5
  done
  log "not quiet after 5 minutes (load $(cut -d' ' -f1 /proc/loadavg)); results are flagged quiet=false"
}

cleanup() {
  rm -f "$quiet_file"
  "$here/cluster.sh" down || true
}
trap cleanup EXIT

# ---- per-window host and server counters -----------------------------------
cgroups=()
find_cgroups() {
  cgroups=()
  local id
  for id in $(podman ps -q --no-trunc --filter name='^bq1sql-'); do
    cgroups+=("$(find /sys/fs/cgroup -maxdepth 8 -type d -name "libpod-$id.scope" -print -quit)")
  done
}
server_usec() {
  local sum=0 g v
  for g in "${cgroups[@]}"; do
    v="$(awk '/^usage_usec/ { print $2 }' "$g/cpu.stat" 2>/dev/null || echo 0)"
    sum=$((sum + v))
  done
  echo "$sum"
}
cpu_ticks() { awk '/^cpu / { print $2+$3+$4+$5+$6+$7+$8, $6 }' /proc/stat; }

# window <file>: samples counters over the measured window of a run that
# started now (warm-up WARMUP s, window DURATION s) into <file>.
window() {
  local file=$1
  sleep "$WARMUP"
  local u0 t0 w0
  u0="$(server_usec)"; read -r t0 w0 < <(cpu_ticks)
  sleep $(( DURATION / 2 ))
  podman stats --no-stream --format '{{.Name}} {{.CPUPerc}} {{.MemUsage}}' | grep '^bq1sql-' > "$file.stats" || true
  sleep $(( DURATION - DURATION / 2 ))
  local u1 t1 w1
  u1="$(server_usec)"; read -r t1 w1 < <(cpu_ticks)
  jq -n --argjson us $((u1 - u0)) --argjson dt $((t1 - t0)) --argjson dw $((w1 - w0)) \
    --arg load "$(cut -d' ' -f1-3 /proc/loadavg)" --argjson d "$DURATION" \
    '{server_cpu_s: ($us / 1e6), server_busy_cores: ($us / 1e6 / $d),
      iowait_pct: (if $dt > 0 then 100 * $dw / $dt else 0 end), loadavg: $load}' > "$file.win"
}

# finish <file> <ops/s field>: merges the window counters and the topology
# into the result.
finish() {
  local file=$1 ops=$2
  jq --arg sc "$scenario" --arg lg "$LG_CPUS" --arg pd "$PD_CPUS" --arg tk "$TIKV_CPUS" \
     --arg td "$TIDB_CPUS" --argjson leaders "${counts:-[]}" --argjson quiet "$quiet" \
     --slurpfile win "$file.win" --rawfile stats "$file.stats" \
     '. + {scenario: $sc, quiet: $quiet, cpus: {loadgen: $lg, pd: $pd, tikv: $tk, tidb: $td},
           leaders_per_store: $leaders, host: $win[0],
           container_stats: ($stats | split("\n") | map(select(length > 0)))}
        | .ops_per_busy_core = (if .host.server_busy_cores > 0
                                then .'"$ops"' / .host.server_busy_cores else null end)' \
     "$file.tmp" > "$file"
  rm -f "$file.tmp" "$file.win" "$file.stats"
  jq -c '{scenario, mode: (.mode // .op), threads, depth, requests_per_s, ops_per_s, latency_us,
          errors, ops_per_busy_core, host, rates: .metric_rates_per_s}' "$file" >&2
}

# ---- cluster and data ------------------------------------------------------
quiet_begin
"$here/cluster.sh" down >/dev/null 2>&1 || true
"$here/cluster.sh" up
find_cgroups

addrs="" metrics=""
# TSO: PD-side TSO stream messages (one per client batch); per TiDB the
# timestamps its sessions waited for, the batched TSO RPCs it sent, and its
# KV RPC batching (BatchCommands requests per send).
metrics="http://127.0.0.1:39379/metrics|grpc_server_msg_received_total{grpc_method=\"Tso\""
for ((j = 0; j < NTIDB; j++)); do
  s="http://127.0.0.1:$((45000 + j))/metrics"
  addrs+="${addrs:+,}127.0.0.1:$((44000 + j))"
  metrics+=",$s|pd_client_request_handle_requests_duration_seconds_count{type=\"tso\"}"
  metrics+=",$s|pd_client_cmd_handle_cmds_duration_seconds_count{type=\"wait\"}"
  metrics+=",$s|tidb_tikvclient_batch_requests_sum,$s|tidb_tikvclient_batch_requests_count"
done

counts="[]"
if (( sql_runs )); then
  split=$((NTIKV * 4))
  log "loading $TABLES x $ROWS rows, $split regions per table"
  t0=$(date +%s)
  taskset -c "$LG_CPUS" "$loadgen" -mode prepare -addrs "${addrs%%,*}" \
    -tables "$TABLES" -rows "$ROWS" -split "$split"
  log "loaded in $(( $(date +%s) - t0 )) s"
  # Wait until every store leads a fair share of the regions (PD balances).
  for _ in $(seq 120); do
    counts="$(curl -fsS http://127.0.0.1:39379/pd/api/v1/stores | jq -c '[.stores[].status.leader_count // 0]')"
    if jq -e --argjson n "$NTIKV" 'length == $n and (min * 10 >= max * 7)' <<< "$counts" >/dev/null; then break; fi
    sleep 2
  done
  log "leaders per store: $counts"
fi

# sql <name> <mode> <threads> <depth>
sql() {
  local mode=$2 threads=$3 depth=$4 file="$out/$scenario-$1.json"
  window "$file" &
  local w=$!
  taskset -c "$LG_CPUS" "$loadgen" -mode "$mode" -addrs "$addrs" -threads "$threads" \
    -depth "$depth" -tables "$TABLES" -rows "$ROWS" -warmup "${WARMUP}s" \
    -duration "${DURATION}s" -metrics "$metrics" -label "$scenario" -out "$file.tmp" >/dev/null
  wait "$w"
  finish "$file" ops_per_s
}

# tikv <name> <api> <threads> <depth>
tikv() {
  local api=$2 threads=$3 depth=$4 file="$out/$scenario-$1.json"
  window "$file" &
  local w=$!
  taskset -c "$LG_CPUS" "$tikvload" -mode get -api "$api" -keyspace "bq1$api" -keys "$TIKV_KEYS" \
    -threads "$threads" -depth "$depth" -warmup "${WARMUP}s" -duration "${DURATION}s" \
    -out "$file.tmp" >/dev/null
  wait "$w"
  finish "$file" ops_per_s
}

# Hot-path CPU profile of TiDB 0 (Go pprof) under point-select load.
profile() {
  local t=$1 dir="$BENCH_DATA/profile-$scenario"
  mkdir -p "$dir"
  taskset -c "$LG_CPUS" "$loadgen" -mode point -addrs "$addrs" -threads "$t" \
    -tables "$TABLES" -rows "$ROWS" -warmup 3s -duration 15s -label profile >/dev/null &
  local lg=$!
  sleep 5
  curl -fsS "http://127.0.0.1:45000/debug/pprof/profile?seconds=10" -o "$dir/tidb.pprof" || true
  wait "$lg" || true
  [[ -s "$dir/tidb.pprof" ]] && GOTOOLCHAIN=local go tool pprof -top -nodecount=40 "$dir/tidb.pprof" \
    > "$out/$scenario-profile-tidb-top.txt" 2>/dev/null || true
  rm -rf "$dir"
}

set_global() { mariadb --skip-ssl -h127.0.0.1 -P44000 -uroot -e "SET GLOBAL $1"; }

if (( sql_runs )); then
  for t in $POINT_THREADS; do sql "point-d1-t$t" point "$t" 1; done
  sql "point-d$DEPTH-t$BATCH_THREADS" point "$BATCH_THREADS" "$DEPTH"
  [[ "${PROFILE:-0}" == 1 ]] && profile "$BATCH_THREADS"
  for t in $OLTP_THREADS; do sql "oltp-t$t" oltp "$t" 1; done
  if [[ "${TSO_BATCH:-0}" == 1 ]]; then
    t="${OLTP_THREADS##* }"
    set_global "tidb_tso_client_batch_max_wait_time = 2"
    sql "oltp-t$t-tsowait2ms" oltp "$t" 1
    set_global "tidb_tso_client_batch_max_wait_time = 0"
  fi
fi

if (( tikv_runs )); then
  for api in raw txn; do
    curl -fsS -X POST http://127.0.0.1:39379/pd/api/v2/keyspaces -H 'content-type: application/json' \
      -d "{\"name\":\"bq1$api\"}" >/dev/null
    log "loading $TIKV_KEYS $api keys"
    taskset -c "$LG_CPUS" "$tikvload" -mode load -api "$api" -keyspace "bq1$api" -keys "$TIKV_KEYS"
  done
  for api in raw txn; do
    tikv "$api-d1-t64" "$api" 64 1
    tikv "$api-d$DEPTH-t64" "$api" 64 "$DEPTH"
  done
fi
log "$scenario done"
