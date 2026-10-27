#!/usr/bin/env bash
# Airflow 3.3 on Loams Postgres (design 68 §4.3, design 74 §9.2; DX1 DX0
# Task 6, which lands OC1 OC0 Task 6): the stock image with only
# configuration, the metadata database on Loams Postgres (direct endpoint),
# two schedulers, remote logs and object-storage XComs on the bucket, and the
# DAGs from a GitDagBundle. Judged by §68 §4.3's gate tests, each checking an
# effect (tries in the metadata database, objects in the bucket, the code
# that ran), then, with the Edge executor, by edge/run.sh.
#
#   conformance/compute/airflow/run.sh                                # Edge executor (the default)
#   AIRFLOW_EXECUTOR=LocalExecutor conformance/compute/airflow/run.sh # the comparison leg
#
# OC_PROJECT runs a stack of its own beside another (set it on a shared
# machine); OC_SKIP=edge leaves out edge/run.sh. The stack stays up for a
# look afterwards: `oc_compose --profile airflow --profile rustfs down -v`
# with the same OC_PROJECT removes it. Writes $OC_OUT/airflow*.junit.xml.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../lib.sh"
# shellcheck source=conformance/compute/airflow/common.sh
. "$here/common.sh"

case $AIRFLOW_EXECUTOR in
  "$AF_EDGE") suite=airflow product=airflow workers=("${AF_WORKERS[@]}") ;;
  LocalExecutor) suite=airflow-local product=airflow/local workers=() ;;
  *) echo "run.sh: AIRFLOW_EXECUTOR must be the Edge executor or LocalExecutor" >&2; exit 2 ;;
esac
skip=",${OC_SKIP:-},"
mkdir -p "$OC_OUT"
rm -f "$OC_OUT/$suite.junit.xml" "$OC_OUT/airflow-edge.junit.xml"
ADMIN=$(oc_pg_mode direct)

# 1. Loams Postgres, the bucket and the DAG repository; nothing of Airflow's yet.
af_compose stop "${AF_CORE[@]}" "${AF_WORKERS[@]}" >/dev/null 2>&1 || true
af_compose up -d compute1 rustfs git-server
oc_wait_pg "$ADMIN" 300
SHA1=$(af_git_reset | tail -n 1)
echo "DAG repository at $SHA1"

# airflow_db_migrate_empty: `airflow db migrate` from an empty database; the
# advisory lock it takes is released (pg_locks shows none afterwards).
airflow_db_migrate_empty() {
  oc_psql "$ADMIN" 'DROP DATABASE IF EXISTS airflow WITH (FORCE)'
  af_compose run --rm --no-deps -T compute-init
  [ "$(af_sql "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")" = 0 ]
  af_compose run --rm --no-deps -T airflow-init
  local ver tables locks
  ver=$(af_sql 'SELECT version_num FROM alembic_version')
  tables=$(af_sql "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")
  locks=$(oc_psql "$ADMIN" "SELECT count(*) FROM pg_locks l JOIN pg_database d ON d.oid = l.database
    WHERE l.locktype = 'advisory' AND d.datname = 'airflow'")
  echo "alembic version $ver, $tables tables, $locks advisory locks held"
  [ -n "$ver" ] && [ "$tables" -gt 50 ] && [ "$locks" = 0 ]
}
af_case "$suite" airflow_db_migrate_empty airflow_db_migrate_empty

# 2. Airflow: the API server, two schedulers, the DAG processor, the
# triggerer and, with the Edge executor, two edge workers.
af_up "${AF_CORE[@]}" "${workers[@]}"
af wait-dags oc_ha_mapped oc_xcom_big oc_log_probe oc_bundle_probe

ti_field() { # <dag> <run> <task> <field>
  af get "/api/v2/dags/$1/dagRuns/$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$2")/taskInstances/$3" |
    python3 -c 'import json, sys; print(json.load(sys.stdin)[sys.argv[1]])' "$4"
}
need_success() { # <dag> <run> [timeout]
  local s
  s=$(af wait-run "$1" "$2" "${3:-600}")
  echo "$1 $2: $s"
  [ "$s" = success ]
}

