#!/usr/bin/env bash
# BQ1-pg's native load for lane BQ1-ALL's combined run (owner, 2026-10-28:
# native APIs only). Postgres's native API is the PG wire protocol, straight
# to the Loams Postgres compute: no gateway, no TLS, loopback in the
# compute's network namespace.
#
#   bench/pg/native-load.sh --duration 60 --cores 0-5 --depth 16
#
#   --duration S   measured seconds (default 60), after a 5 s warm-up
#   --cores LIST   this engine's whole CPU budget as a taskset list (default
#                  0-5). Split: pgbench gets a third (rounded up); with
#                  --target compute the storage tier (pageserver, safekeeper,
#                  broker) gets a sixth (at least one); the server the rest.
#   --depth N      1: pgbench -S, one SELECT per round trip; N > 1: N point
#                  SELECTs per libpq pipeline (one Sync)
#   --op OP        select (default) or tpcb (depth 1 only)
#   --clients C    pgbench connections (default 64 at depth 1, 16 above)
#   --target T     compute (default): deploy/loams-postgres-dev's compute-v17
#                  with its pageserver and safekeeper; stock: the pinned
#                  Postgres 17.10 image (no Loams storage)
#   --scale N      pgbench scale (default 10, which fits the compute's
#                  shared_buffers plus file cache)
#
# Prints one JSON line on stdout:
#   {"engine":"postgres","op":...,"qps":...,"p50_ms":...,"p99_ms":...,
#    "p999_ms":...,"depth":...,"cores":...,"errors":...}
# Progress goes to stderr. It takes no bench slot: the combined run holds it.
# Everything it starts (containers, its compose project bq1pg-native) is
# removed on exit.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)

duration=60 cores=0-5 depth=1 op=select clients= target=compute scale=10
while [ $# -gt 0 ]; do
  case $1 in
    --duration) duration=$2; shift 2 ;;
    --cores) cores=$2; shift 2 ;;
    --depth) depth=$2; shift 2 ;;
    --op) op=$2; shift 2 ;;
    --clients) clients=$2; shift 2 ;;
    --target) target=$2; shift 2 ;;
    --scale) scale=$2; shift 2 ;;
    -h|--help) sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "native-load.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
[[ "$depth" =~ ^[0-9]+$ && "$depth" -ge 1 ]] || { echo "native-load.sh: --depth must be >= 1" >&2; exit 2; }
[ "$op" = select ] || [ "$depth" = 1 ] || { echo "native-load.sh: --op tpcb takes --depth 1" >&2; exit 2; }
case $op in select | tpcb) ;; *) echo "native-load.sh: --op is select or tpcb" >&2; exit 2 ;; esac
case $target in compute | stock) ;; *) echo "native-load.sh: --target is compute or stock" >&2; exit 2 ;; esac
[ -n "$clients" ] || { [ "$depth" = 1 ] && clients=64 || clients=16; }

# Split the budget.
mapfile -t cpus < <(python3 -c '
import sys
out = []
for part in sys.argv[1].split(","):
    a, _, b = part.partition("-")
    out += range(int(a), int(b or a) + 1)
print("\n".join(map(str, out)))' "$cores")
n=${#cpus[@]}
[ "$n" -ge 3 ] || { echo "native-load.sh: --cores needs at least 3 CPUs" >&2; exit 2; }
nc=$(((n + 2) / 3))
ns=0
[ "$target" = compute ] && ns=$(((n + 5) / 6))
np=$((n - nc - ns))
join() { local IFS=,; echo "$*"; }
export PG_CORES CLIENT_CORES STORAGE_CORES GW_CORES
PG_CORES=$(join "${cpus[@]:0:np}")
CLIENT_CORES=$(join "${cpus[@]:np:nc}")
if [ "$ns" -gt 0 ]; then STORAGE_CORES=$(join "${cpus[@]:np+nc:ns}"); else STORAGE_CORES=$PG_CORES; fi
GW_CORES=$CLIENT_CORES
export DURATION=$duration WARMUP=5 SCALE=$scale
export BQ1_DATA=${BQ1_DATA:-/mnt/Projects/bench-data/bq1-pg}/native-$$
# shellcheck source=bench/pg/lib.sh
. "$here/lib.sh"
BQ1_OUT=$BQ1_DATA/result.jsonl
: >"$BQ1_OUT"

dev=$BQ1_ROOT/deploy/loams-postgres-dev
dc() { podman compose -p bq1pg-native -f "$dev/compose.yaml" -f "$BQ1_HERE/compute.override.yaml" "$@"; }
cleanup() {
  if [ "$target" = compute ]; then dc down -v --remove-orphans >/dev/null 2>&1 || true; else pg_stop bq1pg-native; fi
  rm -rf "$BQ1_DATA"
}
trap cleanup EXIT

bq1_log "postgres native: target=$target server=$PG_CORES client=$CLIENT_CORES storage=$STORAGE_CORES depth=$depth op=$op clients=$clients"
if [ "$target" = compute ]; then
  dc down -v --remove-orphans >/dev/null 2>&1 || true
  dc up -d compute1 >/dev/null 2>&1
  cid=$(dc ps -q compute1)
  export BQ1_CLIENT_NET="--network=container:$cid" BQ1_USER=cloud_admin BQ1_PW=cloud_admin
  port=55433
  ok=
  for _ in $(seq 180); do
    if podman exec "$cid" sh -c "pg_isready -q -h 127.0.0.1 -p $port" 2>/dev/null; then ok=1; break; fi
    sleep 1
  done
  [ -n "$ok" ] || { dc logs --tail 40 compute1 >&2; exit 1; }
else
  port=$PG_PORT
  pg_start bq1pg-native "$PG_CORES" "$port"
fi
mkdir -p "$BQ1_DATA/run"; chmod 0777 "$BQ1_DATA/run"
pgbench_c PGHOST=127.0.0.1 PGPORT="$port" PGSSLMODE=disable -- -i -q -s "$scale" >/dev/null 2>&1

if [ "$op" = tpcb ]; then workload=tpcb; elif [ "$depth" = 1 ]; then workload=select-only; else workload=pipe$depth; fi
bench_run "native-$op-d$depth" plain "$port" "$workload" "$clients" \
  "{\"target\": \"$target\", \"scale\": $scale}"
python3 - "$BQ1_OUT" "$op" "$depth" "$n" <<'PY'
import json, sys
r = json.loads(open(sys.argv[1]).read().splitlines()[-1])
lat = r["latency_ms"]
op = sys.argv[2] if int(sys.argv[3]) == 1 else f"{sys.argv[2]}-pipeline"
print(json.dumps({
    "engine": "postgres", "op": op, "qps": r["qps"],
    # Latency of one round trip: one statement at depth 1, one pipeline of
    # `depth` statements above it.
    "p50_ms": lat["p50"], "p99_ms": lat["p99"], "p999_ms": lat["p99.9"],
    "depth": int(sys.argv[3]), "cores": int(sys.argv[4]), "errors": r["failed_tx"],
    "tps": r["tps"], "clients": r["clients"], "target": r.get("target"),
}))
PY
