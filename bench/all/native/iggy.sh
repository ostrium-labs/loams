#!/usr/bin/env bash
# Iggy through its native TCP protocol, time-bounded for the combined run:
# iggy-bench (balanced producers) driving the
# iggy-server that engines.sh starts (127.0.0.1:48190), pinned to --cores.
# It measures produce only: a consumer group in a chunked run re-reads with
# --reuse-streams and reports nothing, so consume stays with the Iggy lane's
# own harness (bench/iggy).
#
# iggy-bench sizes a run by data volume, not by time, and a volume
# calibrated before the window overruns it badly under contention (166 s
# for a 60 s window in the first combined run). So this script runs it in
# chunks of about 10 s, sizing each chunk by the rate of the one before,
# until the window ends. qps = messages / window, per op. Latencies:
# message-weighted mean of the chunks' p50, worst chunk's p99 and p99.9
# (iggy-bench prints percentiles, not histograms, so they cannot be merged
# exactly).
#   depth = messages per batch (depth_kind "batch"), 100-byte messages,
#   --conns producers (default 4) on 4 partitions.
set -euo pipefail
BIN=${BQ1_ALL_BIN:-/mnt/Projects/bench-data/bq1-all/bin}
duration=30 cores="" depth=1 target=127.0.0.1:48190 start_at=0 conns=4 parts=4 size=100
while [ $# -gt 0 ]; do
  case $1 in
    --duration) duration=${2%s}; shift 2 ;;
    --cores) cores=$2; shift 2 ;;
    --depth) depth=$2; shift 2 ;;
    --target) target=$2; shift 2 ;;
    --start-at) start_at=$2; shift 2 ;;
    --conns) conns=$2; shift 2 ;;
    --ops) shift 2 ;;  # produce only (see above)
    --server-cores) echo "iggy: drives the engines.sh server; use --target" >&2; exit 2 ;;
    *) echo "iggy: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$cores" ] || { echo "iggy: --cores is required" >&2; exit 2; }
work=${BQ1_ALL_DATA:-/mnt/Projects/bench-data/bq1-all}/iggy-$$
mkdir -p "$work"; trap 'rm -rf "$work"' EXIT
chunk() { # <MB> <log>
  taskset -c "$cores" "$BIN/iggy-bench" -u iggy -p bq1-all -m "$size" -P "$depth" -T "${1}MB" \
    --reuse-streams balanced-producer -s 1 -a "$parts" -p "$conns" \
    tcp --server-address "$target" > "$2" 2>&1
}
parse() { # <log> -> "kind msgs_per_s secs p50 p99 p999 mb_per_s" lines
  python3 -I - "$1" <<'PY'
import re, sys
t = re.sub(r"\x1b\[[0-9;]*m", "", open(sys.argv[1], errors="replace").read())
for m in re.finditer(r"(Producers|Consumers) Results: Total throughput: ([\d.]+) MB/s, (\d+) messages/s.*?p50 latency: ([\d.]+) ms.*?p99 latency: ([\d.]+) ms, p999 latency: ([\d.]+) ms.*?total time: ([\d.]+) s", t):
    print(m.group(1), m.group(3), m.group(7), m.group(4), m.group(5), m.group(6), m.group(2))
PY
}
# Setup: a small warm-up chunk gives the first rate (and creates the stream).
chunk 5 "$work/warm.log"
rate=$(parse "$work/warm.log" | awk '$1 == "Producers" {print $7}')
rate=${rate:-5}
now=$(date +%s); late=0
if [ "$start_at" -gt "$now" ]; then sleep $(( start_at - now )); elif [ "$start_at" -gt 0 ]; then late=$(( now - start_at )); fi
t_start=$(date +%s.%N)
deadline=$(python3 -I -c "print($t_start + $duration)")
i=0
while :; do
  left=$(python3 -I -c "print(max(0.0, $deadline - $(date +%s.%N)))")
  python3 -I -c "import sys; sys.exit(0 if $left > 2.0 else 1)" || break
  mb=$(python3 -I -c "print(max(1, int($rate * min($left, 10.0))))")
  chunk "$mb" "$work/c$i.log"
  r=$(parse "$work/c$i.log" | awk '$1 == "Producers" {print $7}')
  [ -n "$r" ] && rate=$r
  i=$(( i + 1 ))
done
t_end=$(date +%s.%N)
python3 -I - "$work" "$i" "$t_start" "$t_end" "$depth" "$cores" "$conns" "$late" "$duration" <<'PY'
import json, re, subprocess, sys
work, n, t0, t1, depth, cores, conns, late, dur = sys.argv[1:]
t0, t1 = float(t0), float(t1)
acc = {}
errors = 0
for k in range(int(n)):
    path = f"{work}/c{k}.log"
    text = open(path, errors="replace").read()
    errors += len(re.findall(r"\bERROR\b|panicked", text))
    t = re.sub(r"\x1b\[[0-9;]*m", "", text)
    for m in re.finditer(r"(Producers|Consumers) Results: Total throughput: ([\d.]+) MB/s, (\d+) messages/s.*?p50 latency: ([\d.]+) ms.*?p99 latency: ([\d.]+) ms, p999 latency: ([\d.]+) ms.*?total time: ([\d.]+) s", t):
        a = acc.setdefault(m.group(1), {"msgs": 0.0, "w50": 0.0, "p99": 0.0, "p999": 0.0})
        msgs = int(m.group(3)) * float(m.group(7))
        a["msgs"] += msgs
        a["w50"] += msgs * float(m.group(4))
        a["p99"] = max(a["p99"], float(m.group(5)))
        a["p999"] = max(a["p999"], float(m.group(6)))
ncores = int(subprocess.run(["taskset", "-c", cores, "nproc"], capture_output=True, text=True).stdout.strip() or 1)
window = t1 - t0
for kind, op in (("Producers", "produce"), ("Consumers", "consume")):
    a = acc.get(kind)
    if not a:
        continue
    qps = a["msgs"] / window
    print(json.dumps({"engine": "iggy", "op": op, "qps": round(qps), "rps": round(qps / max(1, int(depth))),
        "p50_ms": round(a["w50"] / a["msgs"], 3) if a["msgs"] else None, "p99_ms": a["p99"], "p999_ms": a["p999"],
        "depth": int(depth), "depth_kind": "batch", "cores": ncores, "errors": errors, "conns": int(conns),
        "window_s": round(window, 1), "t_start": round(t0, 1), "t_end": round(t1, 1), "late_s": float(late),
        "chunks": int(n), "api": "iggy native TCP (iggy-bench balanced-producer, chunked to the window)",
        "note": "p50 is the message-weighted mean of chunk p50s; p99/p99.9 the worst chunk's"}))
PY
