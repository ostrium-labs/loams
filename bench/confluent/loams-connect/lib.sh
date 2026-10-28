# shellcheck shell=bash
# Shared settings and helpers for up.sh, down.sh, connect.sh and e2e.sh.
# Every setting is an environment variable with a default; see README.md.

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"

: "${CFB_RUN:=$HERE/.run}"                       # state, secrets, logs, pids
: "${CFB_PREFIX:=loams-cfb-}"                    # consumer group + transactional id prefix
: "${CFB_TOPIC_IN:=cfb-in}"                      # Kafka -> Loams (source instance)
: "${CFB_TOPIC_OUT:=cfb-out}"                    # Loams -> Kafka (sink instance)
: "${CFB_STREAM:=cfb}"                           # the Loams stream between them
: "${CFB_NAMESPACE:=default}"
: "${CFB_PARTITIONS:=1}"                         # topic and stream partitions
: "${CFB_INTEGRATION:=cfb}"                      # secrets/0/confluent-<integration>/
: "${CFB_CHECKPOINT_TOPIC:=__loams_checkpoints}"
: "${CFB_DEBEZIUM:=0}"                           # 1: also a debezium-json source instance
: "${CFB_TOPIC_DBZ:=cfb-dbz}"
: "${LOAMS_PORT:=18080}"                         # engine main API (loopback)
: "${CONNECT_ADMIN:=127.0.0.1:17790}"            # connect admin API
: "${KAFKA_IMAGE:=docker.io/apache/kafka:4.3.1}"
: "${KAFKA_LOCAL_PORT:=19092}"
: "${KAFKA_CONTAINER:=cfb-kafka}"
: "${CFB_CONNECT_VIA:=socket}"                   # socket: the engine's --connect-socket; tcp: its main port
: "${OTEL_EXPORTER_OTLP_ENDPOINT:=http://127.0.0.1:4318}"
export OTEL_EXPORTER_OTLP_ENDPOINT

# Binaries: copies in <repo>/bin win over the shared target, so another
# agent's rebuild cannot swap them mid-run.
pick_bin() {
  local name=$1
  for c in "$REPO/bin/$name" "/mnt/Projects/rust-cache/target/release/$name"; do
    [[ -x $c ]] && { echo "$c"; return; }
  done
}
: "${LOAMS_BIN:=$(pick_bin loams)}"
: "${FABRIC_BIN:=$(pick_bin loams-fabric)}"

# Local Kafka unless KAFKA_BOOTSTRAP names one.
if [[ -z ${KAFKA_BOOTSTRAP:-} ]]; then
  KAFKA_LOCAL=1
  KAFKA_BOOTSTRAP="127.0.0.1:$KAFKA_LOCAL_PORT"
else
  KAFKA_LOCAL=0
fi
# Confluent Cloud style bootstrap strings carry SASL_SSL://; strip any scheme.
KAFKA_BOOTSTRAP="${KAFKA_BOOTSTRAP#*://}"
# SASL_SSL PLAIN (the confluent-cloud profile) when a key file is given.
if [[ -n ${KAFKA_KEY_FILE:-} ]]; then KAFKA_SASL=1; else KAFKA_SASL=0; fi

ENGINE_URL="http://127.0.0.1:$LOAMS_PORT"
SOCK="$CFB_RUN/engine.sock"
SECRETS="$CFB_RUN/secrets"
KCFG="$CFB_RUN/kafka-client"   # client.properties for the Kafka CLI tools (0600)

log() { printf '[%(%H:%M:%S)T] %s\n' -1 "$*" >&2; }
die() { log "error: $*"; exit 1; }

