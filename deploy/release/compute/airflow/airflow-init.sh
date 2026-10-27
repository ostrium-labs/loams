#!/usr/bin/env bash
# The airflow profile's one-shot (DX1 DX0 Task 6, OC1 OC0 Task 6): the
# buckets for remote logs, XComs and the conformance runs' effects on the
# stack's bucket (RustFS with the `rustfs` profile, or AIRFLOW_S3_ENDPOINT),
# then `airflow db migrate` on Loams Postgres. Idempotent.
#
#   airflow-init.sh [buckets|migrate|all]   # default: all
#
# Runs in the pinned Airflow image, as the image's own user, with the
# profile's environment (the `loams_s3` connection comes from
# AIRFLOW_CONN_LOAMS_S3).
set -euo pipefail

buckets() {
  python - <<'PY'
import os, sys, time
from airflow.providers.amazon.aws.hooks.s3 import S3Hook

names = os.environ.get("AIRFLOW_INIT_BUCKETS", "airflow-logs airflow-xcom airflow-effects").split()
hook = S3Hook(aws_conn_id="loams_s3")
deadline = time.monotonic() + float(os.environ.get("AIRFLOW_INIT_WAIT_SECONDS", "180"))
while True:
    try:
        have = {b["Name"] for b in hook.get_conn().list_buckets().get("Buckets", [])}
        break
    except Exception as e:  # the bucket service is still starting
        if time.monotonic() > deadline:
            sys.exit(f"airflow-init: the bucket service did not answer: {e}")
        time.sleep(2)
for name in names:
    if name not in have:
        hook.create_bucket(bucket_name=name)
    print(f"airflow-init: bucket {name} ready")
PY
}

migrate() {
  airflow db migrate
  echo "airflow-init: metadata database migrated"
}

case ${1:-all} in
  buckets) buckets ;;
  migrate) migrate ;;
  all) buckets; migrate ;;
  *) echo "usage: airflow-init.sh [buckets|migrate|all]" >&2; exit 2 ;;
esac
