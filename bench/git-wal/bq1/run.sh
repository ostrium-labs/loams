#!/usr/bin/env bash
# BQ1 Git-on-S3 runs: ref updates (push), ref reads, pack reads (fetch), at depth 1 and depth N
# (refs per push, refs per snapshot, packs per request), each under the exclusive bench lock and
# pinned with taskset. The store is a file store on /mnt/Projects (ext4, NVMe), with modelled
# remote-store latency where named `s3model` (--put-delay-ms 20 / --get-delay-ms 10); no cloud
# bucket is used. Results: bench/results/bq1/git/<name>.json, one JSON line each.
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
bin="${GIT_WAL_BENCH:-/mnt/Projects/rust-cache/target/release/git-wal-bench}"
slot="${BENCHSLOT:-$HOME/Documents/Ostriumlabs/loams-wt/.he0-shared/benchslot.sh}"
data="${DATA:-/mnt/Projects/bench-data/bq1-graph-git/git}"
out="$root/bench/results/bq1/git"; mkdir -p "$out"
dur="${DURATION:-15}"; cpus="${CPUS:-0-5}"

run() { # run <name> <args...>
  local name=$1; shift
  rm -rf "$data"; mkdir -p "$data"
  "$slot" bq1-graph-git taskset -c "$cpus" "$bin" --store "file://$data" --duration "$dur" "$@" \
    > "$out/$name.json"
  rm -rf "$data"
  cat "$out/$name.json"
}

case "${1:-all}" in
  push)
    for r in 1 2 3; do
      run "push-d1-run$r" --mode push --pushers 32
      run "push-d16-run$r" --mode push --pushers 32 --batch 16
    done
    run push-d1-pack256k --mode push --pushers 32 --pack-bytes 262144
    run push-d1-s3model --mode push --pushers 32 --put-delay-ms 20
    run push-d16-s3model --mode push --pushers 32 --put-delay-ms 20 --batch 16
    ;;
  read)
    for n in 100 1000 10000; do
      run "refread-cached-d1-refs$n" --mode ref-read-cached --pushers 32 --refs "$n"
    done
    run refread-cached-d16-refs1000 --mode ref-read-cached --pushers 32 --refs 1000 --batch 16
    run refread-latest-d1 --mode ref-read --pushers 32 --refs 1000
    run refread-latest-d16 --mode ref-read --pushers 32 --refs 1000 --batch 16
    run refread-latest-d1-s3model --mode ref-read --pushers 32 --refs 1000 --get-delay-ms 10
    run refread-latest-d16-s3model --mode ref-read --pushers 32 --refs 1000 --get-delay-ms 10 --batch 16
    ;;
  fetch)
    run fetch-d1-64k --mode fetch --pushers 32 --refs 10 --pack-bytes 65536 --packs 64
    run fetch-d8-64k --mode fetch --pushers 32 --refs 10 --pack-bytes 65536 --packs 64 --batch 8
    run fetch-d1-1m --mode fetch --pushers 32 --refs 10 --pack-bytes 1048576 --packs 16
    run fetch-d1-64k-s3model --mode fetch --pushers 32 --refs 10 --pack-bytes 65536 --packs 64 --get-delay-ms 10
    run fetch-d8-64k-s3model --mode fetch --pushers 32 --refs 10 --pack-bytes 65536 --packs 64 --get-delay-ms 10 --batch 8
    ;;
  scaling)
    for r in 1 2 4; do
      run "push-scale-r$r" --mode push --repos "$r" --pushers $((32 * r))
      run "refread-cached-scale-r$r" --mode ref-read-cached --repos "$r" --pushers $((32 * r)) --refs 1000
    done
    ;;
  all) for p in push read fetch scaling; do "$0" "$p"; done ;;
esac
