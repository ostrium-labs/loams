#!/usr/bin/env bash
# Git-on-S3 native load for BQ1-ALL's combined run: the bucket WAL and pack store driven
# directly (git-wal-bench; no git processes, no smart-HTTP). Prints one JSON line per op:
#   {engine, op, qps, requests_per_s, p50_ms, p99_ms, p999_ms, depth, cores, errors}
# qps counts ref updates / ref reads / pack reads (depth per request).
#
#   bench/git/native-load.sh --duration 30 --cores 4-5 --depth 8 [--op push|ref-read|fetch|all]
#
# --cores: a taskset list or a count (the caller pins). The store is a file store under
# $DATA (default /mnt/Projects/bench-data/bq1-all/git), emptied before and after each op.
set -euo pipefail
bin="${GIT_WAL_BENCH:-/mnt/Projects/rust-cache/target/release/git-wal-bench}"
data="${DATA:-/mnt/Projects/bench-data/bq1-all/git}"
duration=30 cores=2 depth=1 op=all
while [ $# -gt 0 ]; do
  case "$1" in
    --duration) duration=$2; shift 2 ;;
    --cores) cores=$2; shift 2 ;;
    --depth) depth=$2; shift 2 ;;
    --op) op=$2; shift 2 ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done
pin=(); [[ "$cores" =~ ^[0-9]+$ ]] || pin=(taskset -c "$cores")
ops=("$op"); [ "$op" = all ] && ops=(push ref-read fetch)
for o in "${ops[@]}"; do
  case "$o" in
    push) args=(--mode push --pushers 32) ;;
    ref-read) args=(--mode ref-read-cached --pushers 32 --refs 1000) ;;
    fetch) args=(--mode fetch --pushers 32 --refs 10 --pack-bytes 65536 --packs 64) ;;
    *) echo "unknown op $o" >&2; exit 2 ;;
  esac
  rm -rf "$data"; mkdir -p "$data"
  "${pin[@]}" "$bin" --store "file://$data" --duration "$duration" --batch "$depth" "${args[@]}" \
    2>/dev/null \
  | python3 -c "
import json, sys
d = json.loads(sys.stdin.read().strip().splitlines()[-1])
if 'commits_per_s' in d:
    out = {'qps': d['ref_updates_per_s'], 'requests_per_s': d['commits_per_s'],
           'p50_ms': d['commit_p50_ms'], 'p99_ms': d['commit_p99_ms'], 'p999_ms': d['commit_p999_ms']}
else:
    out = {'qps': d['ops_per_s'], 'requests_per_s': d['requests_per_s'],
           'p50_ms': d['p50_ms'], 'p99_ms': d['p99_ms'], 'p999_ms': d['p999_ms']}
print(json.dumps({'engine': 'git', 'op': '$o', **out, 'depth': $depth,
  'cores': '$cores', 'errors': d['failed']}))"
  rm -rf "$data"
done
