#!/usr/bin/env bash
# Batched inserts (BQ1 pipelining rule): rows/s by batch size over HTTP
# keep-alive (oha POST), plus small batches with async_insert (the House
# queues them and commits together).
# Usage: inserts-batch.sh <name> <http_port> <out_dir> [secs] [conc]
source "$(dirname "$0")/lib.sh"
name=$1; http=$2; out=$3; secs=${4:-10}; c=${5:-4}
mkdir -p "$out"
for n in 100 1000 10000 100000; do
    head -n "$n" "$DATA/ins.tsv" > "$DATA/ins_$n.tsv"
    oha_json "$out/http_batch_${n}_c$c.json" -z "${secs}s" -c "$c" -m POST -D "$DATA/ins_$n.tsv" \
        "http://127.0.0.1:$http/?user=default&password=$BENCH_KEY&query=INSERT%20INTO%20ins_p1%20FORMAT%20TSV"
    echo "$n" > "$out/http_batch_${n}_c$c.rows"
done
for n in 100 1000; do
    oha_json "$out/http_async_${n}_c$((c * 8)).json" -z "${secs}s" -c $((c * 8)) -m POST -D "$DATA/ins_$n.tsv" \
        "http://127.0.0.1:$http/?user=default&password=$BENCH_KEY&async_insert=1&wait_for_async_insert=1&query=INSERT%20INTO%20ins_p1%20FORMAT%20TSV"
    echo "$n" > "$out/http_async_${n}_c$((c * 8)).rows"
done
