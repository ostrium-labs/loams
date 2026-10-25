#!/usr/bin/env bash
# Runs the loams-cloud suite against a throwaway TiDB (tidb-server on unistore:
# one process, no PD or TiKV), so the control-DB tests (LC2a Task 1 onward) run
# on the engine Starter runs. TiDB v8.5.8, the version the durable suite pins.
#
#   scripts/test-tidb.sh            start TiDB, run the tests, stop TiDB
#   TIDB_SERVER=/path/tidb-server   use this binary (default: tiup's v8.5.8)
#   LOAMS_CLOUD_TIDB_DIR=dir        data and log directory (default: ./.tidb)
set -euo pipefail
cd "$(dirname "$0")/.."

bin="${TIDB_SERVER:-$HOME/.tiup/components/tidb/v8.5.8/tidb-server}"
if [ ! -x "$bin" ]; then
  echo "no tidb-server at $bin: run 'tiup install tidb:v8.5.8' or set TIDB_SERVER" >&2
  exit 2
fi
dir="${LOAMS_CLOUD_TIDB_DIR:-$PWD/.tidb}"
mkdir -p "$dir"
port="${LOAMS_CLOUD_TIDB_PORT:-$(( 24000 + RANDOM % 1000 ))}"
status=$(( port + 1000 ))

"$bin" -store unistore -path "$dir/data" -host 127.0.0.1 -P "$port" -status "$status" \
  -log-file "$dir/tidb.log" >/dev/null 2>&1 &
pid=$!
cleanup() { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; rm -rf "$dir"; }
trap cleanup EXIT

for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:$status/status" >/dev/null 2>&1; then break; fi
  sleep 1
done
curl -fsS "http://127.0.0.1:$status/status" >/dev/null || { tail -20 "$dir/tidb.log" >&2; exit 1; }

LOAMS_CLOUD_TEST_TIDB="127.0.0.1:$port" node --experimental-strip-types --no-warnings=ExperimentalWarning --test "${@:-test/*.test.ts}"
