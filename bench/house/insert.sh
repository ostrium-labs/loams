#!/usr/bin/env bash
# Insert rows/s on a running House: INS_ROWS-row batches into tables of 1, 4
# and 16 partitions, over HTTP (oha POSTing the TSV batch) and native
# (concurrent `clickhouse client` loops sending the Native batch).
# Usage: insert.sh <name> <http_port> <native_port> <out_dir> [secs] [concurrencies]
source "$(dirname "$0")/lib.sh"
name=$1; http=$2; native=$3; out=$4; secs=${5:-15}; concs=${6:-"1 4 8"}
mkdir -p "$out"
k=$(key "$name")
rows=$(wc -l < "$DATA/ins.tsv")
echo "$rows" > "$out/batch_rows"
for t in ins_p1 ins_p4 ins_p16; do
    for c in $concs; do
        oha_json "$out/http_${t}_c$c.json" -z "${secs}s" -c "$c" -m POST -D "$DATA/ins.tsv" \
            "http://127.0.0.1:$http/?user=default&password=$k&query=INSERT%20INTO%20$t%20FORMAT%20TSV"
        # Native: c concurrent clients, each sending batches until the time is up.
        end=$(( $(date +%s) + secs ))
        pids=()
        for j in $(seq 1 "$c"); do
            (
                n=0
                while (( $(date +%s) < end )); do
                    taskset -c "$CLIENT_CORES" clickhouse client --host 127.0.0.1 --port "$native" \
                        --user default --password "$k" --query "INSERT INTO $t FORMAT Native" \
                        < "$DATA/ins.native" && n=$((n + 1))
                done
                echo "$n"
            ) > "$out/native_${t}_c${c}_$j.count" &
            pids+=($!)
        done
        t0=$(date +%s.%N)
        wait "${pids[@]}"
        t1=$(date +%s.%N)
        echo "$t0 $t1" > "$out/native_${t}_c$c.time"
    done
done
