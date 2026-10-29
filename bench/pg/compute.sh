#!/usr/bin/env bash
# BQ1-pg hold 3, "compute": the Loams Postgres compute path. compute1 of
# deploy/loams-postgres-dev (the pinned Loams Postgres 17.11 compute, its
# pages from the pageserver, its WAL to a safekeeper) on PG_CORES, the
# storage tier on STORAGE_CORES, pgbench on CLIENT_CORES in compute1's
# network namespace (loopback, as the stock server's runs).
#
#   scale 10   ~160 MB: fits shared_buffers (128 MB) plus the local file
#              cache (256 MB) of the compute spec, so reads stay in the compute
#   scale 50   ~750 MB: exceeds them, so select-only misses go to the
#              pageserver (GetPage@LSN), the path a cold or large tenant takes
#
# The compute has no TLS in this stack (deploy/loams-postgres-dev/README.md),
# so the gateway, which needs TLS to its upstream, is measured in stock.sh.
#
#   .he0-shared/benchslot.sh bq1-pg bench/pg/compute.sh
#
# Its own compose project (bq1pg) and volumes, removed at the end.
set -euo pipefail
# shellcheck source=bench/pg/lib.sh
. "$(dirname "$0")/lib.sh"

dev=$BQ1_ROOT/deploy/loams-postgres-dev
export PG_CORES STORAGE_CORES
dc() { podman compose -p bq1pg -f "$dev/compose.yaml" -f "$BQ1_HERE/compute.override.yaml" "$@"; }
cleanup() { dc down -v --remove-orphans >/dev/null 2>&1 || true; }
trap cleanup EXIT

bq1_out compute
dc down -v --remove-orphans >/dev/null 2>&1 || true
bq1_log "starting the Loams Postgres stack"
dc up -d compute1 >/dev/null 2>&1
cid=$(dc ps -q compute1)
BQ1_CLIENT_NET="--network=container:$cid"
BQ1_USER=cloud_admin
BQ1_PW=cloud_admin
export BQ1_CLIENT_NET BQ1_USER BQ1_PW
ok=
for _ in $(seq 180); do
  if podman exec "$cid" sh -c 'pg_isready -q -h 127.0.0.1 -p 55433' 2>/dev/null; then ok=1; break; fi
  sleep 1
done
[ -n "$ok" ] || { dc logs --tail 40 compute1 >&2; exit 1; }

for scale in 10 50; do
  bq1_log "pgbench -i scale $scale (through the safekeeper and the pageserver)"
  mkdir -p "$BQ1_DATA/run"; chmod 0777 "$BQ1_DATA/run"
  t0=$(date +%s)
  pgbench_c PGHOST=127.0.0.1 PGPORT=55433 PGSSLMODE=disable -- -i -q -s "$scale" >/dev/null 2>&1
  podman exec "$cid" psql -q -U cloud_admin -h 127.0.0.1 -p 55433 -d postgres -c 'VACUUM ANALYZE' >/dev/null
  init_s=$(( $(date +%s) - t0 ))
  bench_run "compute-select-c64-s$scale" plain 55433 select-only 64 "{\"scale\": $scale, \"init_s\": $init_s, \"server\": \"loams-postgres-compute-v17\"}"
  bench_run "compute-select-c1-s$scale" plain 55433 select-only 1 "{\"scale\": $scale, \"server\": \"loams-postgres-compute-v17\"}"
  bench_run "compute-tpcb-c32-s$scale" plain 55433 tpcb 32 "{\"scale\": $scale, \"server\": \"loams-postgres-compute-v17\"}"
  # Pipelining (owner, 2026-10-28): depth 1 against a 16-statement libpq
  # pipeline, 16 connections each.
  bench_run "compute-pipe-c16-d1-s$scale" plain 55433 select-only 16 "{\"scale\": $scale, \"server\": \"loams-postgres-compute-v17\"}"
  bench_run "compute-pipe-c16-d16-s$scale" plain 55433 pipe16 16 "{\"scale\": $scale, \"server\": \"loams-postgres-compute-v17\"}"
done
# The storage tier's CPU over the whole hold is in each run's cpu.storage.
bq1_log "results: $BQ1_OUT"
