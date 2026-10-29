#!/usr/bin/env bash
# BQ1-meter measurements. Each phase is one hold of the exclusive bench lock
# (.he0-shared/benchslot.sh, at most 15 minutes), so run them one at a time:
#
#   bench/meter/run.sh build            # outside the lock (buildslot)
#   bench/meter/run.sh criterion        # micro-benches, pinned to P-core 0
#   bench/meter/run.sh gate             # decisions/s, 1/2/4/8 E-cores
#   bench/meter/run.sh sink             # MeterSink events/s, 1/2/4 E-cores
#   bench/meter/run.sh agent            # agent → StreamSink → mock produce route
#
# CPU (Core Ultra 5 225H, no SMT): cores 0-3 are P-cores (4.9 GHz), 4-11
# E-cores (4.4 GHz), 12-13 low-power E-cores. Scaling runs use the eight
# uniform E-cores; the agent's mock server gets its own four (8-11).
# Results: bench/results/bq1/meter/*.json (small, one object per run).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
shared="${SHARED:-$HOME/Documents/Ostriumlabs/loams-wt/.he0-shared}"
out="$repo/bench/results/bq1/meter"
data="${METER_BENCH_DIR:-/mnt/Projects/bench-data/bq1-meter}"
target="$(cd "$here" && cargo metadata --offline --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
# BIN_DIR runs saved binaries (the pre-fix build is kept in
# $data/before-bin); DEPTHS lists the pipelined depths (empty: depth 1 only,
# which is all the pre-fix load generator supports).
bin_dir="${BIN_DIR:-$target/release}"
loadgen="$bin_dir/meter-loadgen"
depths="${DEPTHS-16 64}"
secs="${SECS:-10}"
label="${LABEL:-before}"   # before / after the hot-path fixes in this PR
crit="$data/criterion-$label"
mkdir -p "$out" "$data"
export METER_BENCH_DIR="$data"

slot() { "$shared/benchslot.sh" bq1-meter "$@"; }

# The quiet window (bench-1b-qps.md): inside the hold, stop new builds for at
# most 20 minutes, wait for running rustc to finish and the load average to
# fall below 2 (giving up after 10 minutes, which the results then show in
# their loadavg), measure, and lift the window.
quiet_prelude='
quiet="'"$shared"'/quiet"
echo $(( $(date +%s) + 1200 )) > "$quiet"
trap "rm -f \"$quiet\"" EXIT
for _ in $(seq 120); do
  if ! pgrep -x rustc >/dev/null && awk "{exit !(\$1 < 2)}" /proc/loadavg; then break; fi
  sleep 5
done
echo "quiet window: load $(cut -d" " -f1-3 /proc/loadavg)" >&2
'


run_loadgen() { # name, taskset cores, args...
  local name=$1 cpus=$2; shift 2
  taskset -c "$cpus" "$loadgen" "$@" --secs "$secs" | tee "$out/$name-$label.json"
}

case "${1:-}" in
build)
  (cd "$here" && "$shared/buildslot.sh" cargo build --release -j 4 --offline --bins --benches \
    && "$shared/buildslot.sh" cargo bench --offline -j 4 --no-run)
  ;;
criterion)
  # The built bench executables, run directly: `cargo bench` would rebuild
  # inside the lock (and from whatever the tree holds now). Criterion writes
  # its estimates under the working directory's target/criterion.
  bins=""
  for b in core gate agent; do
    if [[ -n "${BIN_DIR:-}" ]]; then
      bins="$bins $BIN_DIR/$b"
    else
      bins="$bins $(ls -t "$target"/release/deps/$b-* | grep -v '\.d$' | head -1)"
    fi
  done
  mkdir -p "$crit"
  slot bash -c "cd '$crit' && for x in $bins; do taskset -c 0 \$x --bench --noplot; done"
  python3 - "$crit/target/criterion" "$out/criterion-$label.json" <<'EOF'
