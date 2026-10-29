#!/usr/bin/env bash
# The HE1 Rust engine on its own (loams-house-engine-dev, plan HE1 Task 23):
# kv seeded in memory, point lookups and small aggregations over HTTP (oha).
# It has no catalog, pins or worker hop, so it bounds what the engine itself
# costs per statement; it is a test server, not the House's serving path.
# Usage: benchslot.sh bq1-house bench/house/engine.sh <tag> [secs] [concurrencies]
source "$(dirname "$0")/lib.sh"
tag=$1; secs=${2:-10}; concs=${3:-"1 8 32"}
run="$DATA/runs/$tag"; rm -rf "$run"; mkdir -p "$run"
[[ -s $DATA/kv.parquet ]] || taskset -c "$CLIENT_CORES" clickhouse local --query \
    "SELECT * FROM file('$DATA/kv.native', Native) FORMAT Parquet" > "$DATA/kv.parquet"
taskset -c "$SERVER_CORES" "$BIN/loams-house-engine-dev" --listen 127.0.0.1:28126 > "$run/engine.log" 2>&1 &
pid=$!
trap 'stop_house "$pid"' EXIT
for _ in $(seq 1 60); do curl -fsS http://127.0.0.1:28126/ping > /dev/null 2>&1 && break; sleep 0.25; done
curl -fsS -H 'X-Loams-Columns: [["id","UInt64"],["v","UInt64"],["s","String"]]' \
    --data-binary "@$DATA/kv.parquet" \
    "http://127.0.0.1:28126/loams/seed?database=default&table=kv&engine=MergeTree&sorting_key=id"
fmt="default_format=JSONCompactEachRowWithNamesAndTypes"
curl -fsS "http://127.0.0.1:28126/?$fmt&query=$(urlenc 'SELECT v, s FROM kv WHERE id = 42')"
base="http://localhost:28126/[?]$fmt&query="
point="${base}SELECT%20v%2C%20s%20FROM%20kv%20WHERE%20id%20%3D%20[1-9][0-9]{0,5}"
agg="${base}SELECT%20count%28%29%2C%20sum%28v%29%20FROM%20kv%20WHERE%20intDiv%28id%2C%201000%29%20%3D%20[1-9][0-9]{0,2}"
for c in $concs; do
    oha_json "$run/http_point_c$c.json" -z "${secs}s" -c "$c" --rand-regex-url "$point"
    oha_json "$run/http_agg_c$c.json" -z "${secs}s" -c "$c" --rand-regex-url "$agg"
done
stop_house "$pid"; trap - EXIT
python3 "$HERE/summarize.py" "$run" "$RESULTS/$tag.json" --label "$tag" \
    --meta "server=loams-house-engine-dev" --meta "server_cores=$SERVER_CORES" \
    --meta "client_cores=$CLIENT_CORES" --meta "commit=$(git -C "$REPO" rev-parse --short HEAD)" > /dev/null
echo "results: $RESULTS/$tag.json"
