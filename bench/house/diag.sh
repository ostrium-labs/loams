#!/usr/bin/env bash
# Root-cause probes for per-statement latency (run under benchslot.sh):
# sequential statements by hand over HTTP and native, from trivial to a
# point lookup, with the server's own durations, plus chDB on the same
# Parquet file without the House around it.
source "$(dirname "$0")/lib.sh"
out="$DATA/runs/diag"; rm -rf "$out" "$DATA/diagnode"; mkdir -p "$out"
pid=$(start_house diagnode "$SERVER_CORES" 38123 39000 38125 "${WORKERS:-}")
trap 'stop_house "$pid"' EXIT
q diagnode 38123 "CREATE TABLE kv (id UInt64, v UInt64, s String) ENGINE = MergeTree ORDER BY id" > /dev/null
q diagnode 38123 "INSERT INTO kv FORMAT Native" "$DATA/kv.native" > /dev/null
echo "count: $(q diagnode 38123 'SELECT count() FROM kv')"
http_time() { # one statement over HTTP with keep-alive off, total seconds
    curl -sS -o /dev/null -w '%{time_total}\n' -H "X-ClickHouse-User: default" -H "X-ClickHouse-Key: $BENCH_KEY" \
        --data-binary @/dev/null "http://127.0.0.1:38123/?query=$(urlenc "$1")"
}
for sql in "SELECT 1" "SELECT count() FROM kv" "SELECT v, s FROM kv WHERE id = 424242" \
           "SELECT count(), sum(v) FROM kv WHERE intDiv(id, 1000) = 424"; do
    for i in 1 2 3 4 5 6 7 8 9 10; do http_time "$sql"; done > "$out/http_$(echo "$sql" | tr -c 'a-zA-Z0-9' _ | cut -c1-40).txt"
    echo "HTTP x10 [$sql]: $(sort -n "$out"/http_$(echo "$sql" | tr -c 'a-zA-Z0-9' _ | cut -c1-40).txt | sed -n 5p) s median"
    t0=$(date +%s%N)
    for i in 1 2 3 4 5 6 7 8 9 10; do
        taskset -c "$CLIENT_CORES" clickhouse client --port 39000 --user default --password "$BENCH_KEY" --query "$sql" > /dev/null
    done
    t1=$(date +%s%N)
    echo "native x10 [$sql]: $(( (t1 - t0) / 10000000 )) ms mean (includes client start)"
done
echo "--- server durations (ms) by statement, last 80"
grep house_statement "$DATA/diagnode/house.log" | tail -80 | python3 -c '
import sys, json, collections
by = collections.defaultdict(list)
for l in sys.stdin:
    s = json.loads(l)["house_statement"]
    by[(s["surface"], s["query"][:60])].append(s["duration_ms"])
for k, v in by.items():
    v.sort(); print(k, "n", len(v), "median", v[len(v)//2], "min", v[0])'
echo "--- the lake layout"
find "$DATA/diagnode/bucket" -type f | sed "s#$DATA/diagnode/bucket/##" | awk -F/ '{print $NF}' | sed 's/[0-9a-f-]\{20,\}/X/g' | sort | uniq -c | head -20
f=$(find "$DATA/diagnode/bucket" -name '*.parquet' -path '*kv*' | head -1)
echo "--- chDB on the data file alone: $f"
for i in 1 2 3; do
    /usr/bin/time -f "%e s" taskset -c "$SERVER_CORES" clickhouse local --query "SELECT v, s FROM file('$f', Parquet) WHERE id = 424242" 2>&1
done
stop_house "$pid"; trap - EXIT
cp "$DATA/diagnode/house.log" "$out/"
