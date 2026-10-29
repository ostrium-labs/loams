#!/usr/bin/env bash
# The servers of the BQ1 combined run that bench/all starts itself, each
# pinned to its own cores from budget.sh and capped in memory:
#
#   engines.sh up      start them and wait until every one answers
#   engines.sh down    stop what `up` started (by PID and container name)
#   engines.sh status  one line per server
#
# What it starts (ports on 127.0.0.1):
#   sql      PD 39479, one TiKV 40260 (API v2, keyspaces), one keyspace-mode
#            TiDB 44100 (keyspace "bq1all"), images pinned in
#            release/sqldb-images.toml.
#   kv       a second cluster for the kv engine's TiKV op: PD 39579, one
#            TiKV 40360, on its own core, so sql and kv do not share a store.
#   pg       the pinned Postgres 17.10 image (stock compute, PGDATA on a
#            tmpfs), port 55701
#   durable  the Resonate server on SQLite, port 38101
#   gateway  loams-gateway, admin HTTP listener (h2c and HTTP/1.1) 19387
#   iggy     iggy-server, TCP 48190 (HTTP 43100, QUIC 48180, WS 48192), one
#            shard, user iggy (a bench-only password)
#
# The House and the in-process engines (kv memory store, graph, meter,
# git) start inside their own native-load.sh.
#
# Data: $BQ1_ALL_DATA (default /mnt/Projects/bench-data/bq1-all), removed by
# `down`. Binaries: $BQ1_ALL_BIN (default $BQ1_ALL_DATA/bin).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
# shellcheck source=budget.sh
source "$here/budget.sh"
DATA=${BQ1_ALL_DATA:-/mnt/Projects/bench-data/bq1-all}
BIN=${BQ1_ALL_BIN:-$DATA/bin}
run="$DATA/run"
prefix=bq1all
PG_IMAGE=docker.io/library/postgres:17.10@sha256:7958605b474b3d264a969cb3a123d6aa00ad1e1fe9da8a69984dabb704d93317
mkdir -p "$run"

