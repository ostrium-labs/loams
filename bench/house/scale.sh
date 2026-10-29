#!/usr/bin/env bash
# Scaling: N independent single-node Houses (N = 1, 2, 4), one core each
# (SCALE_CORES, E-cores by default so every node gets the same kind of core),
# each loaded with kv; one load generator spreads point lookups and small
# aggregations over all of them (clickhouse benchmark with several --port).
# Usage: scale.sh <out_dir> [secs] [conc_per_node]
source "$(dirname "$0")/lib.sh"
out=$1; secs=${2:-10}; per=${3:-16}
read -r -a cores <<< "${SCALE_CORES:-4 5 6 7}"
mkdir -p "$out"
for n in 1 2 4; do
    pids=(); ports=()
    for i in $(seq 0 $((n - 1))); do
        name="scale$i"
        rm -rf "${DATA:?}/$name"
        pid=$(start_house "$name" "${cores[$i]}" $((18123 + i)) $((19000 + i)) $((18200 + i)) 1)
        pids+=("$pid")
        q "$name" $((18123 + i)) "CREATE TABLE kv (id Int64, v Int64, s String) ENGINE = MergeTree ORDER BY id" > /dev/null
        q "$name" $((18123 + i)) "INSERT INTO kv FORMAT Native" "$DATA/kv_i64.native" > /dev/null
        ports+=(--port $((19000 + i)))
    done
    for kind in point agg; do
        taskset -c "$CLIENT_CORES" clickhouse benchmark --host 127.0.0.1 "${ports[@]}" \
            --user default --password "$BENCH_KEY" -c $((per * n)) -t "$secs" -d 0 -r \
            < "$DATA/$kind.tsv" > "$out/native_${kind}_n$n.txt" 2>&1 || true
    done
    for pid in "${pids[@]}"; do stop_house "$pid"; done
    for i in $(seq 0 $((n - 1))); do rm -rf "${DATA:?}/scale$i"; done
done
