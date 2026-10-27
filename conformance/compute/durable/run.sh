#!/usr/bin/env bash
# DX1 DX0 Task 2's tests: the compute profiles' databases and roles on Loams
# Postgres (deploy/release/compute/durable-init.sh) and the endpoint modes
# (conformance/compute/endpoint-modes.md). Needs a container runtime; starts
# only the Postgres stack (compute1 and what it needs) and, for the
# transaction-mode check, the PgDog overlay. Writes $OC_OUT/durable.junit.xml
# and checks it against durable/xfail.toml.
#
#   conformance/compute/durable/run.sh
#
# Tests: durable_init_idempotent, durable_init_separate_databases,
# durable_init_roles_isolated, pg_modes_reachable_direct,
# pg_modes_reachable_txn, pg_modes_reachable_session (expected to fail until
# a session-pooled endpoint exists, #1129).
set -euo pipefail
# shellcheck source=conformance/compute/lib.sh
. "$(dirname "$0")/../lib.sh"

SUITE=durable
JUNIT=$OC_OUT/$SUITE.junit.xml
mkdir -p "$OC_OUT"
rm -f "$JUNIT"
PROFILES=(--profile temporal --profile dbos --profile airflow)
ADMIN=$(oc_pg_mode direct)
PRODUCT_DBS=(temporal temporal_visibility airflow dbos_py dbos_ts dbos_go dbos_java)

case_() { oc_junit_case "$SUITE" "$@"; }

# run <name> <function>: one JUnit case per check; the run goes on after a failure.
run() {
  local name=$1 fn=$2 log
  log=$(mktemp)
  # Not in an `if`: errexit is ignored there, even in a subshell.
  set +e
  (set -e; "$fn") >"$log" 2>&1
  local rc=$?
  set -e
  if [ "$rc" -eq 0 ]; then
    case_ "$name" pass
    echo "ok   $name"
  else
    case_ "$name" fail "$(tail -n 20 "$log")"
    echo "FAIL $name"
    sed 's/^/     /' "$log"
  fi
  rm -f "$log"
}

init() {
  oc_compose "${PROFILES[@]}" run --rm --no-TTY compute-init
  oc_compose --profile temporal run --rm --no-TTY temporal-schema
}

# Everything durable-init decides: roles and their attributes, databases,
# owners and ACLs, and Temporal's schema versions.
snapshot() {
  oc_psql "$ADMIN" "
    SELECT 'role', rolname, rolcanlogin, rolcreatedb, rolsuper FROM pg_roles
     WHERE rolname IN ('temporal', 'airflow', 'postgres') ORDER BY 2;"
  oc_psql "$ADMIN" "
    SELECT 'db', datname, pg_get_userbyid(datdba), coalesce(datacl::text, '') FROM pg_database
     WHERE datname = ANY ('{$(IFS=,; echo "${PRODUCT_DBS[*]}")}') ORDER BY 2;"
  local db
  for db in temporal temporal_visibility; do
    oc_psql "$(oc_pg_mode direct "$db")" "SELECT '$db', curr_version, min_compatible_version FROM schema_version;"
  done
}

durable_init_idempotent() {
  init
  snapshot >"$OC_OUT/durable.snapshot.1"
  init
  snapshot >"$OC_OUT/durable.snapshot.2"
  diff -u "$OC_OUT/durable.snapshot.1" "$OC_OUT/durable.snapshot.2"
  # And it did create what it says.
  [ "$(grep -c '^db|' "$OC_OUT/durable.snapshot.2")" -eq "${#PRODUCT_DBS[@]}" ]
  grep -q '^temporal|1\.' "$OC_OUT/durable.snapshot.2"
  grep -q '^temporal_visibility|1\.' "$OC_OUT/durable.snapshot.2"
}

