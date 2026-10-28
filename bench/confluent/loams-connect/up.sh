#!/usr/bin/env bash
# Start a local Loams engine (dev mode, data in $CFB_RUN/engine), the
# `loams-fabric connect` role with a Kafka source (topic -> stream) and an
# exactly-once Kafka sink (stream -> topic), and, when KAFKA_BOOTSTRAP is
# unset, a local Apache Kafka 4.3.1 container. See README.md.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

command -v podman >/dev/null || die "podman is needed for the Kafka container and CLI tools"
podman image exists "$KAFKA_IMAGE" || die "image $KAFKA_IMAGE is not present (podman pull it)"
[[ -x $LOAMS_BIN ]] || die "no loams binary: put it in $REPO/bin or set LOAMS_BIN"
[[ -x $FABRIC_BIN ]] || die "no loams-fabric binary: put it in $REPO/bin or set FABRIC_BIN"
pid_alive "$CFB_RUN/engine.pid" && die "already up (run down.sh first)"

(umask 077; mkdir -p "$CFB_RUN" "$CFB_RUN/logs" "$CFB_RUN/instances" "$CFB_RUN/connect-state" "$SECRETS/0")
log "loams:        $LOAMS_BIN"
log "loams-fabric: $FABRIC_BIN"
log "kafka:        $KAFKA_BOOTSTRAP ($([[ $KAFKA_SASL == 1 ]] && echo 'SASL_SSL PLAIN, verify_full, profile confluent-cloud' || echo PLAINTEXT))"

# ---------------------------------------------------------------- Kafka
if [[ $KAFKA_LOCAL == 1 ]]; then
  if ! podman container exists "$KAFKA_CONTAINER"; then
    log "starting $KAFKA_CONTAINER ($KAFKA_IMAGE) on 127.0.0.1:$KAFKA_LOCAL_PORT"
    podman run -d --name "$KAFKA_CONTAINER" --network host \
      -e KAFKA_NODE_ID=1 -e KAFKA_PROCESS_ROLES=broker,controller \
      -e KAFKA_LISTENERS="PLAINTEXT://127.0.0.1:$KAFKA_LOCAL_PORT,CONTROLLER://127.0.0.1:$((KAFKA_LOCAL_PORT + 1))" \
      -e KAFKA_ADVERTISED_LISTENERS="PLAINTEXT://127.0.0.1:$KAFKA_LOCAL_PORT" \
      -e KAFKA_CONTROLLER_LISTENER_NAMES=CONTROLLER \
      -e KAFKA_LISTENER_SECURITY_PROTOCOL_MAP=CONTROLLER:PLAINTEXT,PLAINTEXT:PLAINTEXT \
      -e KAFKA_CONTROLLER_QUORUM_VOTERS="1@127.0.0.1:$((KAFKA_LOCAL_PORT + 1))" \
      -e KAFKA_OFFSETS_TOPIC_REPLICATION_FACTOR=1 \
      -e KAFKA_TRANSACTION_STATE_LOG_REPLICATION_FACTOR=1 \
      -e KAFKA_TRANSACTION_STATE_LOG_MIN_ISR=1 \
      -e KAFKA_SHARE_COORDINATOR_STATE_TOPIC_REPLICATION_FACTOR=1 \
      -e KAFKA_SHARE_COORDINATOR_STATE_TOPIC_MIN_ISR=1 \
      -e KAFKA_GROUP_INITIAL_REBALANCE_DELAY_MS=0 \
      "$KAFKA_IMAGE" >/dev/null
  else
    podman start "$KAFKA_CONTAINER" >/dev/null
  fi
  for i in $(seq 1 60); do
    podman exec "$KAFKA_CONTAINER" /opt/kafka/bin/kafka-broker-api-versions.sh \
      --bootstrap-server "127.0.0.1:$KAFKA_LOCAL_PORT" >/dev/null 2>&1 && break
    [[ $i == 60 ]] && die "Kafka did not come up (podman logs $KAFKA_CONTAINER)"
    sleep 1
  done
  log "Kafka up"
fi
write_client_config

