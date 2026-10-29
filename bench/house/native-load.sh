#!/usr/bin/env bash
# The House through its native API, ClickHouse's native TCP protocol, for the
# combined run (lane BQ1-ALL, bench/all/). Starts its own single-node House on
# --cores, loads kv (1M rows, Int64 key), runs point lookups and small
# aggregations with `clickhouse benchmark`, and prints one JSON line per op:
#   {"engine":"house","op":…,"qps":…,"p50_ms":…,"p99_ms":…,"p999_ms":…,
#    "depth":…,"cores":…,"errors":…}
# The native protocol answers one query at a time per connection, so --depth
# is the number of concurrent connections (each with one query in flight).
#
# Usage: native-load.sh [--duration 30] [--cores 0-5] [--client-cores 6-11]
#                       [--depth 8] [--ops point,agg] [--port-base 48000]
# Needs gen.sh's kv_i64.native under $DATA and the release binaries in $BIN.
source "$(dirname "$0")/lib.sh"
duration=30; cores=$SERVER_CORES; client=$CLIENT_CORES; depth=8; ops=point,agg; base=48000
while [[ $# -gt 0 ]]; do
    case $1 in
        --duration) duration=$2; shift 2 ;;
        --cores) cores=$2; shift 2 ;;
        --client-cores) client=$2; shift 2 ;;
        --depth) depth=$2; shift 2 ;;
        --ops) ops=$2; shift 2 ;;
        --port-base) base=$2; shift 2 ;;
        *) echo "native-load.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done
duration=${duration%s}
CLIENT_CORES=$client
name="native-$base"
rm -rf "${DATA:?}/$name"
# Workers: one per two server cores (the House's default), all booted first.
ncores=$(taskset -c "$cores" nproc)
workers=$(( ncores / 2 > 0 ? ncores / 2 : 1 ))
pid=$(start_house "$name" "$cores" "$base" $((base + 1)) $((base + 2)) "$workers")
trap 'stop_house "$pid"; rm -rf "${DATA:?}/$name"' EXIT
[[ -s $DATA/kv_i64.native ]] || { echo "native-load.sh: run gen.sh first" >&2; exit 1; }
q "$name" "$base" "CREATE TABLE kv (id Int64, v Int64, s String) ENGINE = MergeTree ORDER BY id" > /dev/null
q "$name" "$base" "INSERT INTO kv FORMAT Native" "$DATA/kv_i64.native" > /dev/null
IFS=, read -r -a list <<< "$ops"
for op in "${list[@]}"; do
    out="$DATA/$name/$op.txt"
    chbench "$out" $((base + 1)) "$name" "$DATA/$op.tsv" "$depth" "$duration"
    python3 - "$out" "$op" "$depth" "$ncores" <<'PY'
import json, re, sys
path, op, depth, cores = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
text = open(path).read()
m = re.findall(r"queries:? (\d+), QPS: ([\d.]+)", text)
pct = {float(p): float(v) * 1000 for p, v in re.findall(r"^([\d.]+)%\s+([\d.]+) sec", text, re.M)}
errors = len(re.findall(r"Code: \d+", text))
print(json.dumps({"engine": "house", "op": op, "qps": float(m[-1][1]) if m else 0.0,
                  "p50_ms": pct.get(50.0), "p99_ms": pct.get(99.0), "p999_ms": pct.get(99.9),
                  "depth": depth, "cores": cores, "errors": errors}))
PY
done
