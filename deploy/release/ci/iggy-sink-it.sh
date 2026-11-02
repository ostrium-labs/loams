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
# the pull of IGGY_IMAGE (.env.example).
set -euo pipefail

release="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo="$(cd "$release/../.." && pwd)"

export COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-loams-iggy-sink-it}"
export IGGY_IT_PORT="${IGGY_IT_PORT:-28190}"
export LOAMS_POSTGRES_COMPOSE=./postgres-off.yaml
export COMPOSE_PROFILES=iggy
IGGY_PASSWORD="it-$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
export IGGY_PASSWORD

compose() {
  (cd "$release" && docker compose --env-file .env.example -f compose.yaml -f ci/compose.iggy-it.yaml "$@")
}

cleanup() {
  status=$?
  if [[ $status -ne 0 ]]; then
    echo "--- iggy-sink-it failed (exit $status); service logs:" >&2
    compose logs --no-color --tail=80 iggy >&2 || true
  fi
  compose down -v --remove-orphans >&2 || true
  exit "$status"
}
trap cleanup EXIT

echo "== starting iggy"
compose up -d --wait --wait-timeout 240 iggy

restart="cd '$release' && docker compose --env-file .env.example -f compose.yaml -f ci/compose.iggy-it.yaml restart iggy"

echo "== cargo test -p loams-iggy-sink --test iggy"
(
  cd "$repo"
  LOAMS_IGGY_SINK_IT="127.0.0.1:${IGGY_IT_PORT}" \
  LOAMS_IGGY_SINK_IT_USER=iggy \
  LOAMS_IGGY_SINK_IT_PASSWORD="$IGGY_PASSWORD" \
  LOAMS_IGGY_SINK_IT_RESTART="$restart" \
    cargo test -p loams-iggy-sink --test iggy "$@" -- --test-threads=1 --nocapture
)
echo "iggy-sink-it ok"
