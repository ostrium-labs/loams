#!/usr/bin/env bash
# Start one compute profile (temporal or airflow) on the release stack's
# Loams Postgres. Run from deploy/release. The airflow profile also needs the
# rustfs profile for its bucket.
set -euo pipefail
profile=${1:?usage: compute-profile-up.sh <temporal|airflow>}
extra=()
[ "$profile" = airflow ] && extra=(--profile rustfs)
docker compose --env-file .env --env-file ../../conformance/compute/pins.env \
  -f compose.yaml -f compose.compute.yaml --profile "$profile" "${extra[@]}" up -d
