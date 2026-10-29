#!/usr/bin/env bash
# BQ1-iggy stack: iggy-server (pinned, N shards) [+ Kafka gateway] on private ports.
# stack.sh up <shards> [gateway]   |  stack.sh down      (kills only the PIDs it wrote)
# Env: BIN (dir with iggy-server, iggy-gateway-kafka), DATA (default /mnt/Projects/bench-data/bq1-iggy)
set -u
BIN="${BIN:?BIN dir}"; DATA="${DATA:-/mnt/Projects/bench-data/bq1-iggy}"; RUN="$DATA/run"
SERVER_CPUS="${SERVER_CPUS:-0-3}"; GW_CPUS="${GW_CPUS:-4-7}"
mkdir -p "$RUN" "$DATA/logs"
alive() { [ -f "$1" ] && kill -0 "$(cat "$1")" 2>/dev/null; }
case "${1:-}" in
up)
  N="${2:?shards}"; GW="${3:-}"
  rm -rf "$DATA/iggy"
  # a previous run leaves TIME_WAIT sockets that block the bind
  for i in $(seq 90); do [ -z "$(ss -tan "( sport = :48090 or sport = :49093 )" 2>/dev/null | tail -n +2)" ] && break; sleep 2; done
  export IGGY_ROOT_USERNAME=iggy IGGY_ROOT_PASSWORD=bq1-local IGGY_PATH="$DATA/iggy"
  export IGGY_TCP_ADDRESS=127.0.0.1:48090 IGGY_HTTP_ADDRESS=127.0.0.1:43000
  export IGGY_QUIC_ADDRESS=127.0.0.1:48080 IGGY_WEBSOCKET_ADDRESS=127.0.0.1:48092
  export IGGY_SHARDING_CPU_ALLOCATION="$N"
  setsid taskset -c "$SERVER_CPUS" "$BIN/iggy-server" >"$DATA/logs/server.log" 2>&1 </dev/null & echo $! >"$RUN/server.pid"
  for i in $(seq 60); do (exec 3<>/dev/tcp/127.0.0.1/48090) 2>/dev/null && break; sleep 1; done
  alive "$RUN/server.pid" || { echo "server died"; tail -5 "$DATA/logs/server.log"; exit 1; }
  if [ -n "$GW" ]; then
    export IGGY_KAFKA_LISTENERS=PLAINTEXT://127.0.0.1:49093 IGGY_KAFKA_ADVERTISED_LISTENERS=PLAINTEXT://127.0.0.1:49093
    export IGGY_KAFKA_BRIDGE_ENABLED=true IGGY_KAFKA_IGGY_ADDR=127.0.0.1:48090
    export IGGY_KAFKA_IGGY_USERNAME=iggy IGGY_KAFKA_IGGY_PASSWORD=bq1-local IGGY_KAFKA_AUTO_CREATE_TOPICS_ENABLED=true
    setsid taskset -c "$GW_CPUS" "${GW_BIN:-$BIN/iggy-gateway-kafka}" >"$DATA/logs/gateway.log" 2>&1 </dev/null & echo $! >"$RUN/gateway.pid"
    for i in $(seq 30); do (exec 3<>/dev/tcp/127.0.0.1/49093) 2>/dev/null && break; sleep 1; done
  fi
  echo "up shards=$N gateway=${GW:-no}" ;;
down)
  for n in gateway server; do
    if alive "$RUN/$n.pid"; then kill "$(cat "$RUN/$n.pid")"; for i in $(seq 30); do alive "$RUN/$n.pid" || break; sleep 1; done; fi
    rm -f "$RUN/$n.pid"
  done; rm -rf "$DATA/iggy"; echo down ;;
esac
