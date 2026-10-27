#!/usr/bin/env bash
# The compute conformance harness's shared functions (OC1 OC0 Task 1, DX1
# DX0 Task 1). Source it from a product's run.sh:
#
#   . "$(dirname "$0")/../lib.sh"
#   oc_up temporal
#   oc_temporal_wait
#   oc_junit_case temporal temporal_schema_setup pass
#   oc_xfail_check temporal "$OC_OUT/temporal.junit.xml"
#
# Everything runs against deploy/release/compose.yaml plus
# compose.compute.yaml, with the pins of pins.env. Helpers that must reach a
# service do so from a container on the stack's network (OC_NETWORK), never
# through a host port: the compute services publish none.
#
# Environment:
#   OC_OUT        where JUnit files and logs go (default: $OC_ROOT/target/compute)
#   OC_ENV_FILE   the stack's .env (default: deploy/release/.env, else .env.example)
#   OC_NETWORK    the stack's network (default: <OC_PROJECT, else loams>_default)
#   OC_COMPOSE    the compose command (default: docker compose, else docker-compose, else podman compose)
#   OC_PROJECT    a compose project name other than the stack's (`loams`), for a
#                 second stack beside a running one (OC_NETWORK follows it)
#   OC_COMPOSE_EXTRA  colon-separated extra compose files, last (local overrides)
#   LOAMS_PG_DIRECT_URL  the direct endpoint as an admin URL (default: compute1's cloud_admin)
# shellcheck shell=bash

OC_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
OC_DIR="$OC_ROOT/conformance/compute"
OC_PINS="$OC_DIR/pins.env"
OC_RELEASE="$OC_ROOT/deploy/release"
: "${OC_OUT:=$OC_ROOT/target/compute}"
: "${OC_NETWORK:=${OC_PROJECT:-loams}_default}"
# The issue that tracks the missing session-pooled endpoint (DX1 R0.7).
OC_SESSION_ISSUE='https://github.com/ostrium-labs/loams/issues/1129'

# Read the pins into the environment (KEY=value lines; pins.env has no
# variables or quotes, so this is a plain read, not `source`).
oc_load_pins() {
  local line
  while IFS= read -r line || [ -n "$line" ]; do
    case $line in '' | '#'*) continue ;; esac
    export "${line?}"
  done <"$OC_PINS"
}
oc_load_pins

oc_compose_cmd() {
  if [ -n "${OC_COMPOSE:-}" ]; then
    # shellcheck disable=SC2086 # a command and its arguments
    printf '%s\n' $OC_COMPOSE
  elif docker compose version >/dev/null 2>&1; then
    printf '%s\n' docker compose
  elif command -v docker-compose >/dev/null 2>&1; then
    printf '%s\n' docker-compose
  elif podman compose version >/dev/null 2>&1; then
    printf '%s\n' podman compose
  else
    echo "oc: no docker compose, docker-compose or podman compose" >&2
    return 1
  fi
}

# The container CLI that runs one-off helper containers.
oc_container_cmd() {
  if command -v docker >/dev/null 2>&1; then echo docker
  elif command -v podman >/dev/null 2>&1; then echo podman
  else echo "oc: no docker or podman" >&2; return 1
  fi
}

oc_env_file() {
  if [ -n "${OC_ENV_FILE:-}" ]; then echo "$OC_ENV_FILE"
  elif [ -f "$OC_RELEASE/.env" ]; then echo "$OC_RELEASE/.env"
  else echo "$OC_RELEASE/.env.example"
  fi
}

