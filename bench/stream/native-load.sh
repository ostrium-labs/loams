#!/usr/bin/env bash
# BQ1 native load for the stream engines (owner rule: native APIs only).
# RisingWave's native endpoints are its PG wire: ingest with multi-row INSERT
# into a table, and SQL reads of a materialized view. Flink and Spark have no
# native ingest API of their own (they ingest through connectors), so they
# are not part of the combined run; see stream.md.
#
#   bench/stream/native-load.sh --duration 30 --cores 0-5 --depth 16 \
#       [--loadgen-cores 8-11] [--conns 8] [--rows 100] [--keep]
#
# It starts a RisingWave cluster (meta, compactor, compute, frontend) pinned
# to --cores with its Postgres + RustFS on --infra-cores, runs `insert` then
# `select` for --duration seconds each at pipeline depth --depth, prints one
# JSON line per op {engine,op,qps,p50_ms,p99_ms,p999_ms,depth,cores,errors,...}
# and removes the cluster (unless --keep). Run it under benchslot.sh.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
data=${BQ1_STREAM_DATA:-/mnt/Projects/bench-data/bq1-stream}
duration=30 cores=0-5 depth=1 lgcores="" infra=12-13 conns=8 rows=100 keep=0
while (($#)); do
  case $1 in
    --duration) duration=$2; shift 2 ;;
    --cores) cores=$2; shift 2 ;;
    --depth) depth=$2; shift 2 ;;
    --loadgen-cores) lgcores=$2; shift 2 ;;
    --infra-cores) infra=$2; shift 2 ;;
    --conns) conns=$2; shift 2 ;;
    --rows) rows=$2; shift 2 ;;
    --keep) keep=1; shift ;;
    *) echo "native-load.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
ncores() { python3 -c "
import sys
n=0
for part in sys.argv[1].split(','):
    a,_,b=part.partition('-'); n+=int(b or a)-int(a)+1
print(n)" "$1"; }
export ENGINE_CPUSET=$cores INFRA_CPUSET=$infra ENGINE_CORES=$(ncores "$cores")
mkdir -p "$data/rustfs"
compose() { podman compose -f "$here/compose.yaml" --profile risingwave "$@" >/dev/null 2>&1; }
psql_rw() { psql "host=127.0.0.1 port=14566 user=root dbname=dev connect_timeout=5" -XAtq -v ON_ERROR_STOP=1 "$@"; }
lg() { if [[ -n $lgcores ]]; then taskset -c "$lgcores" "$data/loadgen" "$@"; else "$data/loadgen" "$@"; fi; }
cleanup() {
  ((keep)) && return 0
  compose down -v --remove-orphans || true
  podman unshare rm -rf "$data/rustfs" 2>/dev/null || true
}
trap cleanup EXIT

compose up -d postgres rustfs rw-meta rw-compactor rw-compute rw-frontend
for _ in $(seq 120); do curl -s -o /dev/null http://127.0.0.1:19000/ && break; sleep 1; done
curl -sf -o /dev/null -H "x-amz-content-sha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" \
  --aws-sigv4 "aws:amz:us-east-1:s3" --user "bench:benchbench" -X PUT http://127.0.0.1:19000/bench-engines || true
for _ in $(seq 240); do psql_rw -c 'SELECT 1' >/dev/null 2>&1 && break; sleep 1; done
psql_rw >/dev/null <<SQL
DROP MATERIALIZED VIEW IF EXISTS agg_native;
DROP TABLE IF EXISTS events_native;
CREATE TABLE events_native (user_id BIGINT, amount BIGINT, ts TIMESTAMPTZ) APPEND ONLY;
CREATE MATERIALIZED VIEW agg_native AS
  SELECT user_id % 100 AS bucket, count(*) AS cnt, sum(amount) AS total
  FROM events_native GROUP BY user_id % 100;
SQL
dsn="host=127.0.0.1 port=14566 user=root dbname=dev sslmode=disable"
lg native -dsn "$dsn" -op insert -depth "$depth" -rows "$rows" -conns "$conns" \
  -duration "${duration}s" -engine risingwave -cores "$cores"
psql_rw -c 'FLUSH' >/dev/null
lg native -dsn "$dsn" -op select -depth "$depth" -conns "$conns" \
  -duration "${duration}s" -engine risingwave -cores "$cores"
