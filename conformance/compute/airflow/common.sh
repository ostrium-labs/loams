#!/usr/bin/env bash
# Shared by airflow/run.sh and airflow/edge/run.sh (DX1 DX0 Task 6). Source
# it after lib.sh. The stack is compose.compute.yaml's `airflow` profile,
# RustFS (`rustfs` profile, the bucket) and this directory's test overlay.
# shellcheck shell=bash

AF_DIR=$OC_DIR/airflow
# Once: edge/run.sh sources this again under run.sh, and an overlay given
# twice makes compose refuse duplicated list items (second nightly, T6.R12).
case ":${OC_COMPOSE_EXTRA:-}:" in
  *":$AF_DIR/compose.test.yaml:"*) ;;
  *) export OC_COMPOSE_EXTRA="$AF_DIR/compose.test.yaml${OC_COMPOSE_EXTRA:+:$OC_COMPOSE_EXTRA}" ;;
esac
if [ -n "${OC_PROJECT:-}" ] && [ "$OC_NETWORK" = loams_default ]; then
  OC_NETWORK=${OC_PROJECT}_default
fi
AF_PROFILES=(--profile airflow --profile rustfs)
AF_EDGE=airflow.providers.edge3.executors.EdgeExecutor
export AIRFLOW_EXECUTOR=${AIRFLOW_EXECUTOR:-$AF_EDGE}
AF_CORE=(airflow-api-server airflow-scheduler-1 airflow-scheduler-2 airflow-dag-processor airflow-triggerer)
AF_WORKERS=(airflow-edge-worker-1 airflow-edge-worker-2)

# af_compose <args>: compose with the airflow and rustfs profiles and the overlay.
af_compose() { oc_compose "${AF_PROFILES[@]}" "$@"; }
af_compose_txn() { oc_compose --txn "${AF_PROFILES[@]}" "$@"; }

# af <af.py args>: the REST client, from a container on the stack's network.
af() {
  "$(oc_container_cmd)" run --rm --network "$OC_NETWORK" -v "$AF_DIR/af.py:/af.py:ro" \
    "$PYTHON_IMAGE" python3 -I /af.py "$@"
}

# af_sql <sql>: as the airflow role, through the direct endpoint.
af_sql() { oc_psql "$(oc_pg_mode direct airflow airflow:airflow)" "$1"; }

# af_container <service>: the container of a service ("" when none).
af_container() { af_compose ps -a -q "$1" 2>/dev/null | head -n 1; }

# af_kill9 <service>: SIGKILL its container, and keep it down (the restart
# policy would otherwise bring it back, which is not what a dead host does).
af_kill9() {
  local c
  c=$(af_container "$1")
  [ -n "$c" ] || { echo "af: no container for $1" >&2; return 1; }
  "$(oc_container_cmd)" kill -s KILL "$c" >/dev/null
  af_compose stop -t 0 "$1" >/dev/null 2>&1 || true
}

# af_service_of_host <hostname>: the airflow service whose container has it.
af_service_of_host() {
  local s c h
  for s in "${AF_CORE[@]}" "${AF_WORKERS[@]}"; do
    c=$(af_container "$s")
    [ -n "$c" ] || continue
    h=$("$(oc_container_cmd)" inspect -f '{{.Config.Hostname}}' "$c" 2>/dev/null) || continue
    if [ "$h" = "$1" ]; then echo "$s"; return 0; fi
  done
  return 1
}

# af_git <shell script>: in the DAG repository's volume, as the image's user.
af_git() {
  "$(oc_container_cmd)" run --rm --user 50000:0 -e HOME=/tmp \
    -v "$(af_git_volume):/srv/git" -v "$AF_DIR/dags:/src/dags:ro" \
    --entrypoint bash "$AIRFLOW_IMAGE" -c "set -euo pipefail; git config --global user.name oc; git config --global user.email oc@loams.invalid; $1"
}
af_git_volume() { echo "${OC_PROJECT:-loams}_airflow-git"; }

# af_git_reset: a fresh repository with dags/ as its first commit; prints its SHA.
af_git_reset() {
  "$(oc_container_cmd)" volume create "$(af_git_volume)" >/dev/null 2>&1 || true
  "$(oc_container_cmd)" run --rm --user 0 -v "$(af_git_volume):/srv/git" --entrypoint bash "$AIRFLOW_IMAGE" \
    -c 'rm -rf /srv/git/dags.git && git init -q --bare -b main /srv/git/dags.git && chown -R 50000:0 /srv/git'
  af_git 'git clone -q /srv/git/dags.git /tmp/w && cp /src/dags/*.py /tmp/w/ && cd /tmp/w &&
    git add -A && git commit -qm "conformance DAGs" && git push -q origin main && git rev-parse HEAD'
}

