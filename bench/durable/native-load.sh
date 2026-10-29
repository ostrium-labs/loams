#!/usr/bin/env bash
# Durable engines through their native APIs, for the combined run (BQ1-ALL):
#   bench/durable/native-load.sh --duration S --cores LIST --depth N
#
# Runs two legs at the same time, each on its own share of LIST (a taskset
# CPU list such as 0-5 or 2,3,8,9):
#   - Resonate: its native HTTP protocol, promise.create then promise.settle,
#     server on SQLite (or $RESONATE_PG_URL: Postgres) on the first quarter
#     of the cores, the load generator on the second quarter;
#   - the Temporal gateway's store API (loams-temporal-store), compare-and-set
#     commits as completed workflow tasks make them, on the second half: the
#     memory store, or Postgres when $LOAMS_TEMPORAL_PG_URL is set.
# --depth is the requests in flight per leg (concurrent requests over the
# connections: HTTP/1.1 keep-alive, one connection per caller, by default;
# --h2c multiplexes them as HTTP/2 streams on --conns connections).
#
# Writes one JSON line per leg to stdout:
#   {engine, op, qps, p50_ms, p99_ms, p999_ms, depth, cores, errors, ...}
# Binaries: $RS (resonate), $LG (bench/durable/loadgen), $STORE_LOAD
# (cargo build --release -p loams-temporal-store --example store_load).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
DATA=${DATA:-/mnt/Projects/bench-data/bq1-durable}
RS=${RS:-$DATA/bin/resonate}
LG=${LG:-$here/loadgen/bin/bq1-loadgen}
STORE_LOAD=${STORE_LOAD:-$DATA/bin/store_load}
duration=30 cores=0-5 depth=64 h2c=0 conns=0 port=${RESONATE_PORT:-38001}
while [ $# -gt 0 ]; do
  case "$1" in
    --duration) duration=${2%s}; shift 2 ;;
    --cores) cores=$2; shift 2 ;;
    --depth) depth=$2; shift 2 ;;
    --h2c) h2c=1; shift ;;
    --conns) conns=$2; shift 2 ;;
    *) echo "unknown flag $1" >&2; exit 2 ;;
  esac
done

# Expand a CPU list ("0-3,8,10-11") into one core per line.
expand() { tr ',' '\n' <<<"$1" | while IFS=- read -r a b; do seq "$a" "${b:-$a}"; done; }
mapfile -t all < <(expand "$cores")
n=${#all[@]}
[ "$n" -ge 3 ] || { echo "need at least 3 cores" >&2; exit 2; }
join() { local IFS=,; echo "$*"; }
q=$(( n / 4 )); [ "$q" -lt 1 ] && q=1
rs_cores=$(join "${all[@]:0:q}")
lg_cores=$(join "${all[@]:q:q}")
st_cores=$(join "${all[@]:$((2 * q))}")
st_n=$(( n - 2 * q ))

run=$DATA/native-$$
mkdir -p "$run"
pids=()
cleanup() { for p in "${pids[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done; wait 2>/dev/null || true; rm -rf "$run"; }
trap cleanup EXIT

if [ -n "${RESONATE_PG_URL:-}" ]; then
  backend=postgres
  store_env=(RESONATE_SERVERS__ACTIVE=server_postgres RESONATE_SERVERS__SERVER_POSTGRES__URL="$RESONATE_PG_URL" RESONATE_SERVERS__SERVER_POSTGRES__POOL_SIZE=32)
else
  backend=sqlite
  store_env=(RESONATE_SERVERS__ACTIVE=server_sqlite RESONATE_SERVERS__SERVER_SQLITE__PATH="$run/resonate.db")
fi
env "${store_env[@]}" RESONATE_GATEWAYS__GATEWAY_HTTP__BIND=127.0.0.1:$port \
  RESONATE_GATEWAYS__GATEWAY_METRICS__BIND=127.0.0.1:$((port + 1)) \
  taskset -c "$rs_cores" "$RS" serve > "$run/rs.log" 2>&1 &
pids+=($!)
for _ in $(seq 100); do curl -fs "http://127.0.0.1:$port/ready" >/dev/null && break; sleep 0.1; done

lg_flags=(-mode rcreate -resonate "http://127.0.0.1:$port/" -c "$depth" -d "${duration}s" -warmup 3s -label native)
[ "$h2c" = 1 ] && lg_flags+=(-h2c)
[ "$conns" != 0 ] && lg_flags+=(-http-conns "$conns")
taskset -c "$lg_cores" "$LG" "${lg_flags[@]}" > "$run/rs.json" 2> "$run/lg.log" &
lg_pid=$!

if [ -n "${LOAMS_TEMPORAL_PG_URL:-}" ]; then st_backend=(--backend postgres --url "$LOAMS_TEMPORAL_PG_URL"); else st_backend=(--backend memory); fi
taskset -c "$st_cores" "$STORE_LOAD" commit "${st_backend[@]}" --depth "$depth" --duration-s "$duration" \
  --warmup-s 3 --threads "$st_n" --cores "$st_cores" > "$run/store.json" 2> "$run/store.log" &
st_pid=$!
wait "$lg_pid" || true
wait "$st_pid" || true

python3 - "$run/rs.json" "$depth" "$rs_cores+$lg_cores" "$backend" "$h2c" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
r = d.get("rates_per_s", {})
print(json.dumps({
    "engine": "durable", "op": "resonate.promise.create+settle", "backend": sys.argv[4],
    "qps": d["ops_per_s"], "ops_per_s": r.get("creates", 0),
    "p50_ms": d["p50_ms"], "p99_ms": d["p99_ms"], "p999_ms": d["p999_ms"],
    "settle_p99_ms": r.get("settle_p99_ms"),
    "depth": int(sys.argv[2]), "cores": sys.argv[3], "errors": d["errors"],
    "transport": "h2c" if sys.argv[5] == "1" else "http/1.1",
}))
PY
cat "$run/store.json"
