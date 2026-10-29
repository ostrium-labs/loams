#!/usr/bin/env bash
# BQ1-pg hold 4, "pipeline" (owner, 2026-10-28: pipelining at every point):
# libpq pipeline mode (extended protocol, \startpipeline ... \endpipeline,
# one Sync per batch) against depth 1, on every path of stock.sh.
#
#   depth 1   pgbench -S -M prepared: one SELECT, one round trip
#   depth N   bench/pg pipeN script: N point SELECTs in one pipeline
#
# The gateway forwards a connection with tokio's copy_bidirectional in both
# modes: it moves whatever bytes a read returns, in order, and flushes when
# its reader has nothing more, so a pipeline crosses it as a few large
# writes, never one write and flush per message.
#
#   GW_BIN=... bench/pg/hold.sh pipeline.sh
set -euo pipefail
# shellcheck source=bench/pg/lib.sh
. "$(dirname "$0")/lib.sh"

: "${DEPTH:=16}"
name=bq1pg-pipe
cleanup() { gw_stop; pg_stop "$name"; }
trap cleanup EXIT

bq1_out pipeline
pg_start "$name" "$PG_CORES" "$PG_PORT"
gw_start "$GW_CORES" "127.0.0.1:$PG_PORT"
mkdir -p "$BQ1_DATA/run"
read -r -a plain <<<"$(conninfo plain "$PG_PORT")"
pgbench_c "${plain[@]}" -- -i -q -s "$SCALE" >/dev/null 2>&1
podman exec "$name" psql -q -U postgres -h 127.0.0.1 -p "$PG_PORT" -c 'VACUUM ANALYZE' >/dev/null

for mode in plain tls gw-splice gw-terminate; do
  bench_run "pipe-c16-d1" "$mode" "$PG_PORT" select-only 16 '{"pipeline_depth": 1}'
  bench_run "pipe-c16-d$DEPTH" "$mode" "$PG_PORT" "pipe$DEPTH" 16
done
# One connection: what a single client gains from pipelining, per path.
for mode in tls gw-splice; do
  bench_run "pipe-c1-d$DEPTH" "$mode" "$PG_PORT" "pipe$DEPTH" 1
done
# A deeper pipeline on the splice path.
for mode in tls gw-splice; do
  bench_run "pipe-c16-d64" "$mode" "$PG_PORT" pipe64 16
done
bq1_log "results: $BQ1_OUT"
