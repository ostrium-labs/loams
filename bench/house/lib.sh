#!/usr/bin/env bash
# Shared helpers for the House benchmarks (BQ1-house). Sourced, not run.
#
# Every measurement runs under .he0-shared/benchslot.sh (the caller's job).
# Server and load generator get disjoint cores: SERVER_CORES and CLIENT_CORES.

set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
BIN=${BIN:-/mnt/Projects/bench-data/bq1-house/bin-after}
DATA=${DATA:-/mnt/Projects/bench-data/bq1-house}
SERVER_CORES=${SERVER_CORES:-0-5}
CLIENT_CORES=${CLIENT_CORES:-6-11}
BENCH_KEY=${BENCH_KEY:-bq1housebenchkey0123456789}
RESULTS=${RESULTS:-$REPO/bench/results/bq1/house}
LIBCHDB_DIR=${LIBCHDB_DIR:-/mnt/Projects/rust-cache/libchdb-26.9.0}
export LD_LIBRARY_PATH=${LIBCHDB_DIR}${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}

mkdir -p "$DATA" "$RESULTS"

# start_house <name> <cores> <http_port> <native_port> <admin_port> [max_workers]
# Starts one single-node House pinned to <cores>; prints its PID.
start_house() {
    local name=$1 cores=$2 http=$3 native=$4 admin=$5 workers=${6:-}
    local dir="$DATA/$name"
    mkdir -p "$dir"
    # One fixed local key for every node, so one load generator can spread
    # over several nodes (benchmark only; loopback listeners).
    if [[ ! -s $dir/house.key ]]; then
        (umask 077 && printf '%s\n' "$BENCH_KEY" > "$dir/house.key")
    fi
    cat > "$dir/house.toml" <<TOML
[house]
listen = "127.0.0.1:$http"
native_listen = "127.0.0.1:$native"
admin_listen = "127.0.0.1:$admin"
single_node = true
data_dir = "$dir"

[house.pool]
min_idle_workers = ${workers:-2}
${workers:+max_workers = $workers}
max_workers_per_namespace = ${workers:-8}
# The default retires a worker after 500 statements (a 140 ms reboot); a
# benchmark at thousands of statements a second would measure reboots.
max_queries_per_worker = 1000000
TOML
    taskset -c "$cores" "$BIN/loams-fabric" house --config "$dir/house.toml" \
        --worker-binary "$BIN/loams-house-worker" \
        ${OTLP_ENDPOINT:+--otlp-endpoint "$OTLP_ENDPOINT"} \
        > "$dir/house.log" 2>&1 &
    local pid=$!
    for _ in $(seq 1 120); do
        if curl -fsS "http://127.0.0.1:$http/ping" > /dev/null 2>&1; then
            echo "$pid"
            return 0
        fi
        sleep 0.5
    done
    echo "house $name did not answer /ping; log:" >&2
    tail -20 "$dir/house.log" >&2
    kill "$pid" 2> /dev/null || true
    return 1
}

# stop_house <pid>: stops a House this script started (by PID only).
stop_house() {
    local pid=$1
    kill "$pid" 2> /dev/null || true
    for _ in $(seq 1 40); do
        kill -0 "$pid" 2> /dev/null || return 0
        sleep 0.25
    done
    kill -9 "$pid" 2> /dev/null || true
}

# key <name>: the local user's key of House <name>.
key() { tr -d '[:space:]' < "$DATA/$1/house.key"; }

# q <name> <http_port> <sql> [body-file]: one statement over HTTP.
q() {
    local name=$1 port=$2 sql=$3 body=${4:-/dev/null}
    # POST: ClickHouse's HTTP interface makes a GET read-only.
    curl -fsS -H "X-ClickHouse-User: default" -H "X-ClickHouse-Key: $(key "$name")" \
        --data-binary "@$body" "http://127.0.0.1:$port/?query=$(urlenc "$sql")"
}

urlenc() { python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1],safe=""))' "$1"; }

# oha_json <out.json> <oha args...>: oha pinned to the client cores, JSON out.
oha_json() {
    local out=$1; shift
    taskset -c "$CLIENT_CORES" oha --no-tui --ipv4 --output-format json -o "$out" "$@"
}

# chbench <out.txt> <native_port> <name> <queries-file> <concurrency> <seconds>
chbench() {
    local out=$1 port=$2 name=$3 file=$4 conc=$5 secs=$6
    taskset -c "$CLIENT_CORES" clickhouse benchmark --host 127.0.0.1 --port "$port" \
        --user default --password "$(key "$name")" -c "$conc" -t "$secs" -d 0 -r \
        < "$file" > "$out" 2>&1 || true
}
