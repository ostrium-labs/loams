#!/usr/bin/env bash
# BQ1-durable harness (docs/benchmarks/2026-10-28-1b-qps/durable.md).
#
# Run every leg under the exclusive bench lock:
#   .he0-shared/benchslot.sh bq1-durable bench/durable/run.sh <leg> ...
#
# Legs:
#   temporal-mem <mode> <instances> [loadgen flags...]
#       N loams-temporal gateways, memory store + in-process durability (the
#       frontend hot path without Postgres or Resonate), instance i on its
#       own server cores; one loadgen per instance on the loadgen cores.
#   temporal-pg <mode> [loadgen flags...]
#       one gateway on the Postgres store with a real Resonate server on
#       Postgres (the production shape), Postgres in podman.
#   resonate <sqlite|postgres> <servers> [loadgen flags...]
#       Resonate server(s), promise.create + promise.settle.
#
# Modes (loadgen): start, signal, activity, timer, rcreate.
#
# Cores (14 on the bench laptop): servers get SERVER_CORES (default 0-5),
# load generators LOADGEN_CORES (default 6-11); 12-13 are left to the OS.
# Results: one JSON per loadgen in $OUT (default bench/results/bq1/durable).
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
DATA=${DATA:-/mnt/Projects/bench-data/bq1-durable}
OUT=${OUT:-$repo/bench/results/bq1/durable}
LT=${LT:-$DATA/bin/loams-temporal}
RS=${RS:-$DATA/bin/resonate}
LG=${LG:-$here/loadgen/bin/bq1-loadgen}
SERVER_CORES=${SERVER_CORES:-0-5}
LOADGEN_CORES=${LOADGEN_CORES:-6-11}
PG_IMAGE=docker.io/library/postgres:17.10@sha256:7958605b474b3d264a969cb3a123d6aa00ad1e1fe9da8a69984dabb704d93317
PG_PORT=${PG_PORT:-35432}
PG_NAME=bq1-durable-pg
mkdir -p "$DATA/run" "$OUT"

pids=()
cleanup() {
  for p in "${pids[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
  wait 2>/dev/null || true
}
trap cleanup EXIT

