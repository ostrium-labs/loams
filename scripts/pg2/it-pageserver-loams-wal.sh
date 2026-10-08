#!/usr/bin/env bash
# PG2 Tasks 31 and 32, it_pageserver_ingests_from_loams_wal_without_safekeeper and
# it_pageserver_discovers_loams_wal_via_broker: on deploy/loams-pg-bench with
# no safekeeper of any kind (no `sk` profile, no feeder), a compute writes
# through loams-wal-interpreted; the pageserver finds loams-wal through the
# storage broker, ingests over the interpreted protocol until its
# last_record_lsn reaches the commit, and a compute started afresh (from the
# pageserver's basebackup) reads the data back.
#
#   LOAMS_WAL=<path to loams-wal-interpreted> scripts/pg2/it-pageserver-loams-wal.sh
#
# Host networking, as the benchmark: RustFS :9000, broker :50051, pageserver
# :6400/:9898, compute :55433/:3080, loams-wal :5460/:7680. It takes the
# benchmark's lock, and tears the stack down on exit (KEEP=1 keeps it).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
DEPLOY=$ROOT/deploy/loams-pg-bench
: "${LOAMS_WAL:?set LOAMS_WAL to the loams-wal-interpreted binary}"
[ -x "$LOAMS_WAL" ] || { echo "it: no binary at $LOAMS_WAL" >&2; exit 1; }
RUN_DIR=${RUN_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/loams-it-pg.XXXXXX")}
log() { printf '%s it: %s\n' "$(date -u +%H:%M:%S)" "$*" >&2; }
fail() { log "FAIL: $*"; exit 1; }

LOCK=${LOAMS_BENCH_LOCK:-$HOME/.cache/loams-pg-bench.lock}
mkdir -p "$(dirname "$LOCK")"
exec 9>"$LOCK"
flock 9

if [ -z "${DOCKER_HOST:-}" ] && [ -S "/run/user/$(id -u)/podman/podman.sock" ]; then
  export DOCKER_HOST=unix:///run/user/$(id -u)/podman/podman.sock
fi
COMPOSE=(docker-compose)
command -v docker-compose >/dev/null || COMPOSE=(docker compose)
ENGINE=docker
case ${DOCKER_HOST:-} in *podman*) ENGINE=podman ;; esac
command -v "$ENGINE" >/dev/null || ENGINE=podman

for port in 9000 50051 6400 9898 55433 3080 5460 7680; do
  if ss -ltn | awk '{print $4}' | grep -qE "[:.]$port\$"; then
    fail "port $port is in use"
  fi
done

cd "$DEPLOY"
wal_pid=
teardown() {
  [ -n "$wal_pid" ] && kill "$wal_pid" 2>/dev/null || true
  if [ "${KEEP:-0}" != 1 ]; then
    "${COMPOSE[@]}" down -v >/dev/null 2>&1 || true
  fi
}
trap teardown EXIT

# 1. Storage, with no safekeeper.
"${COMPOSE[@]}" down -v >/dev/null 2>&1 || true
"${COMPOSE[@]}" up -d rustfs create-bucket storage_broker pageserver >/dev/null 2>&1
for _ in $(seq 1 90); do curl -sf localhost:9898/v1/status >/dev/null && break; sleep 1; done
curl -sf localhost:9898/v1/status >/dev/null || fail "the pageserver did not start"
if "${COMPOSE[@]}" ps --services --status running | grep -q safekeeper; then
  fail "a safekeeper is running"
fi

# 2. loams-wal, publishing to the broker.
"$LOAMS_WAL" --listen-pg 127.0.0.1:5460 --listen-http 127.0.0.1:7680 \
  --broker-endpoint http://127.0.0.1:50051 >"$RUN_DIR/loams-wal.log" 2>&1 </dev/null 9>&- &
wal_pid=$!
for _ in $(seq 1 30); do curl -sf localhost:7680/v1/status >/dev/null && break; sleep 0.5; done
curl -sf localhost:7680/v1/status >/dev/null || fail "loams-wal did not start ($RUN_DIR/loams-wal.log)"

# 3. A tenant, a timeline and a compute on loams-wal only.
export TENANT_ID=$(openssl rand -hex 16) TIMELINE_ID=$(openssl rand -hex 16)
export SAFEKEEPERS=127.0.0.1:5460 SHARED_BUFFERS=${SHARED_BUFFERS:-128MB}
curl -sf -X PUT -H 'Content-Type: application/json' \
  -d '{"mode":"AttachedSingle","generation":1,"tenant_conf":{}}' \
  "localhost:9898/v1/tenant/$TENANT_ID/location_config" >/dev/null
curl -sf -X POST -H 'Content-Type: application/json' \
  -d "{\"new_timeline_id\":\"$TIMELINE_ID\",\"pg_version\":${PG_VERSION:-16}}" \
  "localhost:9898/v1/tenant/$TENANT_ID/timeline/" >/dev/null
start_compute() {
  "${COMPOSE[@]}" rm -sf compute >/dev/null 2>&1 || true
  "${COMPOSE[@]}" up -d compute >/dev/null 2>&1
  container=$("${COMPOSE[@]}" ps -q compute)
  for _ in $(seq 1 120); do
    "$ENGINE" exec "$container" pg_isready -q -h 127.0.0.1 -p 55433 2>/dev/null && return 0
    sleep 1
  done
  "$ENGINE" logs "$container" >"$RUN_DIR/compute.log" 2>&1 || true
  fail "the compute did not become ready ($RUN_DIR/compute.log)"
}
psql() {
  "$ENGINE" exec -i "$container" psql -X -q -At -v ON_ERROR_STOP=1 \
    "postgresql://cloud_admin@127.0.0.1:55433/postgres" "$@"
}
log "tenant=$TENANT_ID timeline=$TIMELINE_ID wal=$SAFEKEEPERS"
start_compute

# 4. Writes: more than shared_buffers, so pages are evicted and read back
#    from the pageserver while the compute runs.
psql -c "create table it (id int primary key, payload text)"
psql -c "insert into it select g, repeat(md5(g::text), 8) from generate_series(1, 400000) g"
want=$(psql -c "select count(*) || ':' || sum(id) from it")
commit=$(psql -c "select pg_current_wal_flush_lsn()")
log "wrote $want, flush LSN $commit"

# 5. The pageserver catches up through loams-wal (it found it via the broker).
lsn_ge() { python3 -c "import sys
def v(s): a,b=s.split('/'); return (int(a,16)<<32)|int(b,16)
sys.exit(0 if v(sys.argv[1]) >= v(sys.argv[2]) else 1)" "$1" "$2"; }
last=
for _ in $(seq 1 120); do
  last=$(curl -sf "localhost:9898/v1/tenant/$TENANT_ID/timeline/$TIMELINE_ID" |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["last_record_lsn"])' || true)
  [ -n "$last" ] && lsn_ge "$last" "$commit" && break
  sleep 1
done
[ -n "$last" ] && lsn_ge "$last" "$commit" || fail "last_record_lsn $last never reached $commit"
log "pageserver last_record_lsn $last >= $commit"
grep -q "interpreted replication started" "$RUN_DIR/loams-wal.log" ||
  fail "the pageserver did not stream from loams-wal"

# 6. A fresh compute (basebackup from the pageserver) sees the data.
start_compute
got=$(psql -c "select count(*) || ':' || sum(id) from it")
[ "$got" = "$want" ] || fail "after a compute restart: $got, want $want"
log "after a compute restart: $got"
log "PASS"
