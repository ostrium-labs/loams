#!/usr/bin/env bash
# Loams Meter's native load for BQ1-ALL's combined run (bench-1b-qps.md,
# "Native APIs only"): the gate and sink Rust APIs that products link,
# driven at the same time on disjoint halves of the given cores.
#
#   bench/meter/native-load.sh --duration 60 --cores 4-11 --depth 64
#
#   --duration S   seconds (default 30)
#   --cores LIST   a taskset list (0-5, 2,3,8-11) or a count N (= cores 0..N-1).
#                  The first half runs `Gate::check_many` (LeaseGate, 10,000
#                  subjects on lease hits), the second half `MeterSink::emit_batch`
#                  (BestEffortSink, D365, with a producer that only counts).
#                  One core: both share it.
#   --depth N      requests per check_many and events per emit_batch
#                  (1 = unpipelined `check` and `emit`; default 64)
#
# Prints one JSON line per op to stdout:
#   {"engine":"meter","op":"gate.check"|"sink.emit","qps":…,"p50_ms":…,
#    "p99_ms":…,"p999_ms":…,"depth":N,"cores":K,"errors":…}
# qps counts requests (decisions or events), not batch calls. The latencies
# are those of one API call: at depth N, a batch call that each of its N
# requests waits for. errors = refused decisions or dropped events.
# Needs the release build (bench/meter/run.sh build).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
duration=30 cores="" depth=64
while [[ $# -gt 0 ]]; do
  case "$1" in
    --duration) duration=$2; shift 2 ;;
    --cores) cores=$2; shift 2 ;;
    --depth) depth=$2; shift 2 ;;
    *) echo "unknown flag $1" >&2; exit 2 ;;
  esac
done
if [[ -z "$cores" ]]; then cores="0-$(( $(nproc) - 1 ))"; fi
if [[ "$cores" =~ ^[0-9]+$ ]]; then cores="0-$(( cores - 1 ))"; fi

# Expand the list, then split it in two halves.
mapfile -t list < <(python3 - "$cores" <<'EOF'
import sys
out = []
for part in sys.argv[1].split(","):
    if "-" in part:
        a, b = part.split("-"); out += range(int(a), int(b) + 1)
    elif part:
        out.append(int(part))
print("\n".join(map(str, out)))
EOF
)
n=${#list[@]}
if (( n == 1 )); then
  gate_cores=${list[0]} sink_cores=${list[0]}; gate_n=1 sink_n=1
else
  half=$(( n / 2 ))
  gate_cores=$(IFS=,; echo "${list[*]:0:half}"); gate_n=$half
  sink_cores=$(IFS=,; echo "${list[*]:half}"); sink_n=$(( n - half ))
fi

target="$(cd "$here" && cargo metadata --offline --format-version 1 --no-deps \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
loadgen="$target/release/meter-loadgen"
[[ -x "$loadgen" ]] || { echo "build first: bench/meter/run.sh build" >&2; exit 1; }

gate_out=$(mktemp -p "$here" .gate.XXXXXX) sink_out=$(mktemp -p "$here" .sink.XXXXXX)
trap 'rm -f "$gate_out" "$sink_out"' EXIT
taskset -c "$gate_cores" "$loadgen" gate --threads "$gate_n" --cores "$gate_cores" \
  --subjects 10000 --depth "$depth" --secs "$duration" > "$gate_out" &
gate_pid=$!
# The sink's flush thread runs within the sink's cores.
taskset -c "$sink_cores" "$loadgen" sink --threads "$sink_n" --cores "$sink_cores" \
  --subjects 10000 --depth "$depth" --secs "$duration" > "$sink_out" &
sink_pid=$!
wait "$gate_pid" "$sink_pid"

python3 - "$gate_out" "$sink_out" "$depth" <<'EOF'
import json, sys
gate, sink = (json.load(open(p)) for p in sys.argv[1:3])
depth = int(sys.argv[3])
def line(op, d, qps, lat, cores):
    return {"engine": "meter", "op": op, "qps": round(qps),
            "p50_ms": lat["p50_ns"] / 1e6, "p99_ms": lat["p99_ns"] / 1e6,
            "p999_ms": lat["p999_ns"] / 1e6, "depth": depth, "cores": cores,
            "errors": d.get("errors", 0), "system": d.get("system", {})}
print(json.dumps(line("gate.check", gate, gate["ops_per_sec"], gate["latency"], gate["threads"])))
print(json.dumps(line("sink.emit", sink, sink["produced_per_sec"], sink["emit_latency"], sink["threads"])))
EOF
