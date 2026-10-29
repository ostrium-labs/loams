#!/usr/bin/env bash
# BQ1-stream benchmark driver (docs/benchmarks/2026-10-28-1b-qps/stream.md).
# Every measuring command must run under the bench lock:
#
#   .he0-shared/benchslot.sh bq1-stream bench/stream/run.sh <cmd> ...
#
# Commands:
#   up <risingwave|flink>          start the stack (Iggy + its Kafka gateway, Postgres,
#                                  RustFS, Lakekeeper, and that engine)
#   down                           stop it and delete its data
#   rw-window  <parallelism> <events>   Kafka -> RisingWave TUMBLE(1s) MV
#   rw-iceberg <parallelism> <events>   Kafka -> RisingWave -> Iceberg (Lakekeeper) sink
#   flink-window <parallelism> <events> Kafka -> Flink SQL TUMBLE(1s) -> blackhole
#   produce <partitions> <events>  produce only (the Kafka gateway's ingest rate)
#   gw-pg <seconds>                PG wire to RisingWave, direct vs through loams-gateway's
#                                  RisingWave route (TLS + key check + upstream login), steady
#                                  and one connection per query (needs `up risingwave`)
#   facade <seconds>               control-plane API QPS: loams-engines' HTTP facade with the
#                                  RisingWave adapter (examples/engines_facade_bench), oha
#
# Each run prints one JSON line and appends it to $OUT/<engine>.jsonl.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
data=${BQ1_STREAM_DATA:-/mnt/Projects/bench-data/bq1-stream}
out=${OUT:-$root/bench/results/bq1/stream}
mkdir -p "$out" "$data/sql-gen" "$data/iggy" "$data/rustfs"

compose() { podman compose -f "$here/compose.yaml" "$@" 2>/dev/null; }
psql_rw() { psql "host=127.0.0.1 port=14566 user=root dbname=dev connect_timeout=5" -XAtq -v ON_ERROR_STOP=1 "$@"; }
# The load generator runs in a container on the compose network (the Kafka gateway
# advertises its compose name), pinned to cores 8-11, disjoint from the engine's 0-5.
LOADGEN_IMAGE=docker.io/library/postgres:17.10@sha256:7958605b474b3d264a969cb3a123d6aa00ad1e1fe9da8a69984dabb704d93317
loadgen() {
  podman run --rm --network bq1-stream_default --cpuset-cpus 8-11 \
    -v "$data/loadgen:/loadgen:ro" --entrypoint /loadgen "$LOADGEN_IMAGE" "$@"
}
now() { date +%s.%N; }
rev() { git -C "$root" rev-parse --short HEAD 2>/dev/null || echo unknown; }
record() { # engine json
  echo "$2" | tee -a "$out/$1.jsonl"
}

s3() { # method bucket
  curl -sf -o /dev/null -H "x-amz-content-sha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" \
    --aws-sigv4 "aws:amz:us-east-1:s3" --user "bench:benchbench" -X "$1" "http://127.0.0.1:19000/$2"
}

wait_for() { # what seconds cmd...
  local what=$1 secs=$2; shift 2
  local end=$((SECONDS + secs))
  until "$@" >/dev/null 2>&1; do
    ((SECONDS < end)) || { echo "run.sh: $what not ready after ${secs}s" >&2; return 1; }
    sleep 1
  done
}

up() {
  local engine=$1
  compose --profile "$engine" up -d
  wait_for "RustFS" 120 curl -s -o /dev/null http://127.0.0.1:19000/
  s3 PUT bench-engines || true
  s3 PUT bench-lake || true
  wait_for "Lakekeeper" 180 curl -sf http://127.0.0.1:18181/health
  curl -s -o /dev/null -X POST http://127.0.0.1:18181/management/v1/bootstrap \
    -H 'Content-Type: application/json' --data '{"accept-terms-of-use": true}' || true
  if ! curl -sf http://127.0.0.1:18181/management/v1/warehouse | grep -q '"name":"bench"'; then
    curl -sf -o /dev/null -X POST http://127.0.0.1:18181/management/v1/warehouse \
      -H 'Content-Type: application/json' --data '{
        "warehouse-name": "bench",
        "storage-profile": {"type": "s3", "bucket": "bench-lake", "key-prefix": "warehouse",
          "endpoint": "http://rustfs:9000", "region": "us-east-1", "path-style-access": true,
          "flavor": "s3-compat", "sts-enabled": false},
        "storage-credential": {"type": "s3", "credential-type": "access-key",
          "access-key-id": "bench", "secret-access-key": "benchbench"}}'
  fi
  local prefix
  prefix=$(curl -sf 'http://127.0.0.1:18181/catalog/v1/config?warehouse=bench' |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["defaults"]["prefix"])')
  curl -s -o /dev/null -X POST "http://127.0.0.1:18181/catalog/v1/$prefix/namespaces" \
    -H 'Content-Type: application/json' --data '{"namespace": ["bench"]}' || true
  case $engine in
    risingwave) wait_for "RisingWave" 240 psql_rw -c 'SELECT 1' ;;
    flink) wait_for "Flink" 240 curl -sf http://127.0.0.1:18081/overview ;;
  esac
  echo "up: $engine"
}