# Expand "a-b" to a list of cores.
cores() { local a=${1%-*} b=${1#*-}; seq "$a" "$b"; }
# The i-th slice (of n) of a core range, as "x-y" or "x".
slice() {
  local -a all; mapfile -t all < <(cores "$1"); local n=$2 i=$3
  local per=$(( ${#all[@]} / n )); [ "$per" -lt 1 ] && per=1
  local from=${all[$((i * per))]} to=${all[$((i * per + per - 1))]}
  [ "$from" = "$to" ] && echo "$from" || echo "$from-$to"
}

stamp() { date -u +%Y%m%dT%H%M%SZ; }

pg_up() {
  if ! podman exec "$PG_NAME" pg_isready -U bq1 >/dev/null 2>&1; then
    podman rm -f "$PG_NAME" >/dev/null 2>&1 || true
    podman run -d --name "$PG_NAME" --cpuset-cpus "$SERVER_CORES" -p 127.0.0.1:$PG_PORT:5432 \
      -e POSTGRES_PASSWORD=bq1 -e POSTGRES_USER=bq1 "$PG_IMAGE" \
      -c max_connections=400 -c shared_buffers=1GB -c synchronous_commit=on >/dev/null
    for _ in $(seq 60); do podman exec "$PG_NAME" pg_isready -U bq1 >/dev/null 2>&1 && break; sleep 1; done
    sleep 2
  fi
  podman exec "$PG_NAME" psql -q -U bq1 -d postgres \
    -c 'drop database if exists gw' -c 'drop database if exists rs' \
    -c 'create database gw' -c 'create database rs' >/dev/null
}
pg_down() { podman rm -f "$PG_NAME" >/dev/null 2>&1 || true; }

wait_port() { for _ in $(seq 100); do (exec 3<>/dev/tcp/127.0.0.1/"$1") 2>/dev/null && return 0; sleep 0.1; done; echo "port $1 not up" >&2; return 1; }

# gateway <index> <store toml> <durability toml> <cores>
gateway() {
  local i=$1 port=$((27233 + $1)) cfg="$DATA/run/gw$1.toml"
  cat > "$cfg" <<TOML
listen = "127.0.0.1:$port"
insecure_dev = true
pid = "bq1-gw-$i"
dev_loams_namespace = "local"
[store]
$2
[durability]
$3
[[bootstrap_namespaces]]
loams_namespace_id = "local"
name = "default"
TOML
  GW_DB="postgres://bq1:bq1@127.0.0.1:$PG_PORT/gw" RUST_LOG=${GW_LOG:-warn} \
    taskset -c "$4" "$LT" --config "$cfg" > "$DATA/run/gw$i.log" 2>&1 &
  pids+=($!)
  wait_port "$port"
}

# resonate_server <index> <env...> on <cores>
resonate_server() {
  local i=$1 cores=$2; shift 2
  local port=$((28001 + 2 * i))
  env "$@" RESONATE_GATEWAYS__GATEWAY_HTTP__BIND=127.0.0.1:$port \
    RESONATE_GATEWAYS__GATEWAY_METRICS__BIND=127.0.0.1:$((port + 1)) \
    taskset -c "$cores" "$RS" serve > "$DATA/run/rs$i.log" 2>&1 &
  pids+=($!)
  for _ in $(seq 100); do curl -fs "http://127.0.0.1:$port/ready" >/dev/null && return 0; sleep 0.1; done
  echo "resonate $i not ready" >&2; return 1
}

# CPU seconds a process used (utime + stime), from /proc.
cpu_s() { awk '{print ($14 + $15) / 100}' "/proc/$1/stat" 2>/dev/null || echo 0; }

leg=${1:?leg}; shift
ts=$(stamp)
case "$leg" in
  temporal-mem)
    mode=${1:?mode}; n=${2:?instances}; shift 2
    for i in $(seq 0 $((n - 1))); do
      gateway "$i" 'kind = "memory"' 'kind = "in_process"' "$(slice "$SERVER_CORES" "$n" "$i")"
    done
    before=(); for p in "${pids[@]}"; do before+=("$(cpu_s "$p")"); done
    lg=()
    for i in $(seq 0 $((n - 1))); do
      taskset -c "$(slice "$LOADGEN_CORES" "$n" "$i")" "$LG" -mode "$mode" -addr 127.0.0.1:$((27233 + i)) \
        -label "temporal-mem n=$n i=$i" -run "bq1-$ts-$i" "$@" > "$OUT/temporal-mem-$mode-n$n-i$i-$ts.json" 2> "$DATA/run/lg$i.log" &
      lg+=($!)
    done
    for p in "${lg[@]}"; do wait "$p"; done
    k=0; for p in "${pids[@]}"; do echo "gateway $k cpu_s=$(awk -v a="$(cpu_s "$p")" -v b="${before[$k]}" 'BEGIN{print a-b}')"; k=$((k+1)); done
    ;;
  temporal-pg)
    mode=${1:?mode}; shift
    pg_up
    resonate_server 0 "$SERVER_CORES" RESONATE_SERVERS__ACTIVE=server_postgres \
      RESONATE_SERVERS__SERVER_POSTGRES__URL="postgres://bq1:bq1@127.0.0.1:$PG_PORT/rs" \
      RESONATE_SERVERS__SERVER_POSTGRES__POOL_SIZE=40
    gateway 0 'kind = "postgres"
url_env = "GW_DB"
[store.pool]
max_connections = 64' 'kind = "resonate"
url = "http://127.0.0.1:28001"' "$SERVER_CORES"
    taskset -c "$LOADGEN_CORES" "$LG" -mode "$mode" -addr 127.0.0.1:27233 -label "temporal-pg" \
      -run "bq1-$ts" "$@" > "$OUT/temporal-pg-$mode-$ts.json" 2> "$DATA/run/lg0.log"
    pg_down
    ;;
  resonate)
    backend=${1:?backend}; n=${2:?servers}; shift 2
    if [ "$backend" = postgres ]; then pg_up; fi
    for i in $(seq 0 $((n - 1))); do
      c=$(slice "$SERVER_CORES" "$n" "$i")
      if [ "$backend" = sqlite ]; then
        rm -f "$DATA/run/rs$i.db"*
        resonate_server "$i" "$c" RESONATE_SERVERS__ACTIVE=server_sqlite \
          RESONATE_SERVERS__SERVER_SQLITE__PATH="$DATA/run/rs$i.db"
      else
        resonate_server "$i" "$c" RESONATE_SERVERS__ACTIVE=server_postgres \
          RESONATE_SERVERS__SERVER_POSTGRES__URL="postgres://bq1:bq1@127.0.0.1:$PG_PORT/rs" \
          RESONATE_SERVERS__SERVER_POSTGRES__POOL_SIZE=$((40 / n))
      fi
    done
    lg=()
    for i in $(seq 0 $((n - 1))); do
      taskset -c "$(slice "$LOADGEN_CORES" "$n" "$i")" "$LG" -mode rcreate -resonate "http://127.0.0.1:$((28001 + 2 * i))/" \
        -label "resonate $backend n=$n i=$i" -run "bq1-$ts-$i" "$@" > "$OUT/resonate-$backend-n$n-i$i-$ts.json" 2> "$DATA/run/lg$i.log" &
      lg+=($!)
    done
    for p in "${lg[@]}"; do wait "$p"; done
    if [ "$backend" = postgres ]; then pg_down; fi
    rm -f "$DATA/run/"rs*.db*
    ;;
  *) echo "unknown leg $leg" >&2; exit 2 ;;
esac
