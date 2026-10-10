#!/usr/bin/env bash
# Captures MySQL client handshakes against keyspace-mode TiDB v8.5.8 for the
# loams-sqlgate codec fixtures (SQ1 Task 3; tests/fixtures/clients/README.md).
# Needs the spike stack (scripts/sqldb/spike/up.sh), python3, openssl and
# podman. Every client is pinned; a version that does not match fails.
#   MYSQL84_IMAGE   mysql 8.4 client image, by digest (default: the pin)
#   NODE_IMAGE      node image for mysql2, by digest (node/package-lock.json)
#   GOLANG_IMAGE    golang image for go-sql-driver, by digest (go/go.sum)
#   CONNECTOR_J     path to mysql-connector-j-9.7.0.jar (optional)
#   CLIENTS         optional: capture only these (e.g. "mysql84 go-sql-driver")
# Modules are downloaded inside the containers only. Usage:
#   capture.sh [out dir] (default crates/loams-sqlgate/tests/fixtures/clients)
# shellcheck shell=bash
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../../sqldb/spike/lib.sh
source "$HERE/../../sqldb/spike/lib.sh"
OUT="${1:-$SPIKE_ROOT/crates/loams-sqlgate/tests/fixtures/clients}"
PROXY_PORT=24300
N=90   # TiDB on 24090/25090

MYSQL84_IMAGE="${MYSQL84_IMAGE:-docker.io/library/mysql@sha256:02aa6476f6b675e5d4d19c5b437798b1b8a4048ae39383eac609814998ed15b8}"
NODE_IMAGE="${NODE_IMAGE:-docker.io/library/node@sha256:c385ec44d77c785e2364ac0c9b150809a0fdc17fde3dbf061e3dad07242c6a85}"
GOLANG_IMAGE="${GOLANG_IMAGE:-docker.io/library/golang@sha256:ebd54034f076819b3054f155db53660ded951612bb4dfd277f933e62059e5d5a}"
CONNECTOR_J_VERSION=9.7.0
MARIADB_VERSION=12.3.3-MariaDB   # client libmariadb 3.4.10
MYSQL84_VERSION=8.4.10

for img in "$MYSQL84_IMAGE" "$NODE_IMAGE" "$GOLANG_IMAGE"; do
  [[ "$img" == *@sha256:* ]] || { log "image not pinned by digest: $img"; exit 1; }
done

clients=(mysql84 mysql2 go-sql-driver)
if [[ -n "${CONNECTOR_J:-}" ]]; then
  v="$(unzip -p "$CONNECTOR_J" META-INF/MANIFEST.MF | sed -n 's/^Implementation-Version: *//p' | tr -d '\r')"
  [[ "$v" == "$CONNECTOR_J_VERSION" ]] || { log "Connector/J is $v, expected $CONNECTOR_J_VERSION"; exit 1; }
  clients+=(connector-j)
fi
if command -v mariadb >/dev/null; then
  mariadb --version | grep -q "from $MARIADB_VERSION," \
    || { log "mariadb is not $MARIADB_VERSION: $(mariadb --version)"; exit 1; }
  clients+=(mariadb)
fi
if [[ -n "${CLIENTS:-}" ]]; then
  wanted=()
  for c in "${clients[@]}"; do [[ " $CLIENTS " == *" $c "* ]] && wanted+=("$c"); done
  clients=("${wanted[@]}")
fi
v="$(podman run --rm "$MYSQL84_IMAGE" mysql --version)"
[[ "$v" == *"Ver $MYSQL84_VERSION "* ]] || { log "mysql client is not $MYSQL84_VERSION: $v"; exit 1; }
mkdir -p "$OUT"

# A throwaway certificate for --tls-sha2 (the proxy terminates client TLS).
TLS_DIR="$SPIKE_OUT/capture-tls"
mkdir -p "$TLS_DIR"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj /CN=127.0.0.1 \
  -keyout "$TLS_DIR/key.pem" -out "$TLS_DIR/cert.pem" 2>/dev/null

ks="capture$(date +%s)"
ks_create "$ks" >/dev/null
tidb_start "$N" "$ks" false false
trap 'tidb_stop "$N"' EXIT
wait_query "$(tidb_port "$N")" 180 || { log "TiDB did not start"; exit 1; }
sql "$(tidb_port "$N")" -e "CREATE USER IF NOT EXISTS 'loams_cap'@'%' IDENTIFIED WITH caching_sha2_password BY 'capture'"

# run_client <client> <tls: 0|1>: one connection through the proxy.
run_client() {
  local t=(timeout 180)
  case "$1" in
    mysql84)
      local ssl=(); [[ "$2" == 1 ]] && ssl=(--ssl-mode=REQUIRED)
      "${t[@]}" podman run --rm --network host "$MYSQL84_IMAGE" mysql -h127.0.0.1 -P"$PROXY_PORT" \
        -uloams_cap -pcapture --get-server-public-key "${ssl[@]}" -e 'SELECT 1' ;;
    mysql2)
      "${t[@]}" podman run --rm --network host -v "$HERE/node:/src:ro,z" "$NODE_IMAGE" sh -c \
        "cp -r /src /work && cd /work && npm ci --ignore-scripts --no-audit --no-fund >/dev/null && node capture.js $PROXY_PORT" ;;
    go-sql-driver)
      "${t[@]}" podman run --rm --network host -v "$HERE/go:/src:ro,z" "$GOLANG_IMAGE" sh -c \
        "cp -r /src /work && cd /work && go run -mod=readonly . $PROXY_PORT" ;;
    connector-j)
      local mode=PREFERRED; [[ "$2" == 1 ]] && mode=REQUIRED
      "${t[@]}" java -cp "$CONNECTOR_J" "$HERE/Capture.java" "$PROXY_PORT" "$mode" ;;
    mariadb)
      "${t[@]}" mariadb --skip-ssl -h127.0.0.1 -P"$PROXY_PORT" -uloams_cap -pcapture -e 'SELECT 1' ;;
  esac
}

# run_capture <file name> <proxy mode: plain|ssl-probe|tls-sha2> <client> <tls>
run_capture() {
  local name="$1" mode="$2"
  local file="$OUT/$name.jsonl"
  local args=("$PROXY_PORT" "$(tidb_port "$N")" "$file")
  case "$mode" in
    ssl-probe) args+=(--ssl-probe) ;;
    tls-sha2) args+=(--tls-sha2 "$TLS_DIR/cert.pem" "$TLS_DIR/key.pem") ;;
  esac
  python3 "$HERE/proxy.py" "${args[@]}" > "$SPIKE_OUT/proxy.log" 2>&1 &
  local pid=$!
  for _ in $(seq 50); do grep -q ready "$SPIKE_OUT/proxy.log" 2>/dev/null && break; sleep 0.1; done
  run_client "$3" "$4" || true
  wait "$pid" || true
  log "$name ($mode): $(wc -l < "$file") packets"
}

for c in "${clients[@]}"; do
  run_capture "$c" plain "$c" 0
  run_capture "$c-ssl" ssl-probe "$c" 0
  case "$c" in
    mysql84|connector-j) run_capture "$c-tls-sha2" tls-sha2 "$c" 1 ;;
  esac
done
log "fixtures in $OUT"