# airflow_rest_api_v2_smoke: trigger, list and fetch a task's log through
# /api/v2 with a token from /auth/token.
airflow_rest_api_v2_smoke() {
  local run
  af token | grep -q .
  run=$(af trigger oc_log_probe)
  af get '/api/v2/dags/oc_log_probe/dagRuns?limit=100' | grep -qF "\"$run\""
  need_success oc_log_probe "$run"
  af log oc_log_probe "$run" say 1 | grep -F "OC-LOG-MARKER $run"
}
af_case "$suite" airflow_rest_api_v2_smoke airflow_rest_api_v2_smoke

# airflow_remote_logs_after_worker_gone: the log is in the bucket, and the
# API server still serves it after the container that ran the task is removed.
airflow_remote_logs_after_worker_gone() {
  local run host svc
  run=$(af trigger oc_log_probe)
  need_success oc_log_probe "$run"
  host=$(ti_field oc_log_probe "$run" say hostname)
  svc=$(af_service_of_host "$host") ||
    svc=airflow-$(af_sql "SELECT edge_worker FROM edge_job WHERE dag_id = 'oc_log_probe' AND run_id = '$run'")
  echo "ran on $host ($svc)"
  oc_bucket_ls "s3://airflow-logs/logs/dag_id=oc_log_probe/run_id=$run/" | tee /dev/stderr | grep -q 'task_id=say'
  af_compose rm -s -f "$svc"
  local rc=0
  af log oc_log_probe "$run" say 1 | grep -F "OC-LOG-MARKER $run" || rc=$?
  af_compose up -d --no-deps "$svc"
  return $rc
}
af_case "$suite" airflow_remote_logs_after_worker_gone airflow_remote_logs_after_worker_gone

# airflow_xcom_object_storage: a 200 kB XCom goes to the bucket (the database
# holds only its path) and the next task reads it back unchanged.
airflow_xcom_object_storage() {
  local run val small
  run=$(af trigger oc_xcom_big)
  need_success oc_xcom_big "$run"
  val=$(af_sql "SELECT value #>> '{}' FROM xcom WHERE dag_id = 'oc_xcom_big' AND run_id = '$run' AND task_id = 'produce' AND key = 'return_value'")
  small=$(af_sql "SELECT length(value::text) FROM xcom WHERE dag_id = 'oc_xcom_big' AND run_id = '$run' AND task_id = 'consume' AND key = 'return_value'")
  echo "produce's XCom in the database: $val; consume's: $small bytes"
  case $val in s3://*airflow-xcom/xcom/oc_xcom_big/*) ;; *) echo "not a bucket path"; return 1 ;; esac
  oc_bucket_ls s3://airflow-xcom/xcom/oc_xcom_big/ | grep -F "$run" | grep -q produce
  [ "$small" -lt 200 ]
}
af_case "$suite" airflow_xcom_object_storage airflow_xcom_object_storage