down() {
  compose --profile risingwave --profile flink down -v --remove-orphans || true
  # The bench's own data only.
  podman unshare rm -rf "$data/iggy" "$data/rustfs" "$data/sql-gen"
  echo "down"
}

fresh_topic() { # name partitions
  loadgen topic -brokers kafka-gateway:9093 -topic "$1" -delete >/dev/null 2>&1 || true
  loadgen topic -brokers kafka-gateway:9093 -topic "$1" -partitions "$2" >/dev/null
}

produce_only() { # partitions events [extra loadgen flags...]
  local p=$1 n=$2 topic="t_produce_p$1_$RANDOM"; shift 2
  fresh_topic "$topic" "$p"
  local res
  res=$(loadgen produce -brokers kafka-gateway:9093 -topic "$topic" -events "$n" "$@")
  loadgen topic -brokers kafka-gateway:9093 -topic "$topic" -delete >/dev/null 2>&1 || true
  record kafka-gateway "$(python3 -c 'import json,sys; r=json.loads(sys.argv[1]); r.update(engine="iggy-kafka-gateway", partitions=int(sys.argv[2]), cores="6-7", rev=sys.argv[3]); print(json.dumps(r))' "$res" "$p" "$(rev)")"
}

# Polls `sql` until it returns `want`; prints "first_nonzero_s done_s" since t0.
poll_rw() { # t0 want sql timeout
  local t0=$1 want=$2 sql=$3 timeout=$4 first="" v
  local end=$((SECONDS + timeout))
  while ((SECONDS < end)); do
    v=$(psql_rw -c "$sql" 2>/dev/null || echo 0)
    v=${v:-0}
    if [[ -z "$first" && "$v" != 0 ]]; then first=$(python3 -c "print($(now)-$t0)"); fi
    if [[ "$v" == "$want" ]]; then
      echo "${first:-0} $(python3 -c "print($(now)-$t0)")"
      return 0
    fi
    sleep 0.25
  done
  echo "${first:-0} -1 $v"
}

rw_source_sql() { # name topic
  cat <<SQL
CREATE SOURCE $1 (user_id BIGINT, amount BIGINT, ts TIMESTAMPTZ,
  WATERMARK FOR ts AS ts - INTERVAL '5 seconds')
WITH (connector = 'kafka', topic = '$2', properties.bootstrap.server = 'kafka-gateway:9093',
  scan.startup.mode = 'earliest') FORMAT PLAIN ENCODE JSON;
SQL
}

rw_reset() {
  psql_rw -c 'DROP SINK IF EXISTS ice_sink' -c 'DROP SOURCE IF EXISTS ice_read' \
    -c 'DROP MATERIALIZED VIEW IF EXISTS agg' -c 'DROP SOURCE IF EXISTS events CASCADE' >/dev/null 2>&1 || true
}

rw_window() { # parallelism events
  local p=$1 n=$2 topic="t_rw_w_p$1_$RANDOM"
  rw_reset
  fresh_topic "$topic" "$p"
  local prod
  prod=$(loadgen produce -brokers kafka-gateway:9093 -topic "$topic" -events "$n")
  local t0
  t0=$(now)
  { echo "SET streaming_parallelism = $p;"; echo "SET background_ddl = true;";
    rw_source_sql events "$topic";
    echo "CREATE MATERIALIZED VIEW agg AS SELECT window_start, user_id % 100 AS bucket,
      count(*) AS cnt, sum(amount) AS total
      FROM TUMBLE(events, ts, INTERVAL '1 second') GROUP BY window_start, user_id % 100;"
  } | psql_rw >/dev/null
  read -r first done rest < <(poll_rw "$t0" "$n" 'SELECT coalesce(sum(cnt), 0) FROM agg' 600)
  rw_reset
  loadgen topic -brokers kafka-gateway:9093 -topic "$topic" -delete >/dev/null 2>&1 || true
  record risingwave "$(python3 - "$prod" "$p" "$n" "$first" "$done" "$(rev)" <<'PY'
import json, sys
prod, p, n, first, done, rev = sys.argv[1:]
p, n, first, done = int(p), int(n), float(first), float(done)
ok = done > 0
print(json.dumps({
    "engine": "risingwave", "version": "v3.1.0", "scenario": "kafka->tumble(1s) mv",
    "parallelism": p, "partitions": p, "events": n, "engine_cores": "0-5",
    "complete": ok, "seconds_to_all": done if ok else None, "seconds_to_first": first,
    "events_per_s": n / done if ok else None,
    "events_per_s_after_first": n / (done - first) if ok and done > first else None,
    "produce": json.loads(prod), "rev": rev}))
PY
)"
}

