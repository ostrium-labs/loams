#!/usr/bin/env bash
# The `iggy` profile's smoke (design 32 5.10, D2450): the profile is off in the
# default stack, and with COMPOSE_PROFILES=iggy the official Apache Iggy server
# starts healthy and answers an authenticated login with the profile's own
# password. There is no Kafka gateway any more (D2450).
#
#   deploy/release/ci/iggy-smoke.sh
#
# Needs docker (or podman with a `docker` shim), `docker compose`, and the pull of
# IGGY_IMAGE (.env.example). It starts only `iggy` (not the engine or the
# Postgres stack), on its own compose project, and removes it with its volumes
# at the end.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-loams-iggy-smoke}"
export LOAMS_POSTGRES_COMPOSE=./postgres-off.yaml
unset COMPOSE_PROFILES IGGY_PASSWORD

compose() { docker compose --env-file .env.example "$@"; }

cleanup() {
  status=$?
  if [[ $status -ne 0 ]]; then
    echo "--- iggy smoke failed (exit $status); service logs:" >&2
    COMPOSE_PROFILES=iggy compose logs --no-color --tail=80 iggy >&2 || true
  fi
  COMPOSE_PROFILES=iggy compose down -v --remove-orphans >&2 || true
  exit "$status"
}
trap cleanup EXIT

echo "== default stack: no Iggy"
default_services="$(compose config --services | sort | tr '\n' ' ')"
case " $default_services" in
  *" iggy "*) echo "the default stack has Iggy: $default_services"; exit 1 ;;
esac
with_profile="$(COMPOSE_PROFILES=iggy compose config --services | sort | tr '\n' ' ')"
case " $with_profile" in *" iggy "*) ;; *) echo "profile iggy lacks iggy: $with_profile"; exit 1 ;; esac
case " $with_profile" in *" kafka-gateway "*) echo "the Kafka gateway is gone (D2450): $with_profile"; exit 1 ;; esac

echo "== the placeholder password is refused before Iggy starts"
set +e
refusal="$(COMPOSE_PROFILES=iggy compose run --rm --no-deps -T iggy 2>&1)"
refused=$?
set -e
[[ $refused == 78 && "$refusal" == *"still the .env.example placeholder; refusing to start"* ]] \
  || { printf 'iggy with the placeholder password: exit %s\n%s\n' "$refused" "$refusal"; exit 1; }
# Nothing was written: the refused run never reached the server.
COMPOSE_PROFILES=iggy compose down -v --remove-orphans >/dev/null 2>&1 || true

# A password of the smoke's own from here on.
IGGY_PASSWORD="smoke-$(od -An -N12 -tx1 /dev/urandom | tr -d ' \n')"
export IGGY_PASSWORD

echo "== starting the iggy profile (iggy)"
COMPOSE_PROFILES=iggy compose up -d --wait --wait-timeout 240 iggy

# Only the Iggy service of the profile is up: the engine did not start.
running="$(COMPOSE_PROFILES=iggy compose ps --services --status running | sort | tr '\n' ' ')"
[[ "$running" == "iggy " ]] || { echo "unexpected running services: $running"; exit 1; }

echo "== login with the profile's password"
COMPOSE_PROFILES=iggy compose exec -T iggy \
  /usr/local/bin/iggy -u "${IGGY_USER:-iggy}" -p "$IGGY_PASSWORD" stream list >/dev/null

echo "iggy smoke ok"
