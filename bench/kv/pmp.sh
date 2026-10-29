#!/usr/bin/env bash
# A poor man's sampling profiler (no perf on this host): stack samples of a
# running process with eu-stack, aggregated by "inclusive frame" over the
# threads that were running Loams or redb code.
#   bench/kv/pmp.sh <pid> <samples> <interval-s> <out.txt>
# Sampling stops the process for a few ms per sample, so profile runs are
# never used for throughput numbers.
set -euo pipefail
pid=$1 n=$2 every=$3 out=$4
raw=$(mktemp -p "$(dirname "$out")" pmp.XXXX)
for ((i = 0; i < n; i++)); do
  eu-stack -q -p "$pid" 2>/dev/null >>"$raw" || true
  echo "=== sample" >>"$raw"
  sleep "$every"
done
# One stack per thread; keep threads whose stack has a loams_/redb frame
# (worker threads doing work, not parked), count each distinct function once
# per stack (inclusive).
awk '
  function flush() {
    if (busy) { for (f in seen) incl[f]++; total++ }
    delete seen; busy = 0
  }
  /^TID / || /^=== sample/ { flush(); next }
  /^#[0-9]+/ {
    line = $0
    sub(/^#[0-9]+ +0x[0-9a-f]+ +(in +)?/, "", line)
    sub(/ - .*/, "", line)
    gsub(/::\{closure#[0-9]+\}/, "", line)
    gsub(/<[^<>]*>/, "", line); gsub(/<[^<>]*>/, "", line); gsub(/<[^<>]*>/, "", line)
    if (line ~ /loams_|redb::|tokio::net|tikv_client/) busy = 1
    if (line != "") seen[line] = 1
  }
  END {
    flush()
    printf "busy thread-samples: %d\n", total > "/dev/stderr"
    for (f in incl) printf "%6.1f%%  %s\n", 100 * incl[f] / total, f
  }' "$raw" 2>"$out" | sort -t% -k1 -rn | head -80 >>"$out"
rm -f "$raw"