log() { printf '[engines %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

image_ref() {
  awk -v want="[$1]" '
    /^\[/ { sec = $0 }
    sec == want && $1 == "image"  { gsub(/"/, "", $3); img = $3 }
    sec == want && $1 == "digest" { gsub(/"/, "", $3); dig = $3 }
    END { if (img == "" || dig == "") exit 1; print img "@" dig }
  ' "$root/release/sqldb-images.toml"
}

wait_for() { # <what> <seconds> <cmd...>
  local what=$1 secs=$2; shift 2
  for _ in $(seq $((secs * 4))); do "$@" >/dev/null 2>&1 && return 0; sleep 0.25; done
  log "timed out waiting for $what"; return 1
}

port_open() { (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; }

start_bg() { # <name> <cores> <cmd...>: a host process, PID in $run/<name>.pid
  local name=$1 cores=$2; shift 2
  setsid taskset -c "$cores" "$@" > "$run/$name.log" 2>&1 < /dev/null &
  echo $! > "$run/$name.pid"
}

# pd_tikv <name> <pd cores> <tikv cores> <pd port> <tikv port>: one PD and one
# TiKV store (API v2, keyspaces) in $DATA/<name>, their own cluster.
pd_tikv() {
  local name=$1 pd_cores=$2 tikv_cores=$3 pd=$4 kv=$5
  local d="$DATA/$name"
  mkdir -p "$d/pd" "$d/tikv"
  cat > "$d/pd.toml" <<'EOF'
[keyspace]
pre-alloc = []
[replication]
max-replicas = 1
EOF
  cat > "$d/tikv.toml" <<EOF
memory-usage-limit = "$TIKV_MEM_LIMIT"
[server]
grpc-concurrency = 2
[readpool.unified]
max-thread-count = 2
[storage]
api-version = 2
enable-ttl = true
[storage.block-cache]
capacity = "$TIKV_BLOCK_CACHE"
[raftstore]
store-pool-size = 1
apply-pool-size = 1
[rocksdb]
max-open-files = 4096
max-background-jobs = 2
[raftdb]
max-open-files = 4096
max-background-jobs = 1
EOF
  log "$name: PD on $pd_cores, TiKV on $tikv_cores"
  podman run -d --name "$prefix-$name-pd" --network host --cpuset-cpus "$pd_cores" --memory "$PD_MEM" \
    -v "$d:/data:Z" "$(image_ref pd)" \
    --name=pd --data-dir=/data/pd --config=/data/pd.toml \
    --client-urls="http://127.0.0.1:$pd" --advertise-client-urls="http://127.0.0.1:$pd" \
    --peer-urls="http://127.0.0.1:$((pd + 1))" --advertise-peer-urls="http://127.0.0.1:$((pd + 1))" \
    --initial-cluster="pd=http://127.0.0.1:$((pd + 1))" > /dev/null
  wait_for "$name PD" 60 curl -fsS "http://127.0.0.1:$pd/pd/api/v1/health"
  podman run -d --name "$prefix-$name-tikv" --network host --cpuset-cpus "$tikv_cores" --memory "$TIKV_MEM" \
    --ulimit nofile=65536:65536 -v "$d:/data:Z" "$(image_ref tikv)" \
    --pd-endpoints="127.0.0.1:$pd" --addr="127.0.0.1:$kv" --advertise-addr="127.0.0.1:$kv" \
    --status-addr="127.0.0.1:$((kv + 20))" --data-dir=/data/tikv --config=/data/tikv.toml > /dev/null
  wait_for "$name TiKV Up" 300 bash -c \
    "curl -fsS http://127.0.0.1:$pd/pd/api/v1/stores | jq -e '[.stores[] | select(.store.state_name == \"Up\")] | length == 1'"
}

up_sql() {
  pd_tikv sql "$PD_CORES" "$TIKV_CORES" 39479 40260
  curl -fsS -X POST http://127.0.0.1:39479/pd/api/v2/keyspaces -H 'content-type: application/json' \
    -d '{"name":"bq1all"}' > /dev/null || true
  cat > "$DATA/sql/tidb.toml" <<'EOF'
keyspace-name = "bq1all"
split-table = true
[performance]
force-init-stats = false
lite-init-stats = true
[log]
level = "error"
EOF
  log "TiDB on $TIDB_CORES"
  podman run -d --name "$prefix-tidb" --network host --cpuset-cpus "$TIDB_CORES" --memory "$TIDB_MEM" \
    -v "$DATA/sql/tidb.toml:/etc/tidb.toml:ro,Z" "$(image_ref tidb)" \
    --store=tikv --path=127.0.0.1:39479 --host=127.0.0.1 -P 44100 --status=45100 \
    --config=/etc/tidb.toml > /dev/null
  wait_for TiDB 300 mysql --skip-ssl -h127.0.0.1 -P44100 -uroot -N -B -e 'SELECT 1'
}

# The kv engine's own TiKV cluster, so its store core is not sql's.
up_kv() {
  pd_tikv kv "$KV_PD_CORES" "$KV_TIKV_CORES" 39579 40360
}

up_pg() {
  log "Postgres on $PG_CORES"
  podman run -d --name "$prefix-pg" --network host --cpuset-cpus "$PG_CORES" --memory "$PG_MEM" \
    --tmpfs /var/lib/postgresql/data:rw,size=1g,mode=0700 -e POSTGRES_PASSWORD=bq1 "$PG_IMAGE" \
    -c port=55701 -c listen_addresses=127.0.0.1 -c max_connections=200 \
    -c shared_buffers=256MB -c synchronous_commit=off > /dev/null
  # The image's init runs the server once and restarts it; wait for the second.
  wait_for Postgres 120 bash -c \
    "podman logs $prefix-pg 2>&1 | grep -q 'PostgreSQL init process complete' && podman exec $prefix-pg pg_isready -q -h 127.0.0.1 -p 55701"
}

up_durable() {
  log "Resonate on $RESONATE_CORES"
  rm -f "$DATA/resonate.db"*
  RESONATE_SERVERS__ACTIVE=server_sqlite RESONATE_SERVERS__SERVER_SQLITE__PATH="$DATA/resonate.db" \
    RESONATE_GATEWAYS__GATEWAY_HTTP__BIND=127.0.0.1:38101 \
    RESONATE_GATEWAYS__GATEWAY_METRICS__BIND=127.0.0.1:38102 \
    start_bg resonate "$RESONATE_CORES" "$BIN/resonate" serve
  wait_for Resonate 30 curl -fs http://127.0.0.1:38101/ready
}

up_gateway() {
  log "loams-gateway on $GATEWAY_CORES"
  printf '[pg]\n' > "$run/routes.toml"
  cat > "$run/gateway.toml" <<TOML
[admin]
listen = "127.0.0.1:19387"
[routes]
path = "$run/routes.toml"
require_generation = false
[pg]
listen = "127.0.0.1:19388"
tls_mode = "passthrough"
TOML
  start_bg gateway "$GATEWAY_CORES" "$BIN/loams-gateway" --config "$run/gateway.toml"
  wait_for gateway 30 port_open 19387
}

up_iggy() {
  log "iggy-server on $IGGY_CORES"
  rm -rf "$DATA/iggy"
  IGGY_ROOT_USERNAME=iggy IGGY_ROOT_PASSWORD=bq1-all IGGY_PATH="$DATA/iggy" \
    IGGY_TCP_ADDRESS=127.0.0.1:48190 IGGY_HTTP_ADDRESS=127.0.0.1:43100 \
    IGGY_QUIC_ADDRESS=127.0.0.1:48180 IGGY_WEBSOCKET_ADDRESS=127.0.0.1:48192 \
    IGGY_SHARDING_CPU_ALLOCATION=1 \
    start_bg iggy "$IGGY_CORES" "$BIN/iggy-server"
  wait_for iggy 60 port_open 48190
}

down() {
  local names
  names="$(podman ps -a --format '{{.Names}}' | grep "^$prefix-" || true)"
  # shellcheck disable=SC2086
  [ -z "$names" ] || podman rm -f -t 5 $names > /dev/null
  for f in "$run"/*.pid; do
    [ -e "$f" ] || continue
    local pid; pid=$(cat "$f")
    kill "$pid" 2> /dev/null || true
    for _ in $(seq 40); do kill -0 "$pid" 2> /dev/null || break; sleep 0.25; done
    kill -9 "$pid" 2> /dev/null || true
    rm -f "$f"
  done
  rm -rf "$DATA/sql" "$DATA/kv" "$DATA/iggy" "$DATA/resonate.db"* "$run"
  log "down, data removed"
}

status() {
  podman ps --format '{{.Names}} {{.Status}}' | grep "^$prefix-" || true
  for f in "$run"/*.pid; do
    [ -e "$f" ] || continue
    local pid; pid=$(cat "$f")
    if kill -0 "$pid" 2> /dev/null; then echo "$(basename "$f" .pid) pid $pid up"; else echo "$(basename "$f" .pid) dead"; fi
  done
}

case "${1:-}" in
  up)
    trap 'log "up failed; stopping"; down' ERR
    up_pg; up_durable; up_gateway; up_iggy; up_sql; up_kv
    trap - ERR
    log "all servers up" ;;
  down) down ;;
  status) status ;;
  *) echo "usage: engines.sh up|down|status" >&2; exit 2 ;;
esac
