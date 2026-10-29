#!/usr/bin/env bash
# BQ1-ALL's entry point for loams-gateway (owner, 2026-10-28: "native APIs only").
#
#   bench/gateway/native-load.sh --duration 30 --cores 0-3 [--load-cores 4-7] [--depth 16]
#
# Starts loams-gateway on --cores and drives its HTTP listener from
# --load-cores. Writes one JSON line per operation to stdout:
#   {engine, op, qps, p50_ms, p99_ms, p999_ms, depth, cores, errors}
#
# What it measures, honestly: loams-gateway does not front the Loams Connect
# API yet. Its HTTP surface today is the listener every module shares (the
# admin routes on hyper). Since BQ1-gw it serves HTTP/1.1 with pipelining and
# HTTP/2 multiplexing (h2c, prior knowledge). So the ops are:
#   http2-mux        HTTP/2, `depth` streams per connection (oha)
#   http1-pipelined  HTTP/1.1, `depth` requests in flight per connection
#   http1            HTTP/1.1, one request in flight (depth 1, for reference)
# The engine's own Connect API is the `loams` engine lane's to measure.
set -euo pipefail

duration=30 cores=0-3 load_cores="" depth=16 conns=256
while [ $# -gt 0 ]; do
  case "$1" in
    --duration) duration=$2; shift 2 ;;
    --cores) cores=$2; shift 2 ;;
    --load-cores) load_cores=$2; shift 2 ;;
    --depth) depth=$2; shift 2 ;;
    --conns) conns=$2; shift 2 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done
bin=${BIN_DIR:-/mnt/Projects/rust-cache/target/release}
port=${GATEWAY_ADMIN_PORT:-19287}
if [ -z "$load_cores" ]; then
  # The cores the gateway does not have, up to as many as it has.
  load_cores=$(python3 -I -c '
import sys
used = set()
for part in sys.argv[1].split(","):
    a, _, b = part.partition("-"); used.update(range(int(a), int(b or a) + 1))
free = [c for c in range(14) if c not in used][:max(1, len(used))]
print(",".join(map(str, free)))' "$cores")
fi
ncores=$(python3 -I -c '
import sys
n = 0
for part in sys.argv[1].split(","):
    a, _, b = part.partition("-"); n += int(b or a) - int(a) + 1
print(n)' "$cores")
work=${BENCH_DATA:-/mnt/Projects/bench-data/bq1-gw}/native-$$
mkdir -p "$work"
gw=""
cleanup() { [ -n "$gw" ] && kill "$gw" 2>/dev/null; wait 2>/dev/null || true; rm -rf "$work"; }
trap cleanup EXIT

printf '[pg]\n' > "$work/routes.toml"
cat > "$work/gateway.toml" <<TOML
[admin]
listen = "127.0.0.1:$port"
[routes]
path = "$work/routes.toml"
require_generation = false
[pg]
listen = "127.0.0.1:$((port + 1))"
tls_mode = "passthrough"
TOML
taskset -c "$cores" "$bin/loams-gateway" --config "$work/gateway.toml" > "$work/gw.log" 2>&1 &
gw=$!
for _ in $(seq 1 100); do (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && break; sleep 0.1; done

emit() { # op depth json-from-tool
  python3 -I -c '
import json, sys
op, depth, ncores, raw = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
d = json.loads(raw)
if "summary" in d:  # oha
    s = d["summary"]; p = d.get("latencyPercentiles", {})
    total = sum((d.get("statusCodeDistribution") or {}).values()) or 0
    ok = (s.get("successRate") or 0)
    line = {"qps": s.get("requestsPerSec"), "p50_ms": (p.get("p50") or 0) * 1000,
            "p99_ms": (p.get("p99") or 0) * 1000, "p999_ms": (p.get("p99.9") or 0) * 1000,
            "errors": round(total * (1 - ok)) if total else (0 if ok == 1 else None)}
else:  # gw-bench http: latency is the sampled batch round trip
    l = d["batch_round_trip_sampled"]
    line = {"qps": d["rps"], "p50_ms": l["p50_us"] / 1000, "p99_ms": l["p99_us"] / 1000,
            "p999_ms": l["p999_us"] / 1000, "errors": d["errors"]}
out = {"engine": "gateway", "op": op}
out.update(line)
out.update({"depth": depth, "cores": ncores})
print(json.dumps(out))' "$1" "$2" "$ncores" "$3"
}

url="http://127.0.0.1:$port/healthz"
emit http2-mux "$depth" "$(taskset -c "$load_cores" oha -z "${duration}s" -c 64 -p "$depth" --http2 \
  --no-tui --output-format json "$url")"
emit http1-pipelined "$depth" "$(taskset -c "$load_cores" "$bin/gw-bench" http \
  --addr "127.0.0.1:$port" --path /healthz --conns "$conns" --depth "$depth" --seconds "$duration")"
emit http1 1 "$(taskset -c "$load_cores" "$bin/gw-bench" http \
  --addr "127.0.0.1:$port" --path /healthz --conns "$conns" --depth 1 --seconds "$duration")"