import json, os, sys
root, dest = sys.argv[1], sys.argv[2]
rows = {}
for group in ("event", "aggregate", "lease", "gate", "agent"):
    gdir = os.path.join(root, group)
    if not os.path.isdir(gdir):
        continue
    for bench in sorted(os.listdir(gdir)):
        est = os.path.join(gdir, bench, "new", "estimates.json")
        bm = os.path.join(gdir, bench, "new", "benchmark.json")
        if not os.path.exists(est):
            continue
        e = json.load(open(est))
        meta = json.load(open(bm)) if os.path.exists(bm) else {}
        elems = (meta.get("throughput") or {}).get("Elements") or 1
        ns = e["mean"]["point_estimate"]
        rows[f"{group}/{bench}"] = {
            "ns_per_iter": round(ns, 1),
            "elements_per_iter": elems,
            "ns_per_element": round(ns / elems, 2),
            "elements_per_sec": round(elems * 1e9 / ns),
            "ci95_ns": [round(e["mean"]["confidence_interval"]["lower_bound"], 1),
                        round(e["mean"]["confidence_interval"]["upper_bound"], 1)],
        }
json.dump({"lane": "bq1-meter", "pinned_core": 0, "benches": rows}, open(dest, "w"), indent=1)
print(json.dumps(rows, indent=1))
EOF
  ;;
gate)
  slot bash -c "
    set -e
    $quiet_prelude
    $(declare -f run_loadgen); loadgen='$loadgen'; out='$out'; secs='$secs'; label='$label'
    run_loadgen gate-pcore-t1-s10k 0 gate --threads 1 --cores 0 --subjects 10000
    for d in $depths; do
      run_loadgen gate-pcore-t1-s10k-d\$d 0 gate --threads 1 --cores 0 --subjects 10000 --depth \$d
    done
    for t in 1 2 4 8; do
      last=\$((3 + t))
      run_loadgen gate-ecore-t\$t-s10k 4-\$last gate --threads \$t --cores 4-\$last --subjects 10000
      run_loadgen gate-ecore-t\$t-hot 4-\$last gate --threads \$t --cores 4-\$last --subjects 1
      for d in $depths; do
        run_loadgen gate-ecore-t\$t-s10k-d\$d 4-\$last gate --threads \$t --cores 4-\$last --subjects 10000 --depth \$d
        run_loadgen gate-ecore-t\$t-hot-d\$d 4-\$last gate --threads \$t --cores 4-\$last --subjects 1 --depth \$d
      done
    done"
  ;;
sink)
  slot bash -c "
    set -e
    $quiet_prelude
    $(declare -f run_loadgen); loadgen='$loadgen'; out='$out'; secs='$secs'; label='$label'
    for t in 1 2 4; do
      last=\$((3 + t))
      # The sink's flush thread is unpinned within the taskset: give it core 11.
      run_loadgen sink-ecore-t\$t 4-\$last,11 sink --threads \$t --cores 4-\$last --subjects 10000
      for d in $depths; do
        run_loadgen sink-ecore-t\$t-d\$d 4-\$last,11 sink --threads \$t --cores 4-\$last --subjects 10000 --depth \$d
      done
    done"
  ;;
agent)
  slot bash -c "
    set -e
    $quiet_prelude
    $(declare -f run_loadgen); loadgen='$loadgen'; out='$out'; secs='$secs'; label='$label'
    for t in 1 2 4; do
      last=\$((3 + t))
      run_loadgen agent-spool-a\$t 4-\$last,8-11 agent --threads \$t --cores 4-\$last --server-cores 8-11 --batch 256 --dir '$data'
      run_loadgen agent-nospool-a\$t 4-\$last,8-11 agent --threads \$t --cores 4-\$last --server-cores 8-11 --batch 256 --no-spool --dir '$data'
    done"
  rm -rf "$data"/agent-*
  ;;
*)
  sed -n '2,13p' "$0"; exit 2 ;;
esac
