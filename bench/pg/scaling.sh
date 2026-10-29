#!/usr/bin/env bash
# BQ1-pg hold 2, "scaling", three measurements:
#
#   pg-cores   one stock server on 1, 2 and 4 cores (select-only, plain):
#              per-core throughput and how it scales inside one instance
#   shards     1, 2 and 4 independent servers of one core each (Loams
#              Postgres scales out as independent computes, one per
#              namespace or branch): the sum of their throughput
#   gw-cores   the gateway on 1, 2 and 4 cores (select-only, gw-splice and
#              gw-terminate) in front of the 6-core server: its CPU per query
#
#   .he0-shared/benchslot.sh bq1-pg env GW_BIN=.../loams-gateway bench/pg/scaling.sh [pg-cores|shards|gw-cores]...
#
# Each part is under 10 minutes; run them in separate holds.
set -euo pipefail
# shellcheck source=bench/pg/lib.sh
. "$(dirname "$0")/lib.sh"

parts=("$@")
[ "${#parts[@]}" -gt 0 ] || parts=(pg-cores shards gw-cores)
names=()
cleanup() { gw_stop; for n in "${names[@]}"; do pg_stop "$n"; done; }
trap cleanup EXIT

init() { # init <port> <scale>
  local -a c
  read -r -a c <<<"$(conninfo plain "$1")"
  mkdir -p "$BQ1_DATA/run"
  pgbench_c "${c[@]}" -- -i -q -s "$2" >/dev/null 2>&1
}

pg_cores() {
  bq1_out scaling-pg-cores
  local cores
  for cores in 0 0-1 0-3; do
    names+=(bq1pg-scale)
    pg_start bq1pg-scale "$cores" "$PG_PORT"
    init "$PG_PORT" "$SCALE"
    PG_CORES=$cores bench_run "pg-cores-$(count_cores "$cores")" plain "$PG_PORT" select-only 64 \
      "{\"pg_cores_n\": $(count_cores "$cores")}"
    pg_stop bq1pg-scale
  done
}

# shards: N one-core servers on cores 0..N-1, ports PG_PORT+i, scale 10
# each; one pgbench per server, all on CLIENT_CORES, started together.
shards() {
  bq1_out scaling-shards
  local n i
  for n in 1 2 4; do
    for i in $(seq 0 $((n - 1))); do
      names+=("bq1pg-shard$i")
      pg_start "bq1pg-shard$i" "$i" $((PG_PORT + i))
      init $((PG_PORT + i)) 10
    done
    local per=$((64 / n)) pids=() t0 t1
    rm -rf "$BQ1_DATA/run"; mkdir -p "$BQ1_DATA/run"; chmod 0777 "$BQ1_DATA/run"
    local pg0 cl0
    pg0=$(cpu_snapshot "0-$((n - 1))"); cl0=$(cpu_snapshot "$CLIENT_CORES")
    t0=$(date +%s.%N)
    for i in $(seq 0 $((n - 1))); do
      local -a c
      read -r -a c <<<"$(conninfo plain $((PG_PORT + i)))"
      pgbench_c "${c[@]}" -- -n -M prepared -S -c "$per" -j "$(( $(count_cores "$CLIENT_CORES") / n > 0 ? $(count_cores "$CLIENT_CORES") / n : 1 ))" \
        -T "$DURATION" >"$BQ1_DATA/run/shard$i.txt" 2>&1 &
      pids+=($!)
    done
    for p in "${pids[@]}"; do wait "$p"; done
    t1=$(date +%s.%N)
    local pg1 cl1
    pg1=$(cpu_snapshot "0-$((n - 1))"); cl1=$(cpu_snapshot "$CLIENT_CORES")
    python3 - "$n" "$BQ1_DATA/run" "$DURATION" "$pg0" "$pg1" "$cl0" "$cl1" "$t0" "$t1" >>"$BQ1_OUT" <<'PY'
import glob, json, os, re, sys
n, d, dur = int(sys.argv[1]), sys.argv[2], float(sys.argv[3])
hz = os.sysconf("SC_CLK_TCK")
def busy(a, b):
    return (int(b.split()[0]) - int(a.split()[0])) / hz / dur
tps = []
for f in sorted(glob.glob(f"{d}/shard*.txt")):
    m = re.search(r"^tps = ([0-9.]+) \(without", open(f).read(), re.M)
    tps.append(float(m.group(1)) if m else 0.0)
pg = busy(sys.argv[4], sys.argv[5])
print(json.dumps({"label": f"shards-{n}", "mode": "plain", "workload": "select-only", "shards": n,
  "clients": 64, "duration_s": dur, "tps_per_shard": [round(t, 1) for t in tps],
  "tps": round(sum(tps), 1), "qps": round(sum(tps), 1),
  "cpu": {"pg": {"n": n, "busy_cores": round(pg, 3)}, "client": {"busy_cores": round(busy(sys.argv[6], sys.argv[7]), 3)}},
  "qps_per_pg_core": round(sum(tps) / pg, 1) if pg else None}, sort_keys=True))
PY
    tail -1 "$BQ1_OUT" | python3 -c 'import json,sys; r=json.loads(sys.stdin.read()); print(f"bq1-pg: shards={r[\"shards\"]} tps={r[\"tps\"]:.0f} per-shard={r[\"tps_per_shard\"]}", file=sys.stderr)'
    for i in $(seq 0 $((n - 1))); do pg_stop "bq1pg-shard$i"; done
    rm -rf "$BQ1_DATA/run"
  done
}

gw_cores() {
  bq1_out scaling-gw-cores
  names+=(bq1pg-gw)
  pg_start bq1pg-gw "$PG_CORES" "$PG_PORT"
  init "$PG_PORT" "$SCALE"
  local cores
  # 6, 6-7, then 6-7 plus the storage pair (free in these holds): 1, 2, 4.
  for cores in 6 6-7 6-7,12-13; do
    GW_CORES=$cores gw_start "$cores" "127.0.0.1:$PG_PORT"
    for mode in gw-splice gw-terminate; do
      GW_CORES=$cores bench_run "gw-cores-$(count_cores "$cores")" "$mode" "$PG_PORT" select-only 64 \
        "{\"gw_cores_n\": $(count_cores "$cores")}"
    done
    gw_stop
  done
}

for part in "${parts[@]}"; do
  case $part in
    pg-cores) pg_cores ;;
    shards) shards ;;
    gw-cores) gw_cores ;;
    *) bq1_log "unknown part $part"; exit 2 ;;
  esac
done
