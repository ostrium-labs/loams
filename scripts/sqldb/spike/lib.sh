#!/usr/bin/env bash
# Shared helpers for the Loams SQL spike scripts (SQ1 Task 1). Sourced, not run.
# shellcheck shell=bash
# The variables below are used by the scripts that source this file.
# shellcheck disable=SC2034

SPIKE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
SPIKE_DEPLOY="$SPIKE_ROOT/deploy/sqldb/spike"
SPIKE_OUT="${SPIKE_OUT:-$SPIKE_ROOT/target-spike}"
SPIKE_PROJECT=loams-sqldb-spike
PD_URL="${PD_URL:-http://127.0.0.1:29379}"
PD_ADDR=127.0.0.1:29379
TIKV_STATUS=http://127.0.0.1:30180
# Memory limit for each TiDB container (the host is shared; keep it tight).
TIDB_MEM="${TIDB_MEM:-1g}"
mkdir -p "$SPIKE_OUT"

# image_ref <name>: "<image>@<digest>" from release/sqldb-images.toml.
image_ref() {
  awk -v want="[$1]" '
    /^\[/ { sec = $0 }
    sec == want && $1 == "image"  { gsub(/"/, "", $3); img = $3 }
    sec == want && $1 == "digest" { gsub(/"/, "", $3); dig = $3 }
    END { if (img == "" || dig == "") exit 1; print img "@" dig }
  ' "$SPIKE_ROOT/release/sqldb-images.toml"
}

spike_compose() {
  PD_IMAGE="$(image_ref pd)" TIKV_IMAGE="$(image_ref tikv)" \
    podman compose -f "$SPIKE_DEPLOY/compose.yaml" -p "$SPIKE_PROJECT" "$@"
}

now_ms() { date +%s%3N; }

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

# emit <file> <jq args...> -- '<jq object expr>': append one JSON line.
emit() {
  local file="$1"; shift
  jq -nc "$@" >> "$SPIKE_OUT/$file"
}

# ---- PD keyspaces -----------------------------------------------------------

ks_create() {
  curl -fsS -X POST "$PD_URL/pd/api/v2/keyspaces" \
    -H 'content-type: application/json' -d "{\"name\":\"$1\"}"
}

ks_state() {
  curl -fsS -X PUT "$PD_URL/pd/api/v2/keyspaces/$1/state" \
    -H 'content-type: application/json' -d "{\"state\":\"$2\"}"
}

ks_id() { curl -fsS "$PD_URL/pd/api/v2/keyspaces/$1" | jq -r .id; }

# ---- TiDB containers --------------------------------------------------------

tidb_name() { echo "$SPIKE_PROJECT-tidb-$1"; }
tidb_port() { echo $((24000 + $1)); }

# tidb_render <n> <keyspace> <split-table> <force-init-stats>: writes the config.
tidb_render() {
  local conf="$SPIKE_OUT/tidb-$1.toml"
  # Single quotes on purpose: envsubst's list of variables to substitute.
  # shellcheck disable=SC2016
  KEYSPACE_NAME="$2" SPLIT_TABLE="$3" FORCE_INIT_STATS="$4" \
    envsubst '${KEYSPACE_NAME} ${SPLIT_TABLE} ${FORCE_INIT_STATS}' \
    < "$SPIKE_DEPLOY/tidb.toml.tmpl" > "$conf"
  [[ -n "$2" ]] || { log "refusing a TiDB without keyspace-name"; return 1; }
  echo "$conf"
}

# tidb_start <n> <keyspace> <split-table> <force-init-stats>: returns once the
# container runs (the port may not be open yet).
tidb_start() {
  local n="$1" conf
  conf="$(tidb_render "$@")" || return 1
  podman run -d --rm --name "$(tidb_name "$n")" --network host \
    --label "io.loams.spike=$SPIKE_PROJECT" \
    --memory "$TIDB_MEM" \
    -v "$conf:/etc/tidb.toml:ro,Z" \
    "$(image_ref tidb)" \
    --store=tikv --path="$PD_ADDR" \
    --host=127.0.0.1 -P "$(tidb_port "$n")" --status=$((25000 + n)) \
    --config=/etc/tidb.toml >/dev/null
}

tidb_stop() {
  podman stop -t 10 "$(tidb_name "$1")" >/dev/null 2>&1 || true
  # --rm removes it; wait until the name is free again.
  local i
  for i in $(seq 100); do
    podman container exists "$(tidb_name "$1")" || return 0
    sleep 0.1
  done
}

port_open() { (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; }

# wait_port <port> <timeout s>
wait_port() {
  local deadline=$(( $(now_ms) + $2 * 1000 ))
  until port_open "$1"; do
    (( $(now_ms) < deadline )) || return 1
    sleep 0.01
  done
}

sql() {
  local port="$1"; shift
  mariadb --skip-ssl -h127.0.0.1 -P"$port" -uroot -N -B "$@"
}

# wait_query <port> <timeout s>: until SELECT 1 answers.
wait_query() {
  local deadline=$(( $(now_ms) + $2 * 1000 ))
  until [[ "$(sql "$1" -e 'SELECT 1' 2>/dev/null)" == 1 ]]; do
    (( $(now_ms) < deadline )) || return 1
    sleep 0.01
  done
}

# rss_kib <container>: VmRSS of the container's main process, in KiB.
rss_kib() {
  local pid
  pid="$(podman inspect -f '{{.State.Pid}}' "$1" 2>/dev/null)" || { echo 0; return; }
  awk '/^VmRSS:/ { print $2 }' "/proc/$pid/status" 2>/dev/null || echo 0
}

# cgroup_mib <container>: podman stats memory usage, in MiB.
cgroup_mib() {
  podman stats --no-stream --format '{{.MemUsage}}' "$1" 2>/dev/null \
    | awk '{ v = $1; u = v; gsub(/[0-9.]/, "", u); gsub(/[^0-9.]/, "", v);
             f = (u ~ /^G/) ? 1024 : (u ~ /^k/) ? 1/1024 : (u ~ /^M/) ? 1 : 1/1048576;
             printf "%.1f\n", v * f }'
}
