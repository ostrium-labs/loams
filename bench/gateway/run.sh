#!/usr/bin/env bash
# BQ1-gw: measure loams-gateway on one laptop (docs/benchmarks/2026-10-28-1b-qps/gateway.md).
#
#   bench/gateway/run.sh <scenario> [label]
#
# Scenarios (each one a short hold of the bench lock; run them under
# .he0-shared/benchslot.sh):
#   http       oha against the gateway's HTTP stack (the admin listener's
#              /healthz and /v1/routes), server on 1, 2, 4 and 6 cores
#   tls        TLS 1.3 handshakes/s against the gateway's rustls ServerCerts
#   browser    the browser module (the gateway's only L7 proxy route):
#              authenticated upgrades/s and spliced echo msgs/s
#   auth       the per-connection token check, in-process
#   conns      gateway RSS with 0, 1k, 5k and 10k held browser sessions
#
# Cores (Intel Core Ultra 5 225H: 0-3 P-cores 4.9 GHz, 4-11 E-cores 4.4 GHz,
# 12-13 low-power E-cores): gateway on SERVER_CPUS, load generator on
# LOAD_CPUS, the stand-in upstream on UPSTREAM_CPUS. Disjoint by default.
set -euo pipefail

scenario=${1:?scenario: http|tls|browser|auth|conns}
label=${2:-$(git rev-parse --short HEAD)}
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
bin=${BIN_DIR:-/mnt/Projects/rust-cache/target/release}
gateway="$bin/loams-gateway"
bench="$bin/gw-bench"
SERVER_CPUS=${SERVER_CPUS:-0-5}
LOAD_CPUS=${LOAD_CPUS:-6-11}
UPSTREAM_CPUS=${UPSTREAM_CPUS:-12-13}
SECONDS_PER_RUN=${SECONDS_PER_RUN:-10}
out="$root/bench/results/bq1/gateway"
work=${BENCH_DATA:-/mnt/Projects/bench-data/bq1-gw}
mkdir -p "$out" "$work"
stamp=$(date -u +%Y%m%dT%H%M%SZ)
result="$out/$stamp-$scenario-$label.jsonl"
pids=()

cleanup() {
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
  rm -rf "$work"/run-$$
}
trap cleanup EXIT
mkdir -p "$work/run-$$"
run="$work/run-$$"

# /proc/stat's aggregate cpu line: total and iowait jiffies.
cpu_stat() { awk '/^cpu / {t=0; for (i=2;i<=NF;i++) t+=$i; print t, $6}' /proc/stat; }
last_stat=$(cpu_stat)
# Every record carries the host's state since the previous one: builds are not
# under the bench lock, so a contended point must be visible as such.
record() {
  local now; now=$(cpu_stat)
  printf '%s\n' "$1" | python3 -I -c '
import json, sys
line = sys.stdin.read()
d = json.loads(line)
t0, w0 = map(int, sys.argv[1].split()); t1, w1 = map(int, sys.argv[2].split())
d["host"] = {"loadavg_1m": float(open("/proc/loadavg").read().split()[0]),
             "iowait_pct": round(100.0 * (w1 - w0) / max(1, t1 - t0), 1)}
print(json.dumps(d))' "$last_stat" "$now" | tee -a "$result"
  last_stat=$now
}

