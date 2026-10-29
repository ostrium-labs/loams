#!/usr/bin/env bash
# Native Iggy depth sweep (batch size) and TCP vs QUIC. Run under benchslot:
#   benchslot.sh bq1-iggy bench/iggy/run_native.sh
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"; export BIN="${BIN:-/mnt/Projects/bench-data/bq1-iggy/bin}"
OUT="$HERE/../results/bq1/iggy"; mkdir -p "$OUT"; : > "$OUT/native.jsonl"
for cfg in "tcp 1" "tcp 10" "tcp 100" "tcp 1000" "tcp 10000" "quic 1000"; do set -- $cfg
  echo "== $1 depth $2" >&2
  "$HERE/native-load.sh" --duration 15 --cores 8-13 --server-cores 0-3 --depth "$2" --transport "$1" 2>>"$OUT/native.err" \
    | sed "s/^{/{\"transport\":\"$1\",/" | tee -a "$OUT/native.jsonl"
done
