#!/usr/bin/env bash
# Compat mode (design 74 D1509; DX0 Task 3, OC0 Task 7): the stock Temporal
# server on Loams Postgres, judged by Temporal's own persistence tests, a
# smoke, a soak with a history restart and a compute suspend, and the
# transaction-mode leg (expected to fail).
#
#   COMPUTE_PG_MAJOR=16 conformance/compute/temporal/run.sh   # the primary leg (default)
#   COMPUTE_PG_MAJOR=17 conformance/compute/temporal/run.sh   # the expected-failure leg
#
# Each leg needs a fresh stack: the major is fixed when compute1's timeline
# is created (DX1 R0.6). SOAK_DURATION (30m), SOAK_RATE (20) and
# OC_SKIP=persistence,soak,txn shorten a local run; a skipped part writes no
# cases, so its expected failures read as stale (the run fails, as it should
# for a partial run). Writes $OC_OUT/temporal-pg<major>*.
# Endpoint mode: direct (endpoint-modes.md; checked by selftest.sh recipe_endpoint_mode_matches_suite)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../lib.sh"

export COMPUTE_PG_MAJOR=${COMPUTE_PG_MAJOR:-16}
case $COMPUTE_PG_MAJOR in
  16) product=temporal ;;
  17) product=temporal/pg17 ;;
  *) echo "run.sh: COMPUTE_PG_MAJOR must be 16 or 17" >&2; exit 2 ;;
esac
# compute1's image for that major, from the Loams Postgres pins (R0.6).
COMPUTE_IMAGE=$(python3 - "$OC_ROOT/deploy/pins/loams-postgres-images.toml" "$COMPUTE_PG_MAJOR" <<'PY'
import sys, tomllib
p = tomllib.load(open(sys.argv[1], "rb"))["images"][f"compute-v{sys.argv[2]}"]
print(f"{p['repository']}@{p['digest']}")
PY
)
export COMPUTE_IMAGE
suite=temporal-pg$COMPUTE_PG_MAJOR
skip=",${OC_SKIP:-},"
mkdir -p "$OC_OUT"
rm -f "$OC_OUT/$suite".junit.xml "$OC_OUT/$suite"-persistence.junit.xml
admin() { oc_run "$TEMPORAL_ADMIN_TOOLS_IMAGE" temporal "$@" --address temporal-frontend:7233; }

# 1. The stack: Loams Postgres, the roles and databases, Temporal's schema,
# the four services.
oc_up temporal
if oc_temporal_wait; then
  oc_junit_case "$suite" temporal_frontend_serving pass
else
  oc_junit_case "$suite" temporal_frontend_serving fail "the frontend never reported SERVING"
  oc_compose --profile temporal logs --tail 50 temporal-schema temporal-frontend temporal-history || true
fi

# temporal_schema_setup: both databases at the newest version the pinned
# admin-tools image carries.
newest() {
  "$(oc_container_cmd)" run --rm --entrypoint sh "$TEMPORAL_ADMIN_TOOLS_IMAGE" -c \
    "ls /etc/temporal/schema/postgresql/v12/$1/versioned | sed 's/^v//' | sort -t. -k1,1n -k2,2n | tail -n 1"
}
msg=
for pair in temporal:temporal temporal_visibility:visibility; do
  db=${pair%%:*}
  want=$(newest "${pair#*:}")
  got=$(oc_psql "$(oc_pg_mode direct "$db")" 'SELECT curr_version FROM schema_version' 2>&1) || got="error: $got"
  [ "$got" = "$want" ] || msg="$msg $db is at '$got', want '$want';"
done
if [ -z "$msg" ]; then oc_junit_case "$suite" temporal_schema_setup pass; else oc_junit_case "$suite" temporal_schema_setup fail "$msg"; fi

# The default namespace: nothing else creates it since auto-setup is gone
# (R0.9's 72 h, the CLI's default retention).
admin operator namespace describe --namespace default >/dev/null 2>&1 ||
  admin operator namespace create --namespace default --retention 72h

# 2. Temporal's own persistence suite.
if [[ $skip != *,persistence,* ]]; then
  "$here/persistence.sh" "$suite-persistence"
fi

# 3. The smoke, then the soak.
"$here/soak.sh" "$suite" smoke || true
if [[ $skip != *,soak,* ]]; then
  "$here/soak.sh" "$suite" soak
fi

# 4. The transaction-mode leg: the four services through PgDog (transaction
# pooling) instead of the direct endpoint, and the smoke again. Planned as an
# expected failure; it passed on 2026-10-24 (SPIKE.md), so it must pass.
if [[ $skip != *,txn,* ]]; then
  services=(temporal-frontend temporal-history temporal-matching temporal-worker)
  oc_compose --txn --profile temporal up -d pgdog-txn
  LOAMS_PG_HOST=pgdog-txn LOAMS_PG_PORT=6432 \
    oc_compose --txn --profile temporal up -d --no-deps --force-recreate "${services[@]}"
  if oc_temporal_wait temporal-frontend:7233 180 &&
    SMOKE_ITERATIONS=10 "$here/soak.sh" "$suite-txn" smoke; then
    oc_junit_case "$suite" temporal_txn_mode_xfail pass
  else
    oc_junit_case "$suite" temporal_txn_mode_xfail fail "the server or the smoke failed through PgDog transaction pooling"
    oc_compose --txn --profile temporal logs --tail 40 temporal-history pgdog-txn >"$OC_OUT/$suite-txn.log" 2>&1 || true
  fi
  rm -f "$OC_OUT/$suite-txn.junit.xml"
  # Back to the direct endpoint.
  oc_compose --txn --profile temporal rm -s -f pgdog-txn
  oc_compose --profile temporal up -d --no-deps --force-recreate "${services[@]}"
fi

files=("$OC_OUT/$suite.junit.xml")
[ -f "$OC_OUT/$suite-persistence.junit.xml" ] && files+=("$OC_OUT/$suite-persistence.junit.xml")
oc_xfail_check "$product" "${files[@]}"