# oc_compose [--txn] [--gateway] <compose args...>: compose with the stack,
# the compute override and the pins; --txn adds the transaction-mode overlay
# (PgDog), --gateway the Temporal gateway's (temporal-gateway.yaml, the
# profile `temporal-gateway`). Both are test overlays, never distributed.
oc_compose() {
  local -a cmd files
  mapfile -t cmd < <(oc_compose_cmd) || return 1
  files=(-f "$OC_RELEASE/compose.yaml" -f "$OC_RELEASE/compose.compute.yaml")
  while :; do
    case ${1:-} in
      --txn) shift; files+=(-f "$OC_DIR/pgdog-txn.yaml") ;;
      --gateway) shift; files+=(-f "$OC_DIR/temporal-gateway.yaml") ;;
      *) break ;;
    esac
  done
  local extra
  if [ -n "${OC_COMPOSE_EXTRA:-}" ]; then
    local -a extras
    IFS=: read -r -a extras <<<"$OC_COMPOSE_EXTRA"
    for extra in "${extras[@]}"; do files+=(-f "$extra"); done
  fi
  local -a project=()
  [ -n "${OC_PROJECT:-}" ] && project=(-p "$OC_PROJECT")
  "${cmd[@]}" "${project[@]}" --project-directory "$OC_RELEASE" --env-file "$(oc_env_file)" --env-file "$OC_PINS" \
    "${files[@]}" "$@"
}

# oc_up <profile> [txn]: start one compute profile's services and what they
# depend on (Loams Postgres's stack), not the whole release stack: the
# engine, the House and agentd are not needed to judge a product.
oc_up() {
  local profile=${1:?oc_up <profile> [txn]} txn=()
  [ "${2:-}" = txn ] && txn=(--txn)
  [ "$profile" = temporal-gateway ] && txn+=(--gateway)
  mkdir -p "$OC_OUT"
  local -a services
  mapfile -t services < <(oc_profile_services "$profile" "${txn[@]}")
  oc_compose "${txn[@]}" --profile "$profile" up -d "${services[@]}"
}

# oc_down <profile> [txn]: stop and remove that profile's services and their
# volumes, leaving the rest of the stack (and Loams Postgres's data) alone.
oc_down() {
  local profile=${1:?oc_down <profile> [txn]} txn=()
  [ "${2:-}" = txn ] && txn=(--txn)
  [ "$profile" = temporal-gateway ] && txn+=(--gateway)
  local -a services
  mapfile -t services < <(oc_profile_services "$profile" "${txn[@]}")
  [ "${#services[@]}" -gt 0 ] || return 0
  oc_compose "${txn[@]}" --profile "$profile" rm -s -f -v "${services[@]}"
}

# oc_profile_services <profile> [--txn] [--gateway]: the services only that
# profile adds.
oc_profile_services() {
  local profile=$1
  shift
  comm -13 <(oc_compose "$@" config --services | sort) \
    <(oc_compose "$@" --profile "$profile" config --services | sort)
}

# oc_run <image> <args...>: a one-off container on the stack's network.
oc_run() {
  local image=$1
  shift
  "$(oc_container_cmd)" run --rm --network "$OC_NETWORK" "$image" "$@"
}

# oc_pg_mode <txn|session|direct> [database] [user:password]: the URL of
# Loams Postgres in that endpoint mode (DX1 R0.7). `direct` defaults to the
# admin URL (LOAMS_PG_DIRECT_URL, else compute1's cloud_admin); `txn` is the
# PgDog of pgdog-txn.yaml and needs one of its users (pgdog/users.toml).
# `session` does not exist yet and fails with the issue that tracks it.
oc_pg_mode() {
  local mode=${1:?oc_pg_mode <txn|session|direct> [database] [user:password]} db=${2:-postgres} who=${3:-}
  local url
  case $mode in
    direct)
      url=${LOAMS_PG_DIRECT_URL:-postgresql://cloud_admin:cloud_admin@compute1:55433/postgres}
      if [ -n "$who" ]; then
        local rest=${url#postgresql://}
        rest=${rest#*@}
        url="postgresql://$who@$rest"
      fi
      echo "${url%/*}/$db" ;;
    txn)
      [ -n "$who" ] || { echo "oc: oc_pg_mode txn needs a user:password from pgdog/users.toml" >&2; return 2; }
      echo "postgresql://$who@pgdog-txn:6432/$db" ;;
    session)
      echo "oc: no session-pooled Loams Postgres endpoint in the release compose ($OC_SESSION_ISSUE)" >&2
      return 1 ;;
    *)
      echo "oc: unknown endpoint mode '$mode' (txn, session or direct)" >&2
      return 2 ;;
  esac
}