rw_iceberg() { # parallelism events [commit_checkpoint_interval]
  local p=$1 n=$2 ci=${3:-1} topic="t_rw_i_p$1_$RANDOM" table="events_p$1_$RANDOM"
  rw_reset
  fresh_topic "$topic" "$p"
  local prod
  prod=$(loadgen produce -brokers kafka-gateway:9093 -topic "$topic" -events "$n")
  local ice="catalog.type = 'rest', catalog.name = 'bench', catalog.uri = 'http://lakekeeper:8181/catalog',
    warehouse.path = 'bench', database.name = 'bench', table.name = '$table',
    s3.endpoint = 'http://rustfs:9000', s3.region = 'us-east-1', s3.access.key = 'bench',
    s3.secret.key = 'benchbench', s3.path.style.access = 'true'"
  local t0
  t0=$(now)
  { echo "SET streaming_parallelism = $p;"; echo "SET background_ddl = true;";
    rw_source_sql events "$topic";
    echo "CREATE SINK ice_sink AS SELECT user_id, amount, ts FROM events WITH (connector = 'iceberg',
      type = 'append-only', create_table_if_not_exists = 'true', commit_checkpoint_interval = '$ci', $ice);"
  } | psql_rw >/dev/null
  # The read-back source can only be created once the sink made the table.
  wait_for "the Iceberg table" 120 psql_rw -c "CREATE SOURCE ice_read WITH (connector = 'iceberg', $ice)"
  read -r first done rest < <(poll_rw "$t0" "$n" 'SELECT count(*) FROM ice_read' 600)
  rw_reset
  loadgen topic -brokers kafka-gateway:9093 -topic "$topic" -delete >/dev/null 2>&1 || true
  record risingwave "$(python3 - "$prod" "$p" "$n" "$first" "$done" "$(rev)" "$ci" <<'PY'
import json, sys
prod, p, n, first, done, rev, ci = sys.argv[1:]
p, n, first, done = int(p), int(n), float(first), float(done)
ok = done > 0
print(json.dumps({
    "engine": "risingwave", "version": "v3.1.0", "scenario": "kafka->iceberg sink (lakekeeper, rustfs)",
    "commit_checkpoint_interval": int(ci),
    "parallelism": p, "partitions": p, "events": n, "engine_cores": "0-5",
    "complete": ok, "seconds_to_all": done if ok else None, "seconds_to_first": first,
    "events_per_s": n / done if ok else None,
    "events_per_s_after_first": n / (done - first) if ok and done > first else None,
    "produce": json.loads(prod), "rev": rev}))
PY
)"
}