wait_port() {
  for _ in $(seq 1 100); do
    if (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; then return 0; fi
    sleep 0.1
  done
  echo "port $1 never opened" >&2
  return 1
}

meta() {
  record "$(printf '{"meta":{"scenario":"%s","label":"%s","commit":"%s","server_cpus":"%s","load_cpus":"%s","upstream_cpus":"%s","kernel":"%s","cpu":"%s"}}' \
    "$scenario" "$label" "$(git -C "$root" rev-parse HEAD)" "$SERVER_CPUS" "$LOAD_CPUS" "$UPSTREAM_CPUS" \
    "$(uname -r)" "$(lscpu | sed -n 's/^Model name: *//p')")"
}

start_pg_gateway() { # $1 = cpu list
  cat > "$run/routes.toml" <<'TOML'
[pg]
TOML
  cat > "$run/gateway.toml" <<TOML
[admin]
listen = "127.0.0.1:19187"
[routes]
path = "$run/routes.toml"
require_generation = false
[pg]
listen = "127.0.0.1:15432"
tls_mode = "passthrough"
TOML
  taskset -c "$1" "$gateway" --config "$run/gateway.toml" > "$run/gateway.log" 2>&1 &
  pids+=($!)
  wait_port 19187
}

start_browser_gateway() { # sets $token and $gateway_pid
  local engine
  token=$(openssl rand -hex 32)
  engine=$(openssl rand -hex 32)
  printf %s "$engine" > "$run/engine-token"
  printf %s "$token" | sha256sum | cut -d' ' -f1 > "$run/clients"
  taskset -c "$UPSTREAM_CPUS" "$bench" mock-upstream --listen 127.0.0.1:19222 > /dev/null 2> "$run/upstream.log" &
  pids+=($!)
  cat > "$run/browser.toml" <<TOML
listen = "127.0.0.1:19223"
upstream = "127.0.0.1:19222"
upstream_token_file = "$run/engine-token"
client_tokens_file = "$run/clients"
max_connections = 20000
TOML
  taskset -c "$SERVER_CPUS" "$gateway" --module browser --config "$run/browser.toml" > "$run/gateway.log" 2>&1 &
  gateway_pid=$!
  pids+=($gateway_pid)
  wait_port 19222
  wait_port 19223
}

rss_kib() { awk '/^VmRSS:/ {print $2}' "/proc/$1/status"; }
cpu_ticks() { awk '{print $14 + $15}' "/proc/$1/stat"; }
# The rest of the machine during the run: builds are not under the bench lock.
record_load() { record '{"sample":"host"}'; }

ulimit -n 65536 2>/dev/null || true
meta
record_load

case "$scenario" in
  http)
    # The lean keep-alive load generator (gw-bench http): oha tops out near
    # 150-230k rps on these E-cores and would be the limit. Each record also
    # carries the cores the gateway actually used, so rps per busy gateway
    # core is a figure the load generator cannot cap. Depth 1 is one request
    # in flight per connection; depth 16 is HTTP/1.1 pipelining; `h2` is
    # HTTP/2 prior knowledge with 16 streams per connection (oha).
    for cpus in 0 0-1 0-3 "$SERVER_CPUS"; do
      start_pg_gateway "$cpus"
      gw=${pids[-1]}
      for spec in "/healthz 256 1" "/healthz 1024 1" "/healthz 256 16" "/v1/routes 256 1" "/v1/routes 256 16" "/healthz 64 h2"; do
        read -r path conns depth <<<"$spec"
        g0=$(cpu_ticks "$gw"); t0=$(date +%s.%N)
        if [ "$depth" = h2 ]; then
          json=$(taskset -c "$LOAD_CPUS" oha -z "${SECONDS_PER_RUN}s" -c "$conns" -p 16 --http2 --no-tui \
            --output-format json "http://127.0.0.1:19187$path" | python3 -I -c '
import json, sys
d = json.load(sys.stdin); s = d["summary"]
print(json.dumps({"bench": "http2", "path": sys.argv[1], "conns": int(sys.argv[2]), "streams": 16,
  "rps": s.get("requestsPerSec"), "success_rate": s.get("successRate")}))' "$path" "$conns")
        else
          json=$(taskset -c "$LOAD_CPUS" "$bench" http --addr 127.0.0.1:19187 --path "$path" \
            --conns "$conns" --depth "$depth" --seconds "$SECONDS_PER_RUN")
        fi
        g1=$(cpu_ticks "$gw"); t1=$(date +%s.%N)
        record "$(python3 -I -c '
import json, sys
d = json.loads(sys.argv[1]); busy = (int(sys.argv[3]) - int(sys.argv[2])) / int(sys.argv[6]) / (float(sys.argv[5]) - float(sys.argv[4]))
d.update({"server_cpus": sys.argv[7], "gateway_cores_busy": round(busy, 2),
          "rps_per_gateway_core": round((d.get("rps") or 0) / busy) if busy > 0 else None})
print(json.dumps(d))' "$json" "$g0" "$g1" "$t0" "$t1" "$(getconf CLK_TCK)" "$cpus")"
      done
      kill "$gw"; wait "$gw" 2>/dev/null || true; unset 'pids[-1]'
    done
    ;;
  tls)
    for cpus in 0 0-3 "$SERVER_CPUS"; do
      taskset -c "$cpus" "$bench" tls-server --listen 127.0.0.1:19443 > /dev/null 2> "$run/tls.log" &
      pids+=($!)
      wait_port 19443
      record "$(taskset -c "$LOAD_CPUS" "$bench" tls --addr 127.0.0.1:19443 --concurrency 128 --seconds "$SECONDS_PER_RUN" \
        | python3 -I -c 'import json,sys; d=json.load(sys.stdin); d["server_cpus"]=sys.argv[1]; print(json.dumps(d))' "$cpus")"
      kill "${pids[-1]}"; wait "${pids[-1]}" 2>/dev/null || true; unset 'pids[-1]'
    done
    ;;
  browser)
    start_browser_gateway
    with_busy() { # $@ = command; records its JSON with the gateway's busy cores
      local g0 t0 g1 t1 json
      g0=$(cpu_ticks "$gateway_pid"); t0=$(date +%s.%N)
      json=$("$@")
      g1=$(cpu_ticks "$gateway_pid"); t1=$(date +%s.%N)
      record "$(python3 -I -c '
import json, sys
d = json.loads(sys.argv[1]); busy = (int(sys.argv[3]) - int(sys.argv[2])) / int(sys.argv[6]) / (float(sys.argv[5]) - float(sys.argv[4]))
d["gateway_cores_busy"] = round(busy, 2)
ops = d["result"]["ops_per_sec"]
d["ops_per_gateway_core"] = round(ops / busy) if busy > 0 else None
print(json.dumps(d))' "$json" "$g0" "$g1" "$t0" "$t1" "$(getconf CLK_TCK)")"
    }
    for concurrency in 64 256; do
      with_busy taskset -c "$LOAD_CPUS" "$bench" upgrade --addr 127.0.0.1:19223 --token "$token" \
        --concurrency "$concurrency" --seconds "$SECONDS_PER_RUN"
    done
    for spec in "64 1" "512 1" "64 16"; do
      read -r conns depth <<<"$spec"
      with_busy taskset -c "$LOAD_CPUS" "$bench" pingpong --addr 127.0.0.1:19223 --token "$token" \
        --conns "$conns" --size 64 --depth "$depth" --seconds "$SECONDS_PER_RUN"
    done
    ;;
  auth)
    for tokens in 10 100 1000; do
      record "$(taskset -c 0 "$bench" auth --file "$run/clients-$tokens" --tokens "$tokens" --iterations 100000)"
    done
    ;;
  conns)
    start_browser_gateway
    sleep 1
    record "$(printf '{"bench":"conns","held":0,"gateway_rss_kib":%s}' "$(rss_kib "$gateway_pid")")"
    for n in 1000 5000 10000; do
      fifo="$run/hold-$n"
      taskset -c "$LOAD_CPUS" "$bench" hold --addr 127.0.0.1:19223 --token "$token" --conns "$n" --seconds 8 > "$fifo" &
      hold_pid=$!
      for _ in $(seq 1 600); do [ -s "$fifo" ] && break; sleep 0.1; done
      sleep 1
      rss=$(rss_kib "$gateway_pid")
      record "$(python3 -I -c 'import json,sys; d=json.loads(open(sys.argv[1]).readline()); d["gateway_rss_kib"]=int(sys.argv[2]); d["bench"]="conns"; print(json.dumps(d))' "$fifo" "$rss")"
      wait "$hold_pid" || true
      sleep 2
    done
    ;;
  *)
    echo "unknown scenario $scenario" >&2
    exit 2
    ;;
esac
record_load
echo "results: $result" >&2
