#!/usr/bin/env bash
# BQ1-pg hold 1, "paths": stock Postgres 17.10 with TLS on PG_CORES, pgbench
# on CLIENT_CORES, loams-gateway on GW_CORES. The same server and data are
# measured on every connection path, so the gateway's cost is the difference:
#
#   plain        no TLS (the server's ceiling)
#   tls          TLS straight to the server
#   gw-splice    through the gateway, SSLRequest: SNI routing then a byte splice
#   gw-terminate through the gateway, direct TLS: terminate and re-encrypt
#
#   .he0-shared/benchslot.sh bq1-pg env GW_BIN=.../loams-gateway bench/pg/stock.sh
#
# About 10 minutes. Writes bench/results/bq1/pg/<ts>-paths.jsonl.
set -euo pipefail
# shellcheck source=bench/pg/lib.sh
. "$(dirname "$0")/lib.sh"

name=bq1pg-stock
cleanup() { gw_stop; pg_stop "$name"; }
trap cleanup EXIT

bq1_out paths
pg_start "$name" "$PG_CORES" "$PG_PORT"
gw_start "$GW_CORES" "127.0.0.1:$PG_PORT"
bq1_log "pgbench -i scale $SCALE"
mkdir -p "$BQ1_DATA/run"
read -r -a plain <<<"$(conninfo plain "$PG_PORT")"
pgbench_c "${plain[@]}" -- -i -q -s "$SCALE" >/dev/null 2>&1
podman exec "$name" psql -q -U postgres -h 127.0.0.1 -p "$PG_PORT" -c 'VACUUM ANALYZE' >/dev/null

# Throughput: 64 clients, select-only, every path.
for mode in plain tls gw-splice gw-terminate; do
  bench_run "paths-select-c64" "$mode" "$PG_PORT" select-only 64
done
# Latency of one client (the hop's cost without queueing).
for mode in tls gw-splice gw-terminate; do
  bench_run "paths-select-c1" "$mode" "$PG_PORT" select-only 1
done
# TPC-B, 32 clients.
for mode in tls gw-splice gw-terminate; do
  bench_run "paths-tpcb-c32" "$mode" "$PG_PORT" tpcb 32
done
bq1_log "results: $BQ1_OUT"