# One database per product, each owned by its role, none open to PUBLIC, and
# no product's tables in another's database or in `postgres` (D1529).
durable_init_separate_databases() {
  local row db owner acl
  declare -A want=([temporal]=temporal [temporal_visibility]=temporal [airflow]=airflow
    [dbos_py]=postgres [dbos_ts]=postgres [dbos_go]=postgres [dbos_java]=postgres)
  while IFS='|' read -r _ db owner acl; do
    [ "${want[$db]:-}" = "$owner" ] || { echo "$db is owned by $owner, want ${want[$db]:-?}"; return 1; }
    case ",${acl#\{}" in *',=c'* | *',=Tc'* | *',=CTc'*) echo "$db: PUBLIC may connect ($acl)"; return 1 ;; esac
    unset "want[$db]"
  done < <(grep '^db|' "$OC_OUT/durable.snapshot.2")
  [ "${#want[@]}" -eq 0 ] || { echo "missing databases: ${!want[*]}"; return 1; }
  # Temporal's tables only in Temporal's databases.
  [ "$(oc_psql "$(oc_pg_mode direct temporal)" "SELECT count(*) FROM pg_tables WHERE tablename = 'executions'")" = 1 ]
  [ "$(oc_psql "$(oc_pg_mode direct temporal_visibility)" "SELECT count(*) FROM pg_tables WHERE tablename = 'executions_visibility'")" = 1 ]
  for db in postgres airflow dbos_py dbos_ts dbos_go dbos_java; do
    row=$(oc_psql "$(oc_pg_mode direct "$db")" "SELECT count(*) FROM pg_tables WHERE tablename IN ('executions', 'executions_visibility', 'schema_version')")
    [ "$row" = 0 ] || { echo "$db holds Temporal tables"; return 1; }
  done
}

# Each role reaches its own databases and not another product's.
durable_init_roles_isolated() {
  [ "$(oc_psql "$(oc_pg_mode direct temporal temporal:temporal)" 'SELECT 1')" = 1 ]
  [ "$(oc_psql "$(oc_pg_mode direct airflow airflow:airflow)" 'SELECT 1')" = 1 ]
  [ "$(oc_psql "$(oc_pg_mode direct dbos_py postgres:dbos)" 'SELECT 1')" = 1 ]
  if oc_psql "$(oc_pg_mode direct airflow temporal:temporal)" 'SELECT 1' 2>/dev/null; then
    echo "temporal can connect to airflow"; return 1
  fi
  if oc_psql "$(oc_pg_mode direct temporal airflow:airflow)" 'SELECT 1' 2>/dev/null; then
    echo "airflow can connect to temporal"; return 1
  fi
}

pg_modes_reachable_direct() {
  [ "$(oc_psql "$ADMIN" 'SELECT 1')" = 1 ]
  local major expect=${COMPUTE_PG_MAJOR:-17}
  major=$(oc_psql "$ADMIN" 'SHOW server_version_num')
  major=$((major / 10000))
  echo "Loams Postgres major $major (want $expect)"
  [ "$major" = "$expect" ]
}

pg_modes_reachable_txn() {
  oc_compose --txn --profile dbos up -d pgdog-txn
  local i
  for i in $(seq 1 30); do
    if [ "$(oc_psql "$(oc_pg_mode txn dbos_py postgres:dbos)" 'SELECT 1' 2>/dev/null)" = 1 ]; then
      oc_compose --txn --profile dbos rm -s -f pgdog-txn >/dev/null 2>&1 || true
      return 0
    fi
    sleep 2
  done
  oc_compose --txn --profile dbos logs --tail 30 pgdog-txn || true
  oc_compose --txn --profile dbos rm -s -f pgdog-txn >/dev/null 2>&1 || true
  return 1
}

pg_modes_reachable_session() {
  local url
  url=$(oc_pg_mode session) || return 1
  [ "$(oc_psql "$url" 'SELECT 1')" = 1 ]
}

run durable_init_idempotent durable_init_idempotent
run durable_init_separate_databases durable_init_separate_databases
run durable_init_roles_isolated durable_init_roles_isolated
run pg_modes_reachable_direct pg_modes_reachable_direct
run pg_modes_reachable_txn pg_modes_reachable_txn
run pg_modes_reachable_session pg_modes_reachable_session
oc_xfail_check "$SUITE" "$JUNIT"