# af_git_commit_version <version>: change oc_bundle_probe's VERSION; prints the new SHA.
af_git_commit_version() {
  af_git "git clone -q /srv/git/dags.git /tmp/w && cd /tmp/w &&
    sed -i 's/^VERSION = .*/VERSION = \"$1\"/' oc_bundle_probe.py &&
    git commit -qam 'oc_bundle_probe $1' && git push -q origin main && git rev-parse HEAD"
}

# af_effects <dag_id> <run_id> [task_id]: the effects of a run, one JSON per line,
# each with its map index (from the key) added.
af_effects() {
  "$(oc_container_cmd)" run --rm --network "$OC_NETWORK" \
    -e AWS_ACCESS_KEY_ID="${RUSTFS_ACCESS_KEY:-rustfsadmin}" \
    -e AWS_SECRET_ACCESS_KEY="${RUSTFS_SECRET_KEY:-rustfsadmin}" \
    --entrypoint python "$AIRFLOW_IMAGE" -c '
import json, sys, boto3
s3 = boto3.client("s3", endpoint_url="http://rustfs:9000", region_name="us-east-1")
prefix = "/".join(sys.argv[1:4]) + "/"
for page in s3.get_paginator("list_objects_v2").paginate(Bucket="airflow-effects", Prefix=prefix):
    for o in page.get("Contents", []):
        parts = o["Key"].split("/")
        body = json.loads(s3.get_object(Bucket="airflow-effects", Key=o["Key"])["Body"].read())
        body.update(task_id=parts[2], map_index=int(parts[3]))
        print(json.dumps(body))
' "$1" "$2" "${3:-}"
}

# af_up [services...]: everything the run needs, waiting for the API server.
af_up() {
  af_compose up -d "$@"
  oc_wait_http http://airflow-api-server:8080/api/v2/version 600
}

# af_case <suite> <name> <function>: one JUnit case; the run goes on after a failure.
af_case() {
  local suite=$1 name=$2 fn=$3 log rc
  log=$(mktemp)
  echo "---- $name"
  set +e
  (set -e; "$fn") 2>&1 | tee "$log"
  rc=${PIPESTATUS[0]}
  set -e
  if [ "$rc" -eq 0 ]; then
    oc_junit_case "$suite" "$name" pass
    echo "ok   $name"
  else
    oc_junit_case "$suite" "$name" fail "$(tail -n 15 "$log")"
    echo "FAIL $name"
  fi
  rm -f "$log"
}

# af_one_try_check <dag_id> <run_id> <n> <task_id>: every one of the n mapped
# task instances succeeded with exactly one try and left exactly one effect.
af_one_try_check() {
  local dag=$1 run=$2 n=$3 task=$4 got hist
  got=$(af_sql "SELECT count(*) || '/' || count(*) FILTER (WHERE state = 'success' AND try_number = 1)
    FROM task_instance WHERE dag_id = '$dag' AND run_id = '$run' AND task_id = '$task'")
  hist=$(af_sql "SELECT count(*) FROM task_instance_history WHERE dag_id = '$dag' AND run_id = '$run'")
  echo "task instances (all/success at try 1): $got; earlier tries: $hist"
  [ "$got" = "$n/$n" ] || { echo "want $n/$n"; return 1; }
  [ "$hist" = 0 ] || { echo "some task instance was tried more than once"; return 1; }
  af_effects "$dag" "$run" "$task" >"$OC_OUT/effects.$$.jsonl"
  python3 - "$OC_OUT/effects.$$.jsonl" "$n" <<'PY'
import collections, json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
n = int(sys.argv[2])
per = collections.Counter(r["map_index"] for r in rows)
hosts = collections.Counter(r["host"] for r in rows)
print(f"effects: {len(rows)} from {len(per)} task instances; per host: {dict(hosts)}")
twice = sorted(i for i, c in per.items() if c > 1)
missing = sorted(set(range(n)) - set(per))
if twice or missing:
    print(f"ran more than once: {twice[:20]}; never ran: {missing[:20]}")
    sys.exit(1)
PY
  rm -f "$OC_OUT/effects.$$.jsonl"
}
