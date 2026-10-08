#!/usr/bin/env bash
# Captures MySQL client handshakes against keyspace-mode TiDB v8.5.8 for the
# loams-sqlgate codec fixtures (SQ1 Task 3). Needs the spike stack
# (scripts/sqldb/spike/up.sh), python3, and the clients below; a client that
# is missing is skipped and reported.
#   MYSQL84_IMAGE   mysql 8.4 client image (by digest)
#   CONNECTOR_J     path to mysql-connector-j-*.jar
#   MYSQL2_MODULES  a node_modules directory that holds mysql2
# Usage: capture.sh [out dir] (default crates/loams-sqlgate/tests/fixtures/clients)
# shellcheck shell=bash
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../../sqldb/spike/lib.sh
source "$HERE/../../sqldb/spike/lib.sh"
OUT="${1:-$SPIKE_ROOT/crates/loams-sqlgate/tests/fixtures/clients}"
PROXY_PORT=24300
N=90   # TiDB on 24090/25090
mkdir -p "$OUT"

ks="capture$(date +%s)"
ks_create "$ks" >/dev/null
tidb_start "$N" "$ks" false false
trap 'tidb_stop "$N"' EXIT
wait_query "$(tidb_port "$N")" 180 || { log "TiDB did not start"; exit 1; }
sql "$(tidb_port "$N")" -e "CREATE USER IF NOT EXISTS 'loams_cap'@'%' IDENTIFIED WITH caching_sha2_password BY 'capture'"

# run_capture <name> <ssl-probe 0|1> <command...>
run_capture() {
  local name="$1" probe="$2"; shift 2
  local file="$OUT/$name.jsonl"
  [[ "$probe" == 1 ]] && file="$OUT/$name-ssl.jsonl"
  local args=("$PROXY_PORT" "$(tidb_port "$N")" "$file")
  [[ "$probe" == 1 ]] && args+=(--ssl-probe)
  python3 "$HERE/proxy.py" "${args[@]}" > "$SPIKE_OUT/proxy.log" 2>&1 &
  local pid=$!
  for _ in $(seq 50); do grep -q ready "$SPIKE_OUT/proxy.log" 2>/dev/null && break; sleep 0.1; done
  timeout 30 "$@" || true
  wait "$pid" || true
  log "$name (ssl-probe=$probe): $(wc -l < "$file") packets"
}

clients=()
if [[ -n "${MYSQL84_IMAGE:-}" ]]; then
  clients+=(mysql84)
fi
if [[ -n "${MYSQL2_MODULES:-}" ]] && command -v node >/dev/null; then clients+=(mysql2); fi
if [[ -n "${CONNECTOR_J:-}" ]] && command -v java >/dev/null; then clients+=(connector-j); fi
if command -v mariadb >/dev/null; then clients+=(mariadb); fi

for c in "${clients[@]}"; do
  for probe in 0 1; do
    case "$c" in
      mysql84) run_capture mysql84 "$probe" podman run --rm --network host "$MYSQL84_IMAGE" \
                 mysql -h127.0.0.1 -P"$PROXY_PORT" -uloams_cap -pcapture --get-server-public-key -e 'SELECT 1' ;;
      mysql2) run_capture mysql2 "$probe" env NODE_PATH="$MYSQL2_MODULES" node "$HERE/capture.js" "$PROXY_PORT" ;;
      connector-j) run_capture connector-j "$probe" java -cp "$CONNECTOR_J" "$HERE/Capture.java" "$PROXY_PORT" ;;
      mariadb) run_capture mariadb "$probe" mariadb --skip-ssl -h127.0.0.1 -P"$PROXY_PORT" -uloams_cap -pcapture -e 'SELECT 1' ;;
    esac
  done
done
log "fixtures in $OUT; missing: $(for c in mysql84 mysql2 connector-j mariadb go-sql-driver; do [[ " ${clients[*]} " == *" $c "* ]] || printf '%s ' "$c"; done)"
