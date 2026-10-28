#!/usr/bin/env bash
# The Edge executor's cases (design 74 §9.2; DX1 DX0 Task 6): Airflow's own
# `edge3` provider, two edge workers on the queue `default`, which reach only
# the API server, never the database. Run by ../run.sh on its stack, after
# OC0's cases; it writes $OC_OUT/airflow-edge.junit.xml, which ../run.sh
# checks against ../xfail.toml. Alone (the stack already up from ../run.sh):
#
#   conformance/compute/airflow/edge/run.sh
#
# Cases: airflow_edge_workers_register, airflow_edge_ha_no_double_run,
# airflow_edge_worker_kill_retries, airflow_edge_maintenance_mode,
# airflow_txn_mode_leg.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../../lib.sh"
# shellcheck source=conformance/compute/airflow/common.sh
. "$here/../common.sh"

suite=airflow-edge
skip=",${OC_SKIP:-},"
mkdir -p "$OC_OUT"
rm -f "$OC_OUT/$suite.junit.xml"
[ "$AIRFLOW_EXECUTOR" = "$AF_EDGE" ] || { echo "edge/run.sh: needs the Edge executor" >&2; exit 2; }
af_up "${AF_CORE[@]}" "${AF_WORKERS[@]}"
af wait-dags oc_ha_mapped edge_kill_probe edge_long edge_txn_probe

HB_TIMEOUT=60 # [scheduler] task_instance_heartbeat_timeout in ../compose.test.yaml

need_success() { # <dag> <run> [timeout]
  local s
  s=$(af wait-run "$1" "$2" "${3:-600}")
  echo "$1 $2: $s"
  [ "$s" = success ]
}
both_workers_up() {
  af wait-worker edge-worker-1 idle,running 300 >/dev/null
  af wait-worker edge-worker-2 idle,running 300 >/dev/null
}

# airflow_edge_workers_register: both workers online through the Edge API.
airflow_edge_workers_register() {
  both_workers_up
  af workers | tee /dev/stderr | awk -F'\t' '$2 == "idle" || $2 == "running" { n++ } END { exit !(n == 2) }'
}
af_case "$suite" airflow_edge_workers_register airflow_edge_workers_register

