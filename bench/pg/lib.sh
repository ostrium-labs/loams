# shellcheck shell=bash
# BQ1-pg harness library (bench/pg; docs/benchmarks/2026-10-28-1b-qps/pg.md).
#
# Core layout on the 14-core laptop (lscpu: 0-13), fixed for every run so
# the numbers compare:
#   PG_CORES       0-5    the Postgres server (stock or Loams Postgres compute)
#   GW_CORES       6-7    loams-gateway (the Postgres front door)
#   CLIENT_CORES   8-11   pgbench
#   STORAGE_CORES  12-13  the Loams Postgres storage tier (pageserver,
#                         safekeeper, broker) in compute.sh
# Each set is disjoint; scaling.sh narrows PG_CORES or GW_CORES.
#
# Data never goes to /tmp: the stock server's PGDATA is a tmpfs inside its
# container (RAM, deleted with it), and pgbench's sampled logs go to
# $BQ1_DATA (default /mnt/Projects/bench-data/bq1-pg), removed after each
# run once its percentiles are computed.
set -euo pipefail

BQ1_HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
BQ1_ROOT=$(cd "$BQ1_HERE/../.." && pwd)
: "${BQ1_DATA:=/mnt/Projects/bench-data/bq1-pg}"
: "${BQ1_RESULTS:=$BQ1_ROOT/bench/results/bq1/pg}"
: "${PG_CORES:=0-5}" "${GW_CORES:=6-7}" "${CLIENT_CORES:=8-11}" "${STORAGE_CORES:=12-13}"
# The pinned Postgres image of the compute harness (conformance/compute/pins.env
# PSQL_IMAGE): the stock server and pgbench 17.10.
: "${PG_IMAGE:=docker.io/library/postgres:17.10@sha256:7958605b474b3d264a969cb3a123d6aa00ad1e1fe9da8a69984dabb704d93317}"
: "${PG_PORT:=55601}" "${GW_PORT:=55610}"
: "${GW_BIN:=}"
: "${DURATION:=30}" "${WARMUP:=5}" "${SCALE:=50}"
# pgbench logs one transaction in SAMPLE (per-transaction latencies for the
# percentiles; a full log at 200k tx/s would be gigabytes).
: "${SAMPLE:=0.01}"
BQ1_HOST=pg.bench.test
BQ1_PW=${BQ1_PW:-bq1-bench-only}
BQ1_USER=${BQ1_USER:-postgres}
# Extra `podman run` arguments for pgbench (compute.sh joins the compute's
# network namespace instead of the host's).
BQ1_CLIENT_NET=${BQ1_CLIENT_NET:---network=host}
mkdir -p "$BQ1_DATA" "$BQ1_DATA/scripts" "$BQ1_RESULTS"

bq1_log() { echo "bq1-pg: $*" >&2; }

# count_cores <list>: how many CPUs a taskset list names (0-5,8 -> 7).
count_cores() {
  python3 -c '
import sys
n = 0
for part in sys.argv[1].split(","):
    a, _, b = part.partition("-")
    n += int(b or a) - int(a) + 1
print(n)' "$1"
}

# cpu_snapshot <list>: "busy total" jiffies summed over those CPUs (/proc/stat).
cpu_snapshot() {
  python3 - "$1" <<'PY'
import sys
want = set()
for part in sys.argv[1].split(","):
    a, _, b = part.partition("-")
    want.update(range(int(a), int(b or a) + 1))
busy = total = 0
for line in open("/proc/stat"):
    f = line.split()
    if f[0].startswith("cpu") and f[0] != "cpu" and int(f[0][3:]) in want:
        v = list(map(int, f[1:8]))
        idle = v[3] + v[4]
        total += sum(v)
        busy += sum(v) - idle
print(busy, total)
PY
}

