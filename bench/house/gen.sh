#!/usr/bin/env bash
# Generates the benchmark data with `clickhouse local` (synthetic, deterministic):
#   kv.native     KV_ROWS rows of (id, v, s) for point lookups and small aggregations
#   hits_*.native HITS_ROWS rows, in CHUNK-row files, of a ClickBench-style subset of `hits` columns
#   ins.tsv       INS_ROWS rows for one insert batch (HTTP and native inserts)
# Usage: gen.sh   (sizes from KV_ROWS, HITS_ROWS, INS_ROWS)
source "$(dirname "$0")/lib.sh"
KV_ROWS=${KV_ROWS:-1000000}
HITS_ROWS=${HITS_ROWS:-5000000}
INS_ROWS=${INS_ROWS:-100000}
cl() { taskset -c "$CLIENT_CORES" clickhouse local --query "$1"; }
cl "SELECT number AS id, cityHash64(number) % 1000000 AS v, concat('s', toString(number % 1000)) AS s
    FROM numbers($KV_ROWS) FORMAT Native" > "$DATA/kv.native"
CHUNK=${CHUNK:-500000}
for start in $(seq 0 "$CHUNK" $((HITS_ROWS - 1))); do
cl "SELECT
      cityHash64(number) AS WatchID,
      cityHash64(number % 400000) AS UserID,
      toUInt32(if(number % 10 = 0, 62, number % 5000)) AS CounterID,
      toUInt32(number % 300) AS RegionID,
      toUInt16(if(number % 20 = 0, number % 7 + 1, 0)) AS AdvEngineID,
      toUInt16(800 + number % 1200) AS ResolutionWidth,
      toUInt8(number % 9 = 0) AS IsRefresh,
      toDate('2013-07-01') + toIntervalDay(number % 31) AS EventDate,
      toDateTime('2013-07-01 00:00:00') + toIntervalSecond(number % 2678400) AS EventTime,
      if(number % 3 = 0, concat('phrase ', toString(cityHash64(number) % 20000)), '') AS SearchPhrase,
      concat('https://', if(number % 13 = 0, 'google.com', 'example.org'), '/p/', toString(number % 50000)) AS URL
    FROM numbers($start, $CHUNK) FORMAT Native" > "$DATA/hits_$start.native"
done
cl "SELECT number AS id, toDateTime('2026-10-28 00:00:00') + toIntervalSecond(number) AS ts,
      toUInt32(cityHash64(number) % 1000) AS k, number / 7 AS v, concat('row ', toString(number)) AS s
    FROM numbers($INS_ROWS) FORMAT TSV" > "$DATA/ins.tsv"
cl "SELECT * FROM file('$DATA/ins.tsv', TSV, 'id UInt64, ts DateTime, k UInt32, v Float64, s String') FORMAT Native" > "$DATA/ins.native"
# Random ids for point lookups and range starts (fixed seed: reproducible).
cl "SELECT concat('SELECT v, s FROM kv WHERE id = ', toString(cityHash64(number, 7) % $KV_ROWS)) FROM numbers(20000) FORMAT TSVRaw" > "$DATA/point.tsv"
cl "SELECT concat('SELECT count(), sum(v) FROM kv WHERE id BETWEEN ', toString(x), ' AND ', toString(x + 999))
    FROM (SELECT cityHash64(number, 11) % ($KV_ROWS - 1000) AS x FROM numbers(20000)) FORMAT TSVRaw" > "$DATA/agg.tsv"
ls -la "$DATA"