# Topics. Both carry LogAppendTime so e2e.sh measures with the broker's clock
# (the input's append time travels as the event's `time`). On Confluent Cloud
# a key without CREATE gets an error here: create the topics yourself.
ensure_topic() {
  local topic=$1; shift
  if kafka_tool kafka-topics.sh --describe --topic "$topic" >/dev/null 2>&1; then
    log "topic $topic exists"; return 0
  fi
  kafka_tool kafka-topics.sh --create --if-not-exists --topic "$topic" "$@" >/dev/null 2>"$CFB_RUN/logs/topic-$topic.err" \
    && log "topic $topic created" \
    || log "warning: could not create topic $topic: $(tail -n1 "$CFB_RUN/logs/topic-$topic.err")"
}
RF=()
[[ $KAFKA_LOCAL == 1 ]] && RF=(--replication-factor 1)
ensure_topic "$CFB_TOPIC_IN"  --partitions "$CFB_PARTITIONS" "${RF[@]}" --config message.timestamp.type=LogAppendTime
ensure_topic "$CFB_TOPIC_OUT" --partitions "$CFB_PARTITIONS" "${RF[@]}" --config message.timestamp.type=LogAppendTime
# The exactly-once sink's checkpoint topic: compacted, one partition.
ensure_topic "$CFB_CHECKPOINT_TOPIC" --partitions 1 "${RF[@]}" --config cleanup.policy=compact
[[ $CFB_DEBEZIUM == 1 ]] && ensure_topic "$CFB_TOPIC_DBZ" --partitions 1 "${RF[@]}"

# ---------------------------------------------------------------- engine
# API keys only: the connect socket takes nothing else.
if [[ ! -s $SECRETS/engine-key ]]; then
  (umask 077; "$LOAMS_BIN" auth new-key --label connect --append "$CFB_RUN/api-keys" 2>/dev/null >"$SECRETS/engine-key")
fi
[[ -s $SECRETS/connect-admin-token ]] || (umask 077; head -c 32 /dev/urandom | base64 | tr -d '\n/+=' >"$SECRETS/connect-admin-token")

log "starting the engine on $ENGINE_URL (connect socket $SOCK)"
rm -f "$SOCK"
LOAMS_AUTH_MODE=apikey LOAMS_API_KEYS_FILE="$CFB_RUN/api-keys" OTEL_SERVICE_NAME=loams-engine \
RUST_LOG="${RUST_LOG:-info}" setsid "$LOAMS_BIN" dev \
  --data-dir "$CFB_RUN/engine" --listen "127.0.0.1:$LOAMS_PORT" \
  $( [[ $CFB_CONNECT_VIA == socket ]] && echo "--connect-socket $SOCK") \
  --no-flight-sql --no-qdrant --no-es --no-durable --no-live --no-graph --no-console \
  >"$CFB_RUN/logs/engine.log" 2>&1 </dev/null &
echo $! >"$CFB_RUN/engine.pid"
for i in $(seq 1 240); do
  [[ $CFB_CONNECT_VIA == tcp || -S $SOCK ]] && grep -q "loams listening on" "$CFB_RUN/logs/engine.log" && break
  pid_alive "$CFB_RUN/engine.pid" || { tail -n 30 "$CFB_RUN/logs/engine.log" >&2; die "engine exited"; }
  [[ $i == 240 ]] && die "engine did not come up"
  sleep 0.25
done
log "engine up (pid $(cat "$CFB_RUN/engine.pid"))"
# The namespace (409 when it exists), then the stream.
curl -sS -o /dev/null -X POST "$ENGINE_URL/loams.collection.v1.NamespaceService/CreateNamespace" \
  -H 'content-type: application/json' -H 'connect-protocol-version: 1' \
  -H "authorization: Bearer $(engine_key)" -d "{\"namespace\":\"$CFB_NAMESPACE\"}"
engine_call CreateStream "{\"namespace\":\"$CFB_NAMESPACE\",\"name\":\"$CFB_STREAM\",\"partitions\":$CFB_PARTITIONS}" >/dev/null \
  || die "CreateStream $CFB_STREAM failed"