flink_window() { # parallelism events
  local p=$1 n=$2 topic="t_fl_w_p$1_$RANDOM"
  fresh_topic "$topic" "$p"
  local prod
  prod=$(loadgen produce -brokers kafka-gateway:9093 -topic "$topic" -events "$n")
  cat > "$data/sql-gen/flink.sql" <<SQL
SET 'parallelism.default' = '$p';
SET 'table.dml-sync' = 'false';
CREATE TABLE events (user_id BIGINT, amount BIGINT, ts TIMESTAMP_LTZ(3),
  WATERMARK FOR ts AS ts - INTERVAL '5' SECOND)
WITH ('connector' = 'kafka', 'topic' = '$topic', 'properties.bootstrap.servers' = 'kafka-gateway:9093',
  'properties.group.id' = 'bq1-$topic', 'scan.startup.mode' = 'earliest-offset', 'format' = 'json',
  'json.timestamp-format.standard' = 'ISO-8601');
CREATE TABLE out_agg (window_start TIMESTAMP_LTZ(3), bucket BIGINT, cnt BIGINT, total BIGINT)
WITH ('connector' = 'blackhole');
INSERT INTO out_agg SELECT window_start, user_id % 100, COUNT(*), SUM(amount)
FROM TABLE(TUMBLE(TABLE events, DESCRIPTOR(ts), INTERVAL '1' SECOND))
GROUP BY window_start, window_end, user_id % 100;
SQL
  local t0
  t0=$(now)
  podman cp "$data/sql-gen/flink.sql" bq1-stream-flink-jobmanager-1:/tmp/bench.sql
  podman exec bq1-stream-flink-jobmanager-1 /opt/flink/bin/sql-client.sh -f /tmp/bench.sql >/dev/null
  local res
  res=$(python3 - "$t0" "$n" <<'PY'
import json, sys, time, urllib.request
t0, n = float(sys.argv[1]), int(sys.argv[2])
base = "http://127.0.0.1:18081"
def get(p):
    with urllib.request.urlopen(base + p, timeout=5) as r:
        return json.load(r)
first = None
deadline = time.time() + 600
job = None
while time.time() < deadline:
    jobs = [j for j in get("/jobs/overview")["jobs"] if j["state"] == "RUNNING"]
    if jobs:
        job = jobs[0]["jid"]
        detail = get(f"/jobs/{job}")
        src = detail["vertices"][0]
        read = src["metrics"].get("write-records", 0)
        if read and first is None:
            first = time.time() - t0
        if read >= n:
            print(json.dumps({"first": first, "done": time.time() - t0, "records": read}))
            break
    time.sleep(0.25)
else:
    print(json.dumps({"first": first, "done": -1}))
if job:
    try:
        urllib.request.urlopen(urllib.request.Request(base + f"/jobs/{job}?mode=cancel", method="PATCH"), timeout=5)
    except Exception:
        pass
PY
)
  loadgen topic -brokers kafka-gateway:9093 -topic "$topic" -delete >/dev/null 2>&1 || true
  record flink "$(python3 - "$prod" "$p" "$n" "$res" "$(rev)" <<'PY'
import json, sys
prod, p, n, res, rev = sys.argv[1:]
p, n, res = int(p), int(n), json.loads(res)
done, first = res["done"], res["first"] or 0
ok = done > 0
print(json.dumps({
    "engine": "flink", "version": "2.2.1", "scenario": "kafka->tumble(1s)->blackhole",
    "parallelism": p, "partitions": p, "events": n, "engine_cores": "0-5",
    "complete": ok, "seconds_to_all": done if ok else None, "seconds_to_first": first,
    "events_per_s": n / done if ok else None,
    "events_per_s_after_first": n / (done - first) if ok and done > first else None,
    "measured_by": "source vertex write-records (JobManager REST)",
    "produce": json.loads(prod), "rev": rev}))
PY
)"
}

bin=${BQ1_STREAM_BIN:-$data/bin}
lg_host() { taskset -c 8-11 "$data/loadgen" "$@"; }

gw_pg() { # seconds
  local secs=${1:-20} gw="$data/gw" key=bq1-bench-key-0123456789
  mkdir -p "$gw"
  psql_rw -c "CREATE USER loams_gateway WITH SUPERUSER PASSWORD 'gwbench'" >/dev/null 2>&1 || true
  printf 'gwbench' > "$gw/password"
  printf 'default %s\n' "$(printf %s "$key" | sha256sum | cut -d' ' -f1)" > "$gw/keys"
  cat > "$gw/gateway.toml" <<TOML
[admin]
listen = "127.0.0.1:19187"
[routes]
path = "$gw/routes.toml"
require_generation = false
[pg]
listen = "127.0.0.1:15432"
dedicated_bind = "127.0.0.1"
dedicated_ports = "14600-14600"
tls_mode = "passthrough"
max_connections = 10000
[risingwave]
self_signed_names = ["localhost"]
auth = { kind = "keys_file", path = "$gw/keys" }
TOML
  cat > "$gw/routes.toml" <<TOML
[[risingwave.endpoints]]
name = "bench"
namespace = "default"
hostnames = []
upstreams = ["127.0.0.1:14566"]
upstream_user = "loams_gateway"
upstream_password_file = "$gw/password"
port = 14600
TOML
  RUST_LOG=warn taskset -c 6-7 "$bin/loams-gateway" --config "$gw/gateway.toml" >"$gw/gateway.log" 2>&1 &
  local gpid=$!
  wait_for "loams-gateway" 30 bash -c "exec 3<>/dev/tcp/127.0.0.1/14600"
  local direct="host=127.0.0.1 port=14566 user=root dbname=dev sslmode=disable"
  local via="host=127.0.0.1 port=14600 user=bench password=$key dbname=dev sslmode=require"
  for mode in steady reconnect; do
    local flag="" conns=16
    [[ $mode == reconnect ]] && flag=-reconnect && conns=8
    for path in direct gateway; do
      local dsn=$direct; [[ $path == gateway ]] && dsn=$via
      local r
      r=$(lg_host pg -dsn "$dsn" -conns "$conns" -duration "${secs}s" $flag -label "$path-$mode")
      record gateway-pg "$(python3 -c 'import json,sys; r=json.loads(sys.argv[1]); r.update(engine="risingwave", path=sys.argv[2], gateway_cores="6-7", engine_cores="0-5", loadgen_cores="8-11", rev=sys.argv[3]); print(json.dumps(r))' "$r" "$path" "$(rev)")"
    done
  done
  kill "$gpid"; wait "$gpid" 2>/dev/null || true
  rm -rf "$gw"
}

