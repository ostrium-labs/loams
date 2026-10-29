#!/usr/bin/env bash
# BQ1 native-API load for KV (the BQ1-ALL contract, bench/all/CONTRACT.md):
#   bench/kv/native-load.sh --duration S --cores N --depth D [--backend B] [--op O]
# Writes one JSON line per op to stdout:
#   {engine, op, qps, rps, p50_ms, p99_ms, p999_ms, depth, cores, errors, ...}
# qps counts keys (operations), rps calls (requests).
#
# Backends: memory (default; loams_kv::Store on in-memory redb), embedded
# (redb file with fsync), tikv (loams_kv::Store on TiKV, needs PD at $PD,
# default 127.0.0.1:19379), tikv-raw (TiKV RawKV batch_get/batch_put).
# Ops: mix (default; 90% get requests, 10% put), get, put; several with commas.
# The caller pins the cores (taskset); this script sizes the runtime to --cores.
set -euo pipefail
duration=15 cores=4 depth=1 backend=${BACKEND:-memory} ops=mix
while [ $# -gt 0 ]; do
  case $1 in
    --duration) duration=$2; shift 2 ;;
    --cores) cores=$2; shift 2 ;;
    --depth) depth=$2; shift 2 ;;
    --backend) backend=$2; shift 2 ;;
    --op) ops=$2; shift 2 ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done
BIN=${KV_NATIVE:-}
if [ -z "$BIN" ]; then
  for c in /mnt/Projects/bench-data/bq1-kv/bin/kv-native /mnt/Projects/rust-cache/target/release/kv-native; do
    [ -x "$c" ] && BIN=$c && break
  done
fi
[ -n "$BIN" ] || { echo "build it: cargo build --release -p kv-native" >&2; exit 1; }
DATA=${DATA:-/mnt/Projects/bench-data/bq1-kv}
mkdir -p "$DATA"
# Requests in flight: enough to keep every core busy while some wait on
# storage (64 per core; TiKV needs more than the in-process backends).
conc=${CONCURRENCY:-$((cores * 64))}
IFS=, read -r -a op_list <<<"$ops"
for op in "${op_list[@]}"; do
  "$BIN" --backend "$backend" --op "$op" --depth "$depth" --threads "$cores" --cores "$cores" \
    --concurrency "$conc" --duration "$duration" --warmup 3 \
    --pd "${PD:-127.0.0.1:19379}" --path "$DATA/native-$$.redb"
done
