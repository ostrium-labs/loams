#!/usr/bin/env bash
# Creates the tables on a running House and loads kv and hits over HTTP
# (`INSERT … FORMAT Native`), timing each load.
# Usage: load.sh <name> <http_port>
source "$(dirname "$0")/lib.sh"
name=$1; port=$2
while IFS= read -r stmt; do
    [[ -z $stmt ]] && continue
    q "$name" "$port" "${stmt//\{KV_INT\}/${KV_INT:-UInt64}}" > /dev/null
done < "$HERE/sql/setup.sql"
# kv's id and v are UInt64 (KV_INT=UInt64, the default) or Int64: the House
# stores UInt64 as Iceberg decimal(20, 0), whose CAST in the read view stops
# Parquet row-group pruning (see the report).
kvfile="$DATA/kv_$( [[ ${KV_INT:-UInt64} == Int64 ]] && echo i64 || echo u64 ).native"
t0=$(date +%s.%N)
q "$name" "$port" "INSERT INTO kv FORMAT Native" "$kvfile" > /dev/null
t1=$(date +%s.%N)
for f in "$DATA"/hits_*.native; do
    q "$name" "$port" "INSERT INTO hits FORMAT Native" "$f" > /dev/null
done
t2=$(date +%s.%N)
awk -v a="$t0" -v b="$t1" -v c="$t2" 'BEGIN { printf "load kv: %.2f s; hits: %.2f s\n", b - a, c - b }' | tee "$DATA/$name/load.txt"
q "$name" "$port" "SELECT 'kv', count() FROM kv UNION ALL SELECT 'hits', count() FROM hits"
