#!/usr/bin/env bash
# loams_sink's integration tests (crates/loams-iggy-sink/tests/iggy.rs, FL1
# Task 10, design 32 5.10) against the image the `kafka` profile pins: starts
# `iggy` and `kafka-gateway` from deploy/release/compose.yaml (with
# ci/compose.iggy-it.yaml publishing Iggy's TCP port on loopback), produces two
# records with kcat through the gateway, runs the tests (including a restart of
# the Iggy container) and removes the project with its volumes.
#
#   deploy/release/ci/iggy-sink-it.sh [extra cargo test args]
#
# Needs docker (or podman with a `docker` shim), `docker compose`, cargo, and
# the pulls of IGGY_KAFKA_IMAGE (.env.example) and the pinned kcat image.
set -euo pipefail

release="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo="$(cd "$release/../.." && pwd)"

KCAT_IMAGE="${KCAT_IMAGE:-docker.io/edenhill/kcat:1.7.1@sha256:8f16a5fed099931ce1122420b7473efe467ff9841d53680b99db25dd1723d711}"
export COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-loams-iggy-sink-it}"
export KAFKA_PORT="${KAFKA_IT_PORT:-29193}"
export IGGY_IT_PORT="${IGGY_IT_PORT:-28190}"
export KAFKA_AUTO_CREATE_TOPICS=true
export LOAMS_POSTGRES_COMPOSE=./postgres-off.yaml
export COMPOSE_PROFILES=kafka
KAFKA_IGGY_PASSWORD="it-$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
export KAFKA_IGGY_PASSWORD

compose() {
  (cd "$release" && docker compose --env-file .env.example -f compose.yaml -f ci/compose.iggy-it.yaml "$@")
}

cleanup() {
  status=$?
  if [[ $status -ne 0 ]]; then
    echo "--- iggy-sink-it failed (exit $status); service logs:" >&2
    compose logs --no-color --tail=80 iggy kafka-gateway >&2 || true
  fi
  compose down -v --remove-orphans >&2 || true
  exit "$status"
}
trap cleanup EXIT

echo "== starting iggy and kafka-gateway"
compose up -d --wait --wait-timeout 240 iggy kafka-gateway

echo "== two records through the Kafka gateway"
topic="gw-$(date +%s)-$RANDOM"
kcat() { docker run --rm -i --network host "$KCAT_IMAGE" -b "127.0.0.1:${KAFKA_PORT}" "$@"; }
for attempt in 1 2 3 4 5 6; do
  if printf 'k1:v1\n' | kcat -P -t "$topic" -p 0 -K: -H trace=abc \
       -X message.timeout.ms=20000 -X socket.timeout.ms=15000 \
     && printf 'k2:{"n":2}\n' | kcat -P -t "$topic" -p 0 -K: \
       -X message.timeout.ms=20000; then break; fi
  [[ $attempt -lt 6 ]] || { echo "kcat -P kept failing"; exit 1; }
  sleep 5
done

restart="cd '$release' && docker compose --env-file .env.example -f compose.yaml -f ci/compose.iggy-it.yaml restart iggy"

echo "== cargo test -p loams-iggy-sink --test iggy"
(
  cd "$repo"
  LOAMS_IGGY_SINK_IT="127.0.0.1:${IGGY_IT_PORT}" \
  LOAMS_IGGY_SINK_IT_USER=iggy \
  LOAMS_IGGY_SINK_IT_PASSWORD="$KAFKA_IGGY_PASSWORD" \
  LOAMS_IGGY_SINK_IT_RESTART="$restart" \
  LOAMS_IGGY_SINK_IT_KAFKA_TOPIC="$topic" \
    cargo test -p loams-iggy-sink --test iggy "$@" -- --test-threads=1 --nocapture
)
echo "iggy-sink-it ok"