# ---------------------------------------------------------------- instances
if [[ $KAFKA_SASL == 1 ]]; then
  d="$SECRETS/0/confluent-$CFB_INTEGRATION"
  (umask 077; mkdir -p "$d"; keyfile_get id >"$d/username"; keyfile_get secret >"$d/password")
  PROFILE_LINE="profile: confluent-cloud"
  SEC="  sasl:
    username_secret: confluent-$CFB_INTEGRATION/username
    password: confluent-$CFB_INTEGRATION/password"
else
  PROFILE_LINE="# no profile: a plaintext local broker"
  SEC="  security_protocol: plaintext"
fi

cat >"$CFB_RUN/instances/cfb-in.yaml" <<EOF
# Kafka topic $CFB_TOPIC_IN -> Loams stream $CFB_STREAM.
name: cfb-in
connector: kafka
$PROFILE_LINE
direction: source
namespace: $CFB_NAMESPACE
namespace_id: 0
stream: $CFB_STREAM
stream_partitions: $CFB_PARTITIONS
config:
  brokers: "$KAFKA_BOOTSTRAP"
  topics: ["$CFB_TOPIC_IN"]
  group_id_prefix: "$CFB_PREFIX"
  group_id: "${CFB_PREFIX}cfb-in"
  auto_offset_reset: earliest
  # At most 1000 events a batch: the engine refuses a bigger
  # ProduceCloudEvents, and connect builds before the cfb-connect fix send
  # every event of such a batch to the DLQ (a restart with a backlog).
$( [[ -n ${CFB_MAX_BATCH_ROWS-1000} ]] && echo "  pipeline: { max_batch_rows: ${CFB_MAX_BATCH_ROWS-1000} }")
$SEC
EOF

cat >"$CFB_RUN/instances/cfb-out.yaml" <<EOF
# Loams stream $CFB_STREAM -> Kafka topic $CFB_TOPIC_OUT, exactly once: each
# batch and its checkpoint (in $CFB_CHECKPOINT_TOPIC) in one Kafka transaction,
# transactional.id ${CFB_PREFIX}cfb-out-<split>.
name: cfb-out
connector: kafka
$PROFILE_LINE
direction: sink
namespace: $CFB_NAMESPACE
namespace_id: 0
stream: $CFB_STREAM
delivery: exactly_once
config:
  brokers: "$KAFKA_BOOTSTRAP"
  topics: ["$CFB_TOPIC_OUT"]
  group_id_prefix: "$CFB_PREFIX"
  transactional_id_prefix: "$CFB_PREFIX"
  delivery: exactly_once
  checkpoint_topic: "$CFB_CHECKPOINT_TOPIC"
  create_checkpoint_topic: false
$SEC
EOF

if [[ $CFB_DEBEZIUM == 1 ]]; then
cat >"$CFB_RUN/instances/cfb-dbz.yaml" <<EOF
# Debezium JSON change events in $CFB_TOPIC_DBZ -> Loams stream ${CFB_STREAM}-dbz.
name: cfb-dbz
connector: kafka
$PROFILE_LINE
direction: source
namespace: $CFB_NAMESPACE
namespace_id: 0
stream: ${CFB_STREAM}-dbz
config:
  brokers: "$KAFKA_BOOTSTRAP"
  topics: ["$CFB_TOPIC_DBZ"]
  group_id_prefix: "$CFB_PREFIX"
  group_id: "${CFB_PREFIX}cfb-dbz"
  auto_offset_reset: earliest
  format: debezium-json
$SEC
EOF
fi

# ---------------------------------------------------------------- connect
start_connect
wait_instances_running || die "see $CFB_RUN/logs/connect.log"
admin_call GET /v1/connect/instances | python3 -I -c '
import json,sys
for i in json.load(sys.stdin)["instances"]:
    print("  %-8s %-6s %-8s stream=%s/%s splits=%s" % (i["name"], i["direction"], i["state"], i["namespace"], i["stream"], i["splits"]))' >&2
log "up. e2e: $HERE/e2e.sh   down: $HERE/down.sh"
