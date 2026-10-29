#!/usr/bin/env bash
# Read QPS on a loaded House: point lookups and small aggregations over HTTP
# (oha) and native (clickhouse benchmark) at several concurrencies, then the
# ClickBench-style subset over native, one query at a time.
# Usage: read.sh <name> <http_port> <native_port> <out_dir> [secs] [concurrencies]
# Run it under benchslot.sh; keep a hold under 15 minutes.
source "$(dirname "$0")/lib.sh"
name=$1; http=$2; native=$3; out=$4; secs=${5:-10}; concs=${6:-"1 8 32 64"}
mkdir -p "$out"
k=$(key "$name")
# The whole URL is a regular expression for oha: no dots in the host (oha's
# generator does not honour `\.`), `[?]` for the query mark, and a key of
# letters and digits only (BENCH_KEY).
base="http://localhost:$http/[?]user=default&password=$k&query="
point="${base}SELECT%20v%2C%20s%20FROM%20kv%20WHERE%20id%20%3D%20[1-9][0-9]{0,5}"
agg="${base}SELECT%20count%28%29%2C%20sum%28v%29%20FROM%20kv%20WHERE%20intDiv%28id%2C%201000%29%20%3D%20[1-9][0-9]{0,2}"
for c in $concs; do
    oha_json "$out/http_point_c$c.json" -z "${secs}s" -c "$c" --rand-regex-url "$point"
    oha_json "$out/http_agg_c$c.json" -z "${secs}s" -c "$c" --rand-regex-url "$agg"
    chbench "$out/native_point_c$c.txt" "$native" "$name" "$DATA/point.tsv" "$c" "$secs"
    chbench "$out/native_agg_c$c.txt" "$native" "$name" "$DATA/agg.tsv" "$c" "$secs"
done
if [[ ${CLICKBENCH:-1} == 1 ]]; then
    i=0
    while IFS= read -r stmt; do
        printf '%s\n' "$stmt" > "$out/cb_q$i.sql"
        chbench "$out/native_cb_q$i.txt" "$native" "$name" "$out/cb_q$i.sql" "${CB_CONC:-1}" "$secs"
        i=$((i + 1))
    done < "$HERE/sql/clickbench.tsv"
fi
# HTTP/1.1 pipelining (BQ1 rule): depth 1 against depth 16 on the same
# keep-alive connections. The House's hyper server answers a connection's
# pipelined requests in order, one at a time.
if [[ ${PIPE:-1} == 1 ]]; then
    for d in 1 16; do
        taskset -c "$CLIENT_CORES" python3 "$HERE/pipeline.py" "$http" "$k" "${PIPE_CONNS:-8}" "$d" "$secs" 1000000 \
            > "$out/pipe_point_c${PIPE_CONNS:-8}_d$d.json"
    done
fi
