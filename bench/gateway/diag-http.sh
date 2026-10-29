#!/usr/bin/env bash
# BQ1-gw: is the HTTP curve limited by the gateway or by the load generator?
#
#   bench/gateway/diag-http.sh <label> [server cpu lists...]
#
# For each server CPU set, runs oha at several connection counts and records,
# next to the requests/s, how much CPU the gateway and oha used during the run
# (from /proc/<pid>/stat, in cores busy). A gateway below its core budget while
# oha sits at its own is a load-generator limit. `rps_per_gateway_core` is the
# efficiency figure that does not depend on the load generator.
set -euo pipefail
label=${1:?label}; shift
server_sets=("${@:-0 0-1 0-3 0-5}")
[ $# -gt 0 ] && server_sets=("$@") || server_sets=(0 0-1 0-3 0-5)
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
bin=${BIN_DIR:-/mnt/Projects/rust-cache/target/release}
DUR=${DUR:-8}
CONNS=${CONNS:-"256 512 1024"}
PATHS=${PATHS:-"healthz v1/routes"}
out="$root/bench/results/bq1/gateway"
mkdir -p "$out"
result="$out/$(date -u +%Y%m%dT%H%M%SZ)-httpdiag-$label.jsonl"
work=${BENCH_DATA:-/mnt/Projects/bench-data/bq1-gw}/diag-$$
mkdir -p "$work"
gw=""
cleanup() { [ -n "$gw" ] && kill "$gw" 2>/dev/null; wait 2>/dev/null || true; rm -rf "$work"; }
trap cleanup EXIT
ticks() { awk '{print $14 + $15}' "/proc/$1/stat"; }
hz=$(getconf CLK_TCK)

printf '[pg]\n' > "$work/routes.toml"
cat > "$work/gateway.toml" <<TOML
[admin]
listen = "127.0.0.1:19187"
[routes]
path = "$work/routes.toml"
require_generation = false
[pg]
listen = "127.0.0.1:15432"
tls_mode = "passthrough"
TOML

for cpus in "${server_sets[@]}"; do
  load=${LOAD_CPUS:-}
  if [ -z "$load" ]; then
    # Every core the gateway does not have goes to the load generator.
    load=$(python3 -I -c '
import sys
used=set()
for part in sys.argv[1].split(","):
    a,_,b=part.partition("-"); used.update(range(int(a), int(b or a)+1))
print(",".join(str(c) for c in range(14) if c not in used))' "$cpus")
  fi
  taskset -c "$cpus" "$bin/loams-gateway" --config "$work/gateway.toml" > "$work/gw.log" 2>&1 &
  gw=$!
  for _ in $(seq 1 100); do (exec 3<>/dev/tcp/127.0.0.1/19187) 2>/dev/null && break; sleep 0.1; done
  for path in $PATHS; do
    for c in $CONNS; do
      g0=$(ticks "$gw")
      taskset -c "$load" oha -z "${DUR}s" -c "$c" --no-tui --output-format json \
        "http://127.0.0.1:19187/$path" > "$work/oha.json" &
      oha=$!
      sleep 1
      o0=$(ticks "$oha"); t0=$(date +%s.%N)
      sleep $((DUR - 2))
      o1=$(ticks "$oha"); t1=$(date +%s.%N)
      wait "$oha"
      g1=$(ticks "$gw")
      python3 -I - "$work/oha.json" "$cpus" "$load" "$path" "$c" "$g0" "$g1" "$o0" "$o1" "$t0" "$t1" "$hz" "$DUR" <<'PY' | tee -a "$result"
import json, sys
f, cpus, load, path, c, g0, g1, o0, o1, t0, t1, hz, dur = sys.argv[1:]
d = json.load(open(f)); s = d["summary"]; p = d.get("latencyPercentiles", {})
hz = float(hz)
gw_cores = (int(g1) - int(g0)) / hz / float(dur)
oha_cores = (int(o1) - int(o0)) / hz / (float(t1) - float(t0))
rps = s["requestsPerSec"]
print(json.dumps({"bench": "httpdiag", "path": path, "server_cpus": cpus, "load_cpus": load,
  "conns": int(c), "rps": round(rps), "gateway_cores_busy": round(gw_cores, 2),
  "oha_cores_busy": round(oha_cores, 2),
  "rps_per_gateway_core": round(rps / gw_cores) if gw_cores > 0 else None,
  "p50_ms": round((p.get("p50") or 0) * 1000, 3), "p99_ms": round((p.get("p99") or 0) * 1000, 3)}))
PY
    done
  done
  kill "$gw"; wait "$gw" 2>/dev/null || true; gw=""
done
echo "results: $result" >&2