# key=value from the key file, without echoing it.
keyfile_get() { sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*//p" "$KAFKA_KEY_FILE" | head -n1 | tr -d '\r'; }

# The Kafka CLI tools from the same image, on the host network.
kafka_tool() {
  local tool=$1; shift
  podman run --rm -i --network host --user 0:0 --security-opt label=disable \
    -v "$KCFG:/cfg:ro" "$KAFKA_IMAGE" "/opt/kafka/bin/$tool" \
    --bootstrap-server "$KAFKA_BOOTSTRAP" --command-config /cfg/client.properties "$@"
}

write_client_config() {
  (umask 077; mkdir -p "$KCFG")
  if [[ $KAFKA_SASL == 1 ]]; then
    local id secret
    id=$(keyfile_get id); secret=$(keyfile_get secret)
    [[ -n $id && -n $secret ]] || die "KAFKA_KEY_FILE needs lines id=... and secret=..."
    (umask 077; cat >"$KCFG/client.properties" <<EOF
security.protocol=SASL_SSL
sasl.mechanism=PLAIN
sasl.jaas.config=org.apache.kafka.common.security.plain.PlainLoginModule required username="$id" password="$secret";
ssl.endpoint.identification.algorithm=https
isolation.level=read_committed
EOF
    )
  else
    (umask 077; printf 'security.protocol=PLAINTEXT\nisolation.level=read_committed\n' >"$KCFG/client.properties")
  fi
  # The tools run as the container's root, which rootless podman maps to our
  # uid, so the 0600 file stays readable to them and to nobody else.
}

engine_key() { cat "$SECRETS/engine-key"; }

# One loams.stream.v1.StreamService call (Connect JSON) on the main port.
engine_call() {
  local method=$1 body=$2
  curl -fsS -X POST "$ENGINE_URL/loams.stream.v1.StreamService/$method" \
    -H 'content-type: application/json' -H 'connect-protocol-version: 1' \
    -H "authorization: Bearer $(engine_key)" -d "$body"
}

admin_call() {
  local method=$1 path=$2
  curl -fsS -X "$method" "http://$CONNECT_ADMIN$path" \
    -H "authorization: Bearer $(cat "$SECRETS/connect-admin-token")"
}

pid_alive() { [[ -f $1 ]] && kill -0 "$(cat "$1")" 2>/dev/null; }

# Starts the connect role in the background (pid in $CFB_RUN/connect.pid).
start_connect() {
  local epoch_log="$CFB_RUN/logs/connect.log"
  OTEL_SERVICE_NAME=loams-connect RUST_LOG="${RUST_LOG:-info}" \
    setsid "$FABRIC_BIN" connect \
      --instances "$CFB_RUN/instances" \
      --engine "$( [[ $CFB_CONNECT_VIA == tcp ]] && echo "$ENGINE_URL" || echo "unix://$SOCK")" \
      --engine-key-file "$SECRETS/engine-key" \
      --secrets-root "$SECRETS" \
      --store "file://$CFB_RUN/connect-state" \
      --admin-listen "$CONNECT_ADMIN" \
      --admin-token-file "$SECRETS/connect-admin-token" \
      --connectors-dir "$REPO/connectors" \
      >>"$epoch_log" 2>&1 </dev/null &
  echo $! >"$CFB_RUN/connect.pid"
  local i
  for i in $(seq 1 120); do
    if curl -fsS "http://$CONNECT_ADMIN/health" >/dev/null 2>&1; then
      log "connect up (pid $(cat "$CFB_RUN/connect.pid"))"; return 0
    fi
    pid_alive "$CFB_RUN/connect.pid" || { tail -n 30 "$epoch_log" >&2; die "connect exited"; }
    sleep 0.25
  done
  die "connect did not answer /health"
}

# Waits until every instance reports running.
wait_instances_running() {
  local i st
  for i in $(seq 1 240); do
    st=$(admin_call GET /v1/connect/instances 2>/dev/null || true)
    if [[ -n $st ]] && python3 -I -c '
import json,sys
d=json.loads(sys.argv[1]); xs=d.get("instances", d if isinstance(d,list) else [])
sys.exit(0 if xs and all(x.get("state", x.get("status"))=="running" for x in xs) else 1)' "$st"; then
      return 0
    fi
    sleep 0.5
  done
  log "instances not all running:"; echo "$st" >&2; return 1
}
