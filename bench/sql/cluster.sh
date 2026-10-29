#!/usr/bin/env bash
# A local Loams SQL cluster for BQ1-sql: one PD, N TiKV stores (API v2) and M
# keyspace-mode TiDB servers, each container pinned to its own cpuset, with
# the images pinned in release/sqldb-images.toml.
#
#   cluster.sh up      # NTIKV, NTIDB, PD_CPUS, TIKV_CPUS, TIDB_CPUS (below)
#   cluster.sh down    # removes this script's containers and data
#
# Environment:
#   NTIKV=1 NTIDB=1              store and TiDB counts
#   PD_CPUS=0                    PD's cpuset
#   TIKV_CPUS="1-2"              one cpuset per store, space-separated
#   TIDB_CPUS="3-5"              one cpuset per TiDB, space-separated
#   TIKV_MEM=1400m TIDB_MEM=1200m PD_MEM=768m   container memory limits
#   TIKV_MEM_LIMIT=1200MB TIKV_BLOCK_CACHE=384MB  TiKV's own memory settings
#   BENCH_DATA=/mnt/Projects/bench-data/bq1-sql
#
# Ports (host networking): PD 39379/39380; TiKV i 40160+i (status 40180+i);
# TiDB j 44000+j (status 45000+j). Replication is one replica per region
# (max-replicas = 1): the runs measure sharding, not Raft replication.
# Every TiDB serves keyspace "bq1" (D260: never a TiDB without one).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
data="${BENCH_DATA:-/mnt/Projects/bench-data/bq1-sql}/cluster"
prefix=bq1sql
NTIKV="${NTIKV:-1}" NTIDB="${NTIDB:-1}"
PD_CPUS="${PD_CPUS:-0}"
read -r -a tikv_cpus <<< "${TIKV_CPUS:-1-2}"
read -r -a tidb_cpus <<< "${TIDB_CPUS:-3-5}"
PD="127.0.0.1:39379"
KEYSPACE=bq1

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

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

stores_up() {
  [[ "$(curl -fsS "http://$PD/pd/api/v1/stores" | jq '[.stores[] | select(.store.state_name == "Up")] | length')" == "$NTIKV" ]]
}

tidb_ready() { mariadb --skip-ssl -h127.0.0.1 -P"$1" -uroot -N -B -e 'SELECT 1'; }

up() {
  (( ${#tikv_cpus[@]} >= NTIKV && ${#tidb_cpus[@]} >= NTIDB )) || {
    log "need one cpuset per store and per TiDB"; exit 1; }
  mkdir -p "$data/pd"
  cat > "$data/pd.toml" <<'EOF'
[keyspace]
pre-alloc = []
[replication]
max-replicas = 1
[schedule]
leader-schedule-limit = 16
region-schedule-limit = 64
EOF
  cat > "$data/tikv.toml" <<EOF
memory-usage-limit = "${TIKV_MEM_LIMIT:-1200MB}"
[storage]
api-version = 2
enable-ttl = true
[storage.block-cache]
capacity = "${TIKV_BLOCK_CACHE:-384MB}"
[rocksdb]
max-open-files = 4096
[raftdb]
max-open-files = 4096
EOF
  cat > "$data/tidb.toml" <<EOF
keyspace-name = "$KEYSPACE"
split-table = true
[performance]
force-init-stats = false
lite-init-stats = true
[log]
level = "error"
EOF
  log "PD on cpus $PD_CPUS"
  podman run -d --name "$prefix-pd" --network host --cpuset-cpus "$PD_CPUS" \
    --memory "${PD_MEM:-768m}" -v "$data:/data:Z" "$(image_ref pd)" \
    --name=pd --data-dir=/data/pd --config=/data/pd.toml \
    --client-urls="http://$PD" --advertise-client-urls="http://$PD" \
    --peer-urls=http://127.0.0.1:39380 --advertise-peer-urls=http://127.0.0.1:39380 \
    --initial-cluster=pd=http://127.0.0.1:39380 >/dev/null
  wait_for PD 60 curl -fsS "http://$PD/pd/api/v1/health"
  for ((i = 0; i < NTIKV; i++)); do
    mkdir -p "$data/tikv-$i"
    log "TiKV $i on cpus ${tikv_cpus[$i]}"
    podman run -d --name "$prefix-tikv-$i" --network host --cpuset-cpus "${tikv_cpus[$i]}" \
      --memory "${TIKV_MEM:-1400m}" --ulimit nofile=65536:65536 -v "$data:/data:Z" "$(image_ref tikv)" \
      --pd-endpoints="$PD" --addr="127.0.0.1:$((40160 + i))" \
      --advertise-addr="127.0.0.1:$((40160 + i))" --status-addr="127.0.0.1:$((40180 + i))" \
      --data-dir="/data/tikv-$i" --config=/data/tikv.toml >/dev/null
  done
  wait_for "$NTIKV stores Up" 300 stores_up || {
    for ((i = 0; i < NTIKV; i++)); do podman logs --tail 15 "$prefix-tikv-$i" 2>&1 | cut -c1-240 >&2; done
    exit 1
  }
  curl -fsS -X POST "http://$PD/pd/api/v2/keyspaces" -H 'content-type: application/json' \
    -d "{\"name\":\"$KEYSPACE\"}" >/dev/null
  for ((j = 0; j < NTIDB; j++)); do
    log "TiDB $j on cpus ${tidb_cpus[$j]}"
    podman run -d --name "$prefix-tidb-$j" --network host --cpuset-cpus "${tidb_cpus[$j]}" \
      --memory "${TIDB_MEM:-1200m}" -v "$data/tidb.toml:/etc/tidb.toml:ro,Z" "$(image_ref tidb)" \
      --store=tikv --path="$PD" --host=127.0.0.1 -P $((44000 + j)) --status=$((45000 + j)) \
      --config=/etc/tidb.toml >/dev/null
    # The first TiDB bootstraps the keyspace alone (11-65 s, SQ1 R1.2).
    wait_for "TiDB $j" 300 tidb_ready $((44000 + j)) || {
      podman logs --tail 25 "$prefix-tidb-$j" 2>&1 | cut -c1-300 >&2
      exit 1
    }
  done
  log "cluster up: $NTIKV store(s), $NTIDB TiDB(s)"
}

down() {
  local names
  names="$(podman ps -a --format '{{.Names}}' | grep "^$prefix-" || true)"
  # shellcheck disable=SC2086
  [[ -z "$names" ]] || podman rm -f -t 5 $names >/dev/null
  rm -rf "$data"
  log "cluster down, data removed"
}

case "${1:-}" in
  up) up ;;
  down) down ;;
  *) echo "usage: cluster.sh up|down" >&2; exit 2 ;;
esac
