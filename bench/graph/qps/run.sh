#!/usr/bin/env bash
# BQ1 graph runs. Every measurement takes the exclusive bench lock (benchslot.sh) and is pinned
# with taskset. The headline path is Grafeo's native session API (--path native); --path loams
# (Loams's statement gate in front) and --path gate (the gate before its cache) are footnotes.
# Results: bench/results/bq1/graph/<path>-<workload>-s<shards>-t<threads>-d<depth>[-TAG].json
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
bin="${GRAPH_QPS_BENCH:-/mnt/Projects/rust-cache/target/release/graph-qps-bench}"
slot="${BENCHSLOT:-$HOME/Documents/Ostriumlabs/loams-wt/.he0-shared/benchslot.sh}"
out="$root/bench/results/bq1/graph"; mkdir -p "$out"
persons="${PERSONS:-200000}"; duration="${DURATION:-20}"

run() { # run <path> <workload> <shards> <threads> <cpus> <depth>
  local f="$out/$1-$2-s$3-t$4-d$6${TAG:+-$TAG}.json"
  "$slot" bq1-graph-git taskset -c "$5" "$bin" --path "$1" --workload "$2" --shards "$3" \
    --threads "$4" --batch "$6" --persons "$persons" --duration "$duration" > "$f"
  cat "$f"
}

case "${1:-all}" in
  peak)
    for w in point hop1 hop2 hop3 recall mix; do run native "$w" 1 6 0-5 1; done
    for d in 16 64; do
      run native point-batch 1 6 0-5 "$d"
      run native hop1-batch 1 6 0-5 "$d"
    done
    ;;
  loams)
    # Loams's gate in front of the native call: `gate` is the gate before its verdict cache,
    # `loams` the gate with it (before/after of the BQ1 fix).
    for w in point hop1 recall; do
      run gate "$w" 1 6 0-5 1
      run loams "$w" 1 6 0-5 1
    done
    ;;
  scaling)
    for s in 1 2 4; do run native "${2:-point}" "$s" $((3 * s)) "0-$((3 * s - 1))" 1; done
    ;;
  all) "$0" peak; "$0" loams; "$0" scaling point; "$0" scaling hop2 ;;
esac