# oc_psql <url> <sql>: run SQL with the pinned client, unaligned output.
oc_psql() {
  oc_run "$PSQL_IMAGE" psql "$1" -v ON_ERROR_STOP=1 -XAtq -c "$2"
}

# oc_wait_pg [url] [seconds]: until Postgres answers SELECT 1.
oc_wait_pg() {
  local url=${1:-$(oc_pg_mode direct)} t=${2:-180} i
  for ((i = 0; i < t; i += 2)); do
    if [ "$(oc_psql "$url" 'SELECT 1' 2>/dev/null)" = 1 ]; then return 0; fi
    sleep 2
  done
  echo "oc: Postgres at ${url%%@*}@… did not answer in ${t}s" >&2
  return 1
}

# oc_wait_http <url> [seconds]: until the URL answers 2xx, from the network.
oc_wait_http() {
  local url=${1:?oc_wait_http <url> [seconds]} t=${2:-180} i
  for ((i = 0; i < t; i += 3)); do
    if oc_run "$PYTHON_IMAGE" curl -fsS -o /dev/null --max-time 5 "$url" 2>/dev/null; then return 0; fi
    sleep 3
  done
  echo "oc: $url did not answer in ${t}s" >&2
  return 1
}

# oc_temporal_wait [address] [seconds]: until the frontend answers
# GetSystemInfo (`temporal operator cluster health` says SERVING).
oc_temporal_wait() {
  local addr=${1:-temporal-frontend:7233} t=${2:-300} i out
  for ((i = 0; i < t; i += 3)); do
    out=$(oc_run "$TEMPORAL_ADMIN_TOOLS_IMAGE" temporal operator cluster health --address "$addr" 2>/dev/null) || out=
    case $out in *SERVING*) return 0 ;; esac
    sleep 3
  done
  echo "oc: the Temporal frontend at $addr did not report SERVING in ${t}s" >&2
  return 1
}

# oc_bucket_ls <s3://bucket/prefix>: the keys under a prefix on the stack's
# RustFS (the `rustfs` profile), one per line.
oc_bucket_ls() {
  local uri=${1:?oc_bucket_ls s3://bucket/prefix} bucket prefix
  uri=${uri#s3://}
  bucket=${uri%%/*}
  prefix=${uri#"$bucket"}
  prefix=${prefix#/}
  "$(oc_container_cmd)" run --rm --network "$OC_NETWORK" \
    -e AWS_ACCESS_KEY_ID="${RUSTFS_ACCESS_KEY:-rustfsadmin}" \
    -e AWS_SECRET_ACCESS_KEY="${RUSTFS_SECRET_KEY:-rustfsadmin}" \
    "$AIRFLOW_IMAGE" python -c '
import sys, boto3
s3 = boto3.client("s3", endpoint_url="http://rustfs:9000", region_name="us-east-1")
for page in s3.get_paginator("list_objects_v2").paginate(Bucket=sys.argv[1], Prefix=sys.argv[2]):
    for o in page.get("Contents", []):
        print(o["Key"])
' "$bucket" "$prefix"
}

# oc_junit_case <suite> <name> <pass|fail|error|skip> [message]: record one
# case in $OC_OUT/<suite>.junit.xml (valid after every call).
oc_junit_case() {
  python3 "$OC_ROOT/scripts/compute/junit.py" add "$OC_OUT/$1.junit.xml" "$1" "$2" "$3" "${4:-}"
}

# oc_xfail_check <product> <junit.xml...>: the strict expected-failure check
# against conformance/compute/<product>/xfail.toml.
oc_xfail_check() {
  local product=${1:?oc_xfail_check <product> <junit.xml...>}
  shift
  python3 "$OC_ROOT/scripts/compute/xfail_check.py" "$@" --xfail "$OC_DIR/$product/xfail.toml"
}