# airflow_ha_no_double_run: two schedulers, 500 mapped task instances, each
# with exactly one try and exactly one effect.
airflow_ha_no_double_run() {
  local run alive
  alive=$(af_sql "SELECT count(*) FROM job WHERE job_type = 'SchedulerJob' AND state = 'running'
    AND latest_heartbeat > now() - interval '60 seconds'")
  echo "schedulers alive: $alive"
  [ "$alive" = 2 ]
  run=$(af trigger oc_ha_mapped '{"n": 500, "sleep": 0}')
  need_success oc_ha_mapped "$run" 2400
  af_one_try_check oc_ha_mapped "$run" 500 work
}
af_case "$suite" airflow_ha_no_double_run airflow_ha_no_double_run

# airflow_scheduler_kill: `kill -9` one scheduler mid-run; the other finishes
# the run, and every task instance ran.
airflow_scheduler_kill() {
  local run i running=0 rc=0
  run=$(af trigger oc_ha_mapped '{"n": 60, "sleep": 8}')
  for ((i = 0; i < 120; i++)); do
    running=$(af_sql "SELECT count(*) FROM task_instance WHERE dag_id = 'oc_ha_mapped' AND run_id = '$run' AND state = 'running'")
    [ "$running" -gt 0 ] && break
    sleep 2
  done
  echo "$running task instances running; killing airflow-scheduler-1"
  af_kill9 airflow-scheduler-1
  need_success oc_ha_mapped "$run" 1200 || rc=1
  af_sql "SELECT state, count(*) FROM task_instance WHERE dag_id = 'oc_ha_mapped' AND run_id = '$run' GROUP BY 1"
  [ "$(af_sql "SELECT count(*) FROM task_instance WHERE dag_id = 'oc_ha_mapped' AND run_id = '$run' AND task_id = 'work' AND state = 'success'")" = 60 ] || rc=1
  # Every task instance ran (an extra try after the kill is Airflow's to
  # make; the count is reported, a missing one fails).
  af_effects oc_ha_mapped "$run" work | python3 -c '
import collections, json, sys
per = collections.Counter(json.loads(l)["map_index"] for l in sys.stdin if l.strip())
missing = sorted(set(range(60)) - set(per))
print(f"effects from {len(per)} task instances; ran twice: {sorted(i for i, c in per.items() if c > 1)}")
sys.exit(1 if missing else 0)' || rc=1
  af_compose up -d --no-deps airflow-scheduler-1
  return $rc
}
af_case "$suite" airflow_scheduler_kill airflow_scheduler_kill

# airflow_git_bundle_version: a run records the bundle commit it started on,
# and a push during the run changes neither that nor the code it runs; the
# next run gets the new commit.
airflow_git_bundle_version() {
  local run run2 v sha2 i
  run=$(af trigger oc_bundle_probe '{"sleep": 60}')
  v=$(af_sql "SELECT bundle_version FROM dag_run WHERE dag_id = 'oc_bundle_probe' AND run_id = '$run'")
  echo "run $run at $v (repository at $SHA1)"
  [ "$v" = "$SHA1" ]
  af wait-ti oc_bundle_probe "$run" wait running 300 >/dev/null
  sha2=$(af_git_commit_version v2 | tail -n 1)
  echo "pushed $sha2 during the run"
  need_success oc_bundle_probe "$run" 600
  v=$(af_sql "SELECT bundle_version FROM dag_run WHERE dag_id = 'oc_bundle_probe' AND run_id = '$run'")
  [ "$v" = "$SHA1" ] || { echo "the run's bundle version changed to $v"; return 1; }
  [ "$(af_sql "SELECT value #>> '{}' FROM xcom WHERE dag_id = 'oc_bundle_probe' AND run_id = '$run' AND task_id = 'which'")" = v1 ] ||
    { echo "the running DAG run ran the new commit's code"; return 1; }
  for ((i = 0; i < 60; i++)); do
    [ "$(af_sql "SELECT count(*) FROM dag_version WHERE dag_id = 'oc_bundle_probe' AND bundle_version = '$sha2'")" -gt 0 ] && break
    sleep 3
  done
  run2=$(af trigger oc_bundle_probe '{"sleep": 0}')
  need_success oc_bundle_probe "$run2" 600
  v=$(af_sql "SELECT bundle_version FROM dag_run WHERE dag_id = 'oc_bundle_probe' AND run_id = '$run2'")
  echo "next run $run2 at $v"
  [ "$v" = "$sha2" ]
  [ "$(af_sql "SELECT value #>> '{}' FROM xcom WHERE dag_id = 'oc_bundle_probe' AND run_id = '$run2' AND task_id = 'which'")" = v2 ]
}
af_case "$suite" airflow_git_bundle_version airflow_git_bundle_version

files=("$OC_OUT/$suite.junit.xml")
if [ "$suite" = airflow ] && [[ $skip != *,edge,* ]]; then
  AF_SHA1=$SHA1 "$here/edge/run.sh"
  files+=("$OC_OUT/airflow-edge.junit.xml")
fi
oc_xfail_check "$product" "${files[@]}"
