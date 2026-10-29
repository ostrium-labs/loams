#!/usr/bin/env bash
# Graph native load for BQ1-ALL's combined run: Grafeo's own session API (graph-qps-bench
# --path native), embedded, no network. Prints one JSON line per op:
#   {engine, op, qps, requests_per_s, p50_ms, p99_ms, p999_ms, depth, cores, errors}
# qps counts lookups (depth per request for the batched op).
#
#   bench/graph/native-load.sh --duration 30 --cores 0-3 --depth 16 [--op point|hop1|hop2|hop3|recall|all]
#
# --cores: a taskset list (0-3, 4,5) to pin to, or a count (threads only; the caller pins).
# --depth 1 runs the unbatched op; depth N > 1 runs point/hop1 as N keys per statement.
set -euo pipefail
bin="${GRAPH_QPS_BENCH:-/mnt/Projects/rust-cache/target/release/graph-qps-bench}"
duration=30 cores=4 depth=1 op=point persons="${PERSONS:-200000}"
while [ $# -gt 0 ]; do
  case "$1" in
    --duration) duration=$2; shift 2 ;;
    --cores) cores=$2; shift 2 ;;
    --depth) depth=$2; shift 2 ;;
    --op) op=$2; shift 2 ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done
if [[ "$cores" =~ ^[0-9]+$ ]]; then
  threads=$cores; pin=()
else
  threads=$(python3 -c "import sys
n=0
for part in sys.argv[1].split(','):
    a,_,b=part.partition('-'); n+= (int(b)-int(a)+1) if b else 1
print(n)" "$cores")
  pin=(taskset -c "$cores")
fi
ops=("$op"); [ "$op" = all ] && ops=(point hop1 hop2 hop3 recall)
for o in "${ops[@]}"; do
  w=$o
  if [ "$depth" -gt 1 ]; then
    case "$o" in point) w=point-batch ;; hop1) w=hop1-batch ;; esac
  fi
  "${pin[@]}" "$bin" --path native --workload "$w" --batch "$depth" --threads "$threads" \
    --persons "$persons" --duration "$duration" --warmup 2 2>/dev/null \
  | python3 -c "
import json, sys
d = json.loads(sys.stdin.read().strip().splitlines()[-1])
batched = d['workload'].endswith('batch')
print(json.dumps({'engine': 'graph', 'op': '$o', 'qps': d['lookups_per_s'],
  'requests_per_s': d['qps'], 'p50_ms': d['p50_us'] / 1e3, 'p99_ms': d['p99_us'] / 1e3,
  'p999_ms': d['p999_us'] / 1e3, 'depth': d['batch'] if batched else 1,
  'cores': '$cores', 'errors': d['errors']}))"
done
