#!/usr/bin/env bash
# The `kafka` profile's smoke (KW-L2, design 32 5.10, D1798): the profile is off
# in the default stack, and with COMPOSE_PROFILES=kafka the fork's Iggy server and
# Kafka gateway start healthy and kcat produces to and consumes from a topic
# through the gateway.
#
#   deploy/release/ci/kafka-smoke.sh
#
# Needs docker (or podman with a `docker` shim), `docker compose`, and the pull of
# IGGY_KAFKA_IMAGE (.env.example) and of the pinned kcat image. It starts only
# `iggy` and `kafka-gateway` (not the engine or the Postgres stack), on its own
# compose project and port, and removes both with their volumes at the end.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Pinned: kcat 1.7.1 (librdkafka 2.x). amd64 only, so this runs on amd64 hosts.
KCAT_IMAGE="${KCAT_IMAGE:-docker.io/edenhill/kcat:1.7.1@sha256:8f16a5fed099931ce1122420b7473efe467ff9841d53680b99db25dd1723d711}"
export COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-loams-kafka-smoke}"
export KAFKA_PORT="${KAFKA_SMOKE_PORT:-29093}"
# The smoke creates its topic by producing to it.
export KAFKA_AUTO_CREATE_TOPICS=true
export LOAMS_POSTGRES_COMPOSE=./postgres-off.yaml
unset COMPOSE_PROFILES KAFKA_IGGY_PASSWORD

compose() { docker compose --env-file .env.example "$@"; }

cleanup() {
  status=$?
  if [[ $status -ne 0 ]]; then
    echo "--- kafka smoke failed (exit $status); service logs:" >&2
    COMPOSE_PROFILES=kafka compose logs --no-color --tail=80 iggy kafka-gateway >&2 || true
  fi
  COMPOSE_PROFILES=kafka compose down -v --remove-orphans >&2 || true
  exit "$status"
}
trap cleanup EXIT

echo "== default stack: no Iggy, no Kafka gateway"
default_services="$(compose config --services | sort | tr '\n' ' ')"
case " $default_services" in
  *" iggy "* | *" kafka-gateway "*) echo "the default stack has Kafka services: $default_services"; exit 1 ;;
esac
with_profile="$(COMPOSE_PROFILES=kafka compose config --services | sort | tr '\n' ' ')"
for s in iggy kafka-gateway; do
  case " $with_profile" in *" $s "*) ;; *) echo "profile kafka lacks $s: $with_profile"; exit 1 ;; esac
done

echo "== the placeholder password is refused before Iggy starts"
set +e
refusal="$(COMPOSE_PROFILES=kafka compose run --rm --no-deps -T iggy 2>&1)"
refused=$?
set -e
[[ $refused == 78 && "$refusal" == *"still the .env.example placeholder; refusing to start"* ]] \
  || { printf 'iggy with the placeholder password: exit %s\n%s\n' "$refused" "$refusal"; exit 1; }
# Nothing was written: the refused run never reached the server.
COMPOSE_PROFILES=kafka compose down -v --remove-orphans >/dev/null 2>&1 || true

# A password of the smoke's own from here on.
KAFKA_IGGY_PASSWORD="smoke-$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
export KAFKA_IGGY_PASSWORD

echo "== starting the kafka profile (iggy, kafka-gateway)"
COMPOSE_PROFILES=kafka compose up -d --wait --wait-timeout 240 iggy kafka-gateway

# Only the Kafka services of the profile are up: the engine did not start.
running="$(COMPOSE_PROFILES=kafka compose ps --services --status running | sort | tr '\n' ' ')"
[[ "$running" == "iggy kafka-gateway " ]] || { echo "unexpected running services: $running"; exit 1; }

broker="127.0.0.1:${KAFKA_PORT}"
topic="smoke-$(date +%s)-$RANDOM"
kcat() { docker run --rm -i --network host "$KCAT_IMAGE" -b "$broker" "$@"; }

echo "== metadata"
for attempt in 1 2 3 4 5 6; do
  if kcat -L -m 15 >/dev/null 2>&1; then break; fi
  [[ $attempt -lt 6 ]] || { echo "kcat -L never answered"; exit 1; }
  sleep 5
done

echo "== produce to $topic"
for attempt in 1 2 3 4 5 6; do
  if printf 'loams-kafka-1\nloams-kafka-2\nloams-kafka-3\n' \
       | kcat -P -t "$topic" -p 0 -X message.timeout.ms=20000 -X socket.timeout.ms=15000; then break; fi
  [[ $attempt -lt 6 ]] || { echo "kcat -P kept failing"; exit 1; }
  sleep 5
done

echo "== consume from $topic"
got="$(kcat -C -t "$topic" -p 0 -o beginning -e -q -X session.timeout.ms=10000)"
want=$'loams-kafka-1\nloams-kafka-2\nloams-kafka-3'
[[ "$got" == "$want" ]] || { printf 'consumed:\n%s\nwanted:\n%s\n' "$got" "$want"; exit 1; }

echo "kafka smoke ok: 3 records through ${broker}"
