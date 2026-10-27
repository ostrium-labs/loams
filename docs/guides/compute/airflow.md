# Airflow 3.3 on Loams

Status: **preview, written but not yet run end to end** (design 68 §4.3; OC0
Task 6, landed by DX0 Task 6). This runs Airflow itself, unmodified, on Loams.
It is not an Airflow implementation. For the Edge executor, see
[../durable/airflow.md](../durable/airflow.md).

Endpoint mode: **direct**

## Settings

These are only configuration. The stock `apache/airflow:3.3.2-python3.12`
image is pinned by digest.

| Need | Setting | On Loams |
|---|---|---|
| Metadata database | `[database] sql_alchemy_conn` | Loams Postgres through the **direct** endpoint, database `airflow`, role `airflow` |
| Schema | `airflow db migrate` (the one-shot `airflow-init`) | from empty; the session advisory lock it takes is released afterwards |
| HA | two schedulers, `[scheduler] use_row_level_locking = True` | row locks with `SKIP LOCKED` |
| Logs | `[logging] remote_logging`, `remote_base_log_folder = s3://airflow-logs/logs`, connection `loams_s3` | the bucket, so a log outlives its worker |
| XComs | `airflow.providers.common.io.xcom.backend.XComObjectStorageBackend`, path `s3://loams_s3@airflow-xcom/xcom`, threshold 1 kB | the bucket |
| DAGs | `GitDagBundle` (`[dag_processor] dag_bundle_config_list`) | a Git repository; each run records its bundle commit |

Never Loams SQL. A transaction pooler is not supported yet: `airflow db
migrate` needs a session, and the schedulers and the triggerer get
transaction mode only after the conformance leg `airflow_txn_mode_leg`
passes.

## Start it

From `deploy/release`, with the `airflow` and `rustfs` profiles:

```sh
docker compose --env-file .env --env-file ../../conformance/compute/pins.env \
  -f compose.yaml -f compose.compute.yaml --profile airflow --profile rustfs up -d
```

## What the conformance run checks

`conformance/compute/airflow/run.sh` runs the following cases, each judged by
its effects:
- `airflow_db_migrate_empty`
- `airflow_rest_api_v2_smoke`
- `airflow_remote_logs_after_worker_gone`
- `airflow_xcom_object_storage`
- `airflow_ha_no_double_run`
- `airflow_scheduler_kill`
- `airflow_git_bundle_version`

The cases are described in
[conformance/compute/airflow/README.md](../../../conformance/compute/airflow/README.md).