facade() { # seconds
  local secs=${1:-20} fb="$bin/engines_facade_bench"
  BENCH_THREADS=6 taskset -c 0-5 "$fb" serve 127.0.0.1:18090 1000 2>"$data/facade.log" &
  local fpid=$!
  wait_for "the facade" 60 curl -sf http://127.0.0.1:18090/v1/engines
  local id
  id=$(sed -n 's/.*one id: //p' "$data/facade.log")
  for route in "/v1/engines" "/v1/namespaces/bench/engines/$id" "/v1/namespaces/bench/engines"; do
    # depth 1: HTTP/1.1, 64 connections, one request in flight each.
    # depth 16: HTTP/2 (h2c), 8 connections x 16 concurrent streams.
    for variant in "1 --http-version=1.1 -c 64" "16 --http2 -c 8 -p 16"; do
      local depth=${variant%% *} flags=${variant#* } r
      r=$(taskset -c 8-11 oha --no-tui --output-format json -z "${secs}s" $flags "http://127.0.0.1:18090$route")
      record facade "$(python3 -c '
import json, sys
r = json.loads(sys.argv[1]); s = r["summary"]; p = r["latencyPercentiles"]
route = sys.argv[2].split("/eng_")[0] + ("/{id}" if "/eng_" in sys.argv[2] else "")
print(json.dumps({"engine": "loams-engines facade + RisingWave adapter", "op": "GET " + route,
  "route": route, "instances": 1000, "depth": int(sys.argv[3]), "client": sys.argv[4],
  "cores": "0-5", "loadgen_cores": "8-11",
  "qps": s["requestsPerSec"], "success_rate": s["successRate"],
  "p50_ms": p["p50"] * 1000, "p99_ms": p["p99"] * 1000, "p999_ms": p["p99.9"] * 1000,
  "errors": sum(r.get("errorDistribution", {}).values()),
  "rev": sys.argv[5]}))' "$r" "$route" "$depth" "oha $flags" "$(rev)")"
    done
  done
  kill "$fpid"; wait "$fpid" 2>/dev/null || true
  for c in 1 8; do
    local r
    r=$(BENCH_THREADS=6 taskset -c 0-5 "$fb" creates 20000 "$c")
    record facade "$(python3 -c 'import json,sys; r=json.loads(sys.argv[1]); r.update(engine="loams-engines facade + RisingWave adapter", route="POST /v1/namespaces/{ns}/engines (in-process)", server_cores="0-5", rev=sys.argv[2]); print(json.dumps(r))' "$r" "$(rev)")"
  done
}

cmd=${1:-}; shift || true
case $cmd in
  up) up "$@" ;;
  down) down ;;
  produce) produce_only "$@" ;;
  rw-window) rw_window "$@" ;;
  rw-iceberg) rw_iceberg "$@" ;;
  flink-window) flink_window "$@" ;;
  gw-pg) gw_pg "$@" ;;
  facade) facade "$@" ;;
  # One bench-lock hold each (<= 15 min): bring the stack up, measure, tear it down.
  session-rw-a) up risingwave; for p in 1 2 4; do produce_only "$p" 1000000; done
    produce_only 4 1000000 -inflight 1 -linger-ms 0
    for p in 1 2 4; do rw_window "$p" "${N:-2000000}"; done; down ;;
  session-rw-b) up risingwave; for p in 1 2 4; do rw_iceberg "$p" "${N:-1000000}" 1; done
    rw_iceberg 4 "${N:-1000000}" 10; gw_pg 15; down ;;
  session-native) shift 0; for d in 1 16; do "$here/native-load.sh" --duration 20 --cores 0-5 \
      --loadgen-cores 8-11 --depth "$d" | tee -a "$out/risingwave-native.jsonl"; done ;;
  session-flink) up flink; for p in 1 2 4; do flink_window "$p" "${N:-2000000}"; done; down ;;
  *) sed -n '2,20p' "$0"; exit 2 ;;
esac