# airflow_edge_ha_no_double_run: two schedulers and two edge workers run 500
# mapped task instances, each with exactly one try and one effect, and both
# workers took a share.
airflow_edge_ha_no_double_run() {
  local run jobs
  run=$(af trigger oc_ha_mapped '{"n": 500, "sleep": 0}')
  need_success oc_ha_mapped "$run" 2400
  af_one_try_check oc_ha_mapped "$run" 500 work
  jobs=$(af_sql "SELECT count(*) || '/' || count(DISTINCT edge_worker) || '/' || max(try_number)
    FROM edge_job WHERE dag_id = 'oc_ha_mapped' AND run_id = '$run' AND task_id = 'work'")
  echo "edge jobs/workers/max try: $jobs"
  [ "$jobs" = 500/2/1 ]
}
af_case "$suite" airflow_edge_ha_no_double_run airflow_edge_ha_no_double_run

# airflow_edge_worker_kill_retries: `kill -9` of the worker running a task;
# the task is retried on the other worker within the heartbeat timeout (plus
# its detection interval, the retry delay and a poll), and has two tries.
airflow_edge_worker_kill_retries() {
  both_workers_up
  local run w1 t0 rc=0
  run=$(af trigger edge_kill_probe)
  af wait-ti edge_kill_probe "$run" victim running 300 >/dev/null
  sleep 5 # the first try records its start
  w1=$(af_sql "SELECT edge_worker FROM edge_job WHERE dag_id = 'edge_kill_probe' AND run_id = '$run' AND try_number = 1")
  echo "try 1 on $w1; kill -9"
  t0=$(date +%s)
  af_kill9 "airflow-$w1"
  need_success edge_kill_probe "$run" 900 || rc=1
  af_sql "SELECT try_number, edge_worker, state FROM edge_job WHERE dag_id = 'edge_kill_probe' AND run_id = '$run' ORDER BY 1"
  [ "$(af_sql "SELECT try_number FROM task_instance WHERE dag_id = 'edge_kill_probe' AND run_id = '$run'")" = 2 ] ||
    { echo "want try_number 2"; rc=1; }
  local w2
  w2=$(af_sql "SELECT edge_worker FROM edge_job WHERE dag_id = 'edge_kill_probe' AND run_id = '$run' AND try_number = 2")
  [ -n "$w2" ] && [ "$w2" != "$w1" ] || { echo "try 2 ran on '$w2', not the other worker"; rc=1; }
  af_effects edge_kill_probe "$run" victim | python3 -c '
import json, sys
t0, limit = float(sys.argv[1]), float(sys.argv[2])
rows = [json.loads(l) for l in sys.stdin if l.strip()]
by = {(r["try_number"], r["phase"]) for r in rows}
print("effects:", sorted(by))
assert by == {(1, "start"), (2, "start"), (2, "done")}, "want try 1 started only, try 2 started and done"
t2 = min(r["at"] for r in rows if r["try_number"] == 2)
print(f"try 2 started {t2 - t0:.0f}s after the kill (limit {limit:.0f}s)")
assert t2 - t0 <= limit
' "$t0" "$((HB_TIMEOUT + 10 + 5 + 45))" || rc=1
  af_compose up -d --no-deps "airflow-$w1"
  return $rc
}
af_case "$suite" airflow_edge_worker_kill_retries airflow_edge_worker_kill_retries

# airflow_edge_maintenance_mode: a worker put in maintenance while it runs a
# task takes no new task and finishes the running one, then rests in
# maintenance until it is taken out.
airflow_edge_maintenance_mode() {
  both_workers_up
  local run w short on rc=0
  run=$(af trigger edge_long '{"sleep": 45}')
  af wait-ti edge_long "$run" long running 300 >/dev/null
  w=$(af_sql "SELECT edge_worker FROM edge_job WHERE dag_id = 'edge_long' AND run_id = '$run'")
  echo "edge_long runs on $w; maintenance on"
  af send POST "/edge_worker/ui/worker/$w/maintenance" '{"maintenance_comment": "conformance: maintenance case"}' >/dev/null
  af wait-worker "$w" 'maintenance pending,maintenance mode' 120
  short=$(af trigger oc_ha_mapped '{"n": 12, "sleep": 1}')
  need_success oc_ha_mapped "$short" 600 || rc=1
  on=$(af_sql "SELECT count(*) FROM edge_job WHERE dag_id = 'oc_ha_mapped' AND run_id = '$short' AND edge_worker = '$w'")
  echo "tasks the worker in maintenance took: $on"
  [ "$on" = 0 ] || rc=1
  need_success edge_long "$run" 300 || rc=1
  af_effects edge_long "$run" long | python3 -c '
import json, sys
rows = [json.loads(l) for l in sys.stdin if l.strip()]
print("effects:", [(r["try_number"], r["phase"]) for r in rows])
assert sorted(r["phase"] for r in rows) == ["done", "start"] and all(r["try_number"] == 1 for r in rows)
' || rc=1
  af wait-worker "$w" 'maintenance mode' 120 || rc=1
  af send DELETE "/edge_worker/ui/worker/$w/maintenance" >/dev/null
  af wait-worker "$w" idle,running 120 || rc=1
  return $rc
}
af_case "$suite" airflow_edge_maintenance_mode airflow_edge_maintenance_mode

# airflow_txn_mode_leg: the schedulers and the triggerer through PgDog in
# transaction pooling (conformance/compute/pgdog-txn.yaml); a deferrable
# sensor and 30 mapped task instances, each with one try and one effect.
TXN_SERVICES=(airflow-scheduler-1 airflow-scheduler-2 airflow-triggerer)
airflow_txn_mode_leg() {
  local run s c conn i n since
  af_compose_txn up -d pgdog-txn
  since=$(af_sql "SELECT now()")
  LOAMS_PG_HOST=pgdog-txn LOAMS_PG_PORT=6432 af_compose_txn up -d --no-deps --force-recreate "${TXN_SERVICES[@]}"
  for s in "${TXN_SERVICES[@]}"; do
    c=$(af_container "$s")
    conn=$("$(oc_container_cmd)" inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$c" | grep '^AIRFLOW__DATABASE__SQL_ALCHEMY_CONN=')
    echo "$s: ${conn#*@}"
    case $conn in *@pgdog-txn:6432/airflow) ;; *) echo "$s is not on PgDog"; return 1 ;; esac
  done
  for ((i = 0; i < 100; i++)); do
    n=$(af_sql "SELECT count(*) FROM job WHERE state = 'running' AND start_date > '$since'
      AND latest_heartbeat > now() - interval '30 seconds' AND job_type IN ('SchedulerJob', 'TriggererJob')")
    [ "$n" -ge 3 ] && break
    sleep 3
  done
  echo "jobs started through PgDog: $n"
  [ "$n" -ge 3 ]
  run=$(af trigger edge_txn_probe '{"n": 30}')
  need_success edge_txn_probe "$run" 900
  [ "$(af_sql "SELECT state || '/' || try_number FROM task_instance WHERE dag_id = 'edge_txn_probe' AND run_id = '$run' AND task_id = 'deferred_wait'")" = success/1 ]
  af_one_try_check edge_txn_probe "$run" 30 work
}
if [[ $skip != *,txn,* ]]; then
  af_case "$suite" airflow_txn_mode_leg airflow_txn_mode_leg
  # Back to the direct endpoint.
  af_compose_txn rm -s -f pgdog-txn >/dev/null 2>&1 || true
  af_compose up -d --no-deps --force-recreate "${TXN_SERVICES[@]}"
fi