# certs: a throw-away CA and a leaf for pg.bench.test and *.bench.test,
# shared by the stock server and the gateway (the terminate path checks that
# the gateway holds the upstream's certificate). Test-only, never reused.
bq1_certs() {
  local d=$BQ1_DATA/certs
  [ -s "$d/leaf.crt" ] && return 0
  mkdir -p "$d"
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 3 \
    -subj "/CN=bq1 bench CA" -keyout "$d/ca.key" -out "$d/ca.crt" 2>/dev/null
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
    -subj "/CN=$BQ1_HOST" -keyout "$d/leaf.key" -out "$d/leaf.csr" 2>/dev/null
  printf 'subjectAltName=DNS:%s,DNS:*.bench.test\n' "$BQ1_HOST" >"$d/ext.cnf"
  openssl x509 -req -in "$d/leaf.csr" -CA "$d/ca.crt" -CAkey "$d/ca.key" -CAcreateserial \
    -days 3 -extfile "$d/ext.cnf" -out "$d/leaf.crt" 2>/dev/null
  chmod 0644 "$d"/*.crt "$d"/*.key
}

# pg_start <name> <cores> <port>: the stock Postgres 17.10 server with TLS,
# PGDATA on a tmpfs; returns when it accepts connections.
pg_start() {
  local name=$1 cores=$2 port=$3
  bq1_certs
  podman rm -f "$name" >/dev/null 2>&1 || true
  # The key must belong to the server's user with mode 0600, so it is
  # copied in at start rather than bind-mounted.
  podman run -d --name "$name" --network host --cpuset-cpus "$cores" \
    --tmpfs /var/lib/postgresql/data:rw,size=2g,mode=0700 \
    -v "$BQ1_DATA/certs:/certs-src:ro" -e POSTGRES_PASSWORD="$BQ1_PW" \
    --entrypoint bash "$PG_IMAGE" -ec '
      install -d -o postgres -g postgres -m 0700 /var/lib/postgresql/tls
      install -o postgres -g postgres -m 0600 /certs-src/leaf.key /var/lib/postgresql/tls/server.key
      install -o postgres -g postgres -m 0644 /certs-src/leaf.crt /var/lib/postgresql/tls/server.crt
      exec docker-entrypoint.sh postgres "$@"' bq1 \
    -c port="$port" -c listen_addresses=127.0.0.1 -c max_connections=400 \
    -c shared_buffers=2GB -c synchronous_commit=off -c ssl=on \
    -c ssl_cert_file=/var/lib/postgresql/tls/server.crt \
    -c ssl_key_file=/var/lib/postgresql/tls/server.key >/dev/null
  local _
  for _ in $(seq 120); do
    if podman exec "$name" pg_isready -q -h 127.0.0.1 -p "$port" 2>/dev/null; then
      # The image's init restarts the server once; wait for the final one.
      sleep 1
      podman exec "$name" pg_isready -q -h 127.0.0.1 -p "$port" && return 0
    fi
    sleep 0.5
  done
  podman logs --tail 30 "$name" >&2
  return 1
}

pg_stop() { podman rm -f "$1" >/dev/null 2>&1 || true; }

# gw_start <cores> <upstream host:port>: loams-gateway's Postgres module
# (tls_mode auto: splice SSLRequest clients, terminate direct-TLS ones) on
# GW_PORT, routing pg.bench.test to the upstream. Sets GW_PID.
gw_start() {
  local cores=$1 upstream=$2 d=$BQ1_DATA/gw
  [ -x "$GW_BIN" ] || { bq1_log "GW_BIN is not an executable: $GW_BIN"; return 1; }
  bq1_certs
  mkdir -p "$d"
  python3 - "$d/routes.toml" "$upstream" "$BQ1_HOST" <<'PY'
import hashlib, sys
path, upstream, host = sys.argv[1:]
body = (
    "[[pg.namespaces]]\n"
    'name = "bench"\n'
    f'hostnames = ["{host}"]\n'
    f'upstreams = ["{upstream}"]\n'
)
# routes::generation_of: sha256 of the file without its generation line.
h = hashlib.sha256()
for line in body.splitlines():
    h.update(line.encode() + b"\n")
open(path, "w").write(f'generation = "sha256:{h.hexdigest()}"\n' + body)
PY
  cat >"$d/gateway.toml" <<TOML
[admin]
listen = "127.0.0.1:0"

[routes]
path = "$d/routes.toml"

[pg]
listen = "127.0.0.1:$GW_PORT"
tls_mode = "auto"
max_connections = 4096

[[pg.certs]]
names = ["$BQ1_HOST", "*.bench.test"]
cert = "$BQ1_DATA/certs/leaf.crt"
key = "$BQ1_DATA/certs/leaf.key"

[pg.upstream_tls]
ca = "$BQ1_DATA/certs/ca.crt"
TOML
  RUST_LOG=warn taskset -c "$cores" "$GW_BIN" --config "$d/gateway.toml" --module pg \
    >"$d/gateway.log" 2>&1 &
  GW_PID=$!
  local _
  for _ in $(seq 100); do
    if python3 -c 'import socket,sys; socket.create_connection(("127.0.0.1", int(sys.argv[1])), 0.2)' "$GW_PORT" 2>/dev/null; then
      return 0
    fi
    kill -0 "$GW_PID" 2>/dev/null || break
    sleep 0.1
  done
  cat "$d/gateway.log" >&2
  return 1
}

gw_stop() {
  if [ -n "${GW_PID:-}" ]; then
    kill "$GW_PID" 2>/dev/null || true
    wait "$GW_PID" 2>/dev/null || true
    GW_PID=
  fi
}

# gw_cpu_jiffies: the gateway process's utime+stime (clock ticks).
gw_cpu_jiffies() {
  awk '{print $14 + $15}' "/proc/$GW_PID/stat"
}

# pgbench_c <conninfo env...> -- <pgbench args...>: pgbench in the pinned
# image on CLIENT_CORES, host network, logs into $BQ1_DATA/run.
pgbench_c() {
  local -a envs=()
  while [ "$1" != -- ]; do envs+=(-e "$1"); shift; done
  shift
  local -a hosts=()
  [ "$BQ1_CLIENT_NET" = --network=host ] && hosts=(--add-host "$BQ1_HOST:127.0.0.1")
  podman run --rm "$BQ1_CLIENT_NET" --cpuset-cpus "$CLIENT_CORES" "${hosts[@]}" \
    -v "$BQ1_DATA/run:/run-logs" -v "$BQ1_DATA/scripts:/scripts:ro" -w /run-logs -e PGPASSWORD="$BQ1_PW" -e PGUSER="$BQ1_USER" \
    "${envs[@]}" "$PG_IMAGE" pgbench "$@"
}

# conninfo <mode> <port>: the libpq environment for one connection path.
#   plain        TCP, no TLS
#   tls          TLS straight to the server (SSLRequest)
#   gw-splice    TLS through the gateway, SSLRequest: the gateway reads SNI
#                and splices bytes (no TLS work in the gateway)
#   gw-terminate direct TLS (PG17 sslnegotiation=direct, ALPN postgresql)
#                through the gateway: it terminates and re-encrypts
conninfo() {
  local mode=$1 port=$2
  case $mode in
    plain) echo "PGHOST=127.0.0.1 PGPORT=$port PGSSLMODE=disable" ;;
    tls) echo "PGHOST=$BQ1_HOST PGHOSTADDR=127.0.0.1 PGPORT=$port PGSSLMODE=require" ;;
    gw-splice) echo "PGHOST=$BQ1_HOST PGHOSTADDR=127.0.0.1 PGPORT=$GW_PORT PGSSLMODE=require" ;;
    gw-terminate) echo "PGHOST=$BQ1_HOST PGHOSTADDR=127.0.0.1 PGPORT=$GW_PORT PGSSLMODE=require PGSSLNEGOTIATION=direct" ;;
    *) bq1_log "unknown mode $mode"; return 1 ;;
  esac
}

# bq1_pipe_script <N>: $BQ1_DATA/scripts/pipeN.sql, N point SELECTs on
# pgbench_accounts inside one pipeline (pgbench 14+; the aids are drawn as
# -S draws them).
bq1_pipe_script() {
  local n=$1 f=$BQ1_DATA/scripts/pipe$1.sql i
  mkdir -p "$BQ1_DATA/scripts"
  {
    for i in $(seq 1 "$n"); do echo "\\set aid$i random(1, 100000 * :scale)"; done
    echo '\startpipeline'
    for i in $(seq 1 "$n"); do echo "SELECT abalance FROM pgbench_accounts WHERE aid = :aid$i;"; done
    echo '\endpipeline'
  } >"$f"
}

# bench_run <label> <mode> <port> <workload> <clients> [extra json]:
# one measured pgbench run (WARMUP s, then DURATION s with a sampled log);
# appends one JSON object to $BQ1_RESULTS/<run file>.
#   workload select-only: pgbench -S (one SELECT per transaction)
#   workload tpcb:        built-in TPC-B (7 statements per transaction)
bench_run() {
  local label=$1 mode=$2 port=$3 workload=$4 clients=$5 extra=${6:-"{}"}
  local -a env wl
  read -r -a env <<<"$(conninfo "$mode" "$port")"
  case $workload in
    select-only) wl=(-S) ;;
    tpcb) wl=() ;;
    pipe[0-9]*)
      # pipeN: N pgbench select-only statements in one libpq pipeline
      # (\startpipeline ... \endpipeline: one Sync, one flush; extended
      # protocol through -M prepared). QPS = TPS x N.
      bq1_pipe_script "${workload#pipe}"
      wl=(-f "/scripts/$workload.sql")
      ;;
    *) bq1_log "unknown workload $workload"; return 1 ;;
  esac
  local threads
  threads=$(count_cores "$CLIENT_CORES")
  [ "$clients" -ge "$threads" ] || threads=$clients
  rm -rf "$BQ1_DATA/run"
  mkdir -p "$BQ1_DATA/run"
  chmod 0777 "$BQ1_DATA/run"
  # What else runs on the bench's cores while ours idle (other agents'
  # builds are not stopped by the bench slot): 2 s, recorded with the run.
  local bg0 bg1
  bg0=$(cpu_snapshot "$PG_CORES,$GW_CORES,$CLIENT_CORES,$STORAGE_CORES")
  sleep 2
  bg1=$(cpu_snapshot "$PG_CORES,$GW_CORES,$CLIENT_CORES,$STORAGE_CORES")
  local background
  background=$(python3 -c 'import os,sys; a=sys.argv[1].split(); b=sys.argv[2].split(); print(round((int(b[0])-int(a[0]))/os.sysconf("SC_CLK_TCK")/2, 2))' "$bg0" "$bg1")
  extra=$(python3 -c 'import json,sys; e=json.loads(sys.argv[1]); e["background_busy_cores"]=float(sys.argv[2]); e["loadavg"]=open("/proc/loadavg").read().split()[:3]; print(json.dumps(e))' "$extra" "$background")
  # Warm-up: connections, plans, caches.
  pgbench_c "${env[@]}" -- -n -M prepared "${wl[@]}" -c "$clients" -j "$threads" -T "$WARMUP" >/dev/null 2>&1 || true
  local pg0 gw0 cl0 st0 g0=0 t0
  pg0=$(cpu_snapshot "$PG_CORES"); gw0=$(cpu_snapshot "$GW_CORES"); cl0=$(cpu_snapshot "$CLIENT_CORES")
  st0=$(cpu_snapshot "$STORAGE_CORES")
  [ -n "${GW_PID:-}" ] && g0=$(gw_cpu_jiffies)
  t0=$(date +%s.%N)
  pgbench_c "${env[@]}" -- -n -M prepared "${wl[@]}" -c "$clients" -j "$threads" -T "$DURATION" \
    -l --sampling-rate "$SAMPLE" --log-prefix bq1 >"$BQ1_DATA/run/summary.txt" 2>&1 || {
    cat "$BQ1_DATA/run/summary.txt" >&2; return 1; }
  local t1 pg1 gw1 cl1 st1 g1=0
  t1=$(date +%s.%N)
  pg1=$(cpu_snapshot "$PG_CORES"); gw1=$(cpu_snapshot "$GW_CORES"); cl1=$(cpu_snapshot "$CLIENT_CORES")
  st1=$(cpu_snapshot "$STORAGE_CORES")
  [ -n "${GW_PID:-}" ] && g1=$(gw_cpu_jiffies)
  python3 "$BQ1_HERE/summarize.py" \
    --label "$label" --mode "$mode" --workload "$workload" --clients "$clients" --threads "$threads" \
    --duration "$DURATION" --summary "$BQ1_DATA/run/summary.txt" --logs "$BQ1_DATA/run" \
    --cpu "pg=$PG_CORES:$pg0:$pg1" --cpu "gw=$GW_CORES:$gw0:$gw1" --cpu "client=$CLIENT_CORES:$cl0:$cl1" \
    --cpu "storage=$STORAGE_CORES:$st0:$st1" \
    --gw-jiffies "$g0:$g1" --wall "$t0:$t1" --extra "$extra" >>"$BQ1_OUT"
  rm -rf "$BQ1_DATA/run"
  python3 "$BQ1_HERE/show.py" "$BQ1_OUT"
}

# bq1_out <name>: start a JSON-lines result file for this session.
bq1_out() {
  BQ1_OUT=$BQ1_RESULTS/$(date -u +%Y%m%dT%H%M%SZ)-$1.jsonl
  export BQ1_OUT
  python3 - >"$BQ1_OUT" <<PY
import json, os, platform, subprocess
def sh(c):
    try:
        return subprocess.run(c, shell=True, capture_output=True, text=True).stdout.strip()
    except Exception:
        return ""
print(json.dumps({"meta": True, "session": "$1",
  "commit": sh("git -C $BQ1_ROOT rev-parse --short HEAD"),
  "host": {"cpu": sh("grep -m1 'model name' /proc/cpuinfo | cut -d: -f2").strip(),
           "cores": os.cpu_count(), "kernel": platform.release(),
           "mem_gb": round(os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") / 2**30, 1)},
  "cores": {"pg": "$PG_CORES", "gateway": "$GW_CORES", "client": "$CLIENT_CORES", "storage": "$STORAGE_CORES"},
  "pg_image": "$PG_IMAGE", "duration_s": $DURATION, "warmup_s": $WARMUP, "scale": $SCALE,
  "sampling_rate": $SAMPLE}))
PY
}
