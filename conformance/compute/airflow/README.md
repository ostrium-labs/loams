# Airflow 3.3 with the Edge executor on Loams Postgres

DX1 DX0 Task 6 (design 68 §4.3, design 74 §9.2; lands OC1's OC0 Task 6). The
stock `apache/airflow:3.3.2-python3.12` image, unmodified, with the pinned
`apache-airflow-providers-edge3` 4.3.2 installed at start (DX1 ruling T1.R2),
and only configuration:

| Need | Setting | On Loams |
|---|---|---|
| Metadata database | `[database] sql_alchemy_conn` | Loams Postgres, **direct** endpoint (`compute1`), database `airflow`, role `airflow` (DX0 Task 2) |
| Schema | `airflow db migrate` (one-shot `airflow-init`) | from empty; the advisory lock is released |
| HA | two schedulers, `use_row_level_locking = True` | row locks with `SKIP LOCKED` on Loams Postgres |
| Executor | `airflow.providers.edge3.executors.EdgeExecutor`, Edge API on the API server | two edge workers, queue `default`; they reach only the API server |
| Logs | `remote_logging`, `remote_base_log_folder = s3://airflow-logs/logs`, connection `loams_s3` | the bucket (RustFS, `rustfs` profile) |
| XComs | `XComObjectStorageBackend`, `s3://loams_s3@airflow-xcom/xcom`, threshold 1 kB | the bucket |
| DAGs | `GitDagBundle` (`oc-git`) and a `LocalDagBundle` (`oc-edge`) | a plain `git daemon` in the test overlay (T6.R3) |

Never on Loams SQL. This runs Airflow on Loams; it is not an Airflow
implementation.

```sh
export OC_PROJECT=af1   # a stack of its own (recommended on a shared machine)
conformance/compute/airflow/run.sh                                 # Edge executor, then edge/run.sh
AIRFLOW_EXECUTOR=LocalExecutor conformance/compute/airflow/run.sh  # the comparison leg
. conformance/compute/lib.sh && OC_COMPOSE_EXTRA=conformance/compute/airflow/compose.test.yaml \
  oc_compose --profile airflow --profile rustfs down -v             # remove the stack
```

With podman, point `DOCKER_HOST` at the user's podman socket. Results go to
`$OC_OUT/airflow.junit.xml`, `airflow-edge.junit.xml` and
`airflow-local.junit.xml`, and are checked strictly against `xfail.toml` (Edge)
and `local/xfail.toml` (LocalExecutor).

## Files

| Path | What |
|---|---|
| `run.sh` | the stack, OC0's seven cases, then `edge/run.sh` |
| `edge/run.sh` | the Edge executor's cases and the transaction-mode leg |
| `common.sh` | shared helpers: compose with the overlay, the REST client, the effects, the DAG repository |
| `af.py` | a stdlib client for `/api/v2`, `/auth/token` and the Edge plugin's `/edge_worker/ui` API |
| `compose.test.yaml` | the test overlay: `git-server`, the bundles, shorter timeouts, no host ports |
| `dags/` | OC0's DAGs, pushed to the Git repository the `GitDagBundle` reads |
| `edge/dags/` | the Edge cases' DAGs (a `LocalDagBundle`) |
| `auth/passwords.json` | the test stack's Simple auth manager password (`admin`/`admin`) |

## Cases

Every case checks an effect, not an exit code: each task execution writes one
object to `s3://airflow-effects/<dag>/<run>/<task>/<map index>/`, so a task
that ran twice shows up even when the metadata database records one try.

| Case | What it checks |
|---|---|
| `airflow_db_migrate_empty` | `airflow db migrate` from a dropped and re-created database; `alembic_version` set; no advisory lock held afterwards (`pg_locks`) |
| `airflow_rest_api_v2_smoke` | a token from `/auth/token`; trigger, list and read a task's log through `/api/v2` |
| `airflow_remote_logs_after_worker_gone` | the task log is in the bucket, and served by the API server after the container that ran the task is removed |
| `airflow_xcom_object_storage` | a 200 kB XCom is a bucket path in the database and round-trips unchanged; a small one stays inline |
| `airflow_ha_no_double_run` | two live schedulers; 500 mapped task instances, each with one try and one effect |
| `airflow_scheduler_kill` | `kill -9` of one scheduler mid-run; the other finishes it, and every task instance ran |
| `airflow_git_bundle_version` | a run records its bundle commit; a push during the run changes neither that nor the code it runs; the next run gets the new commit |
| `airflow_edge_workers_register` | both edge workers online through the Edge API |
| `airflow_edge_ha_no_double_run` | 500 mapped task instances on two schedulers and two edge workers: one try and one effect each, both workers used |
| `airflow_edge_worker_kill_retries` | `kill -9` of the worker running a task: try 2 on the other worker within the heartbeat timeout (60 s, plus detection, retry delay and a poll), two tries in all |
| `airflow_edge_maintenance_mode` | a worker in maintenance takes none of 12 new tasks and finishes its running one, then rests in maintenance until taken out |
| `airflow_txn_mode_leg` | the schedulers and the triggerer through PgDog transaction pooling: a deferrable sensor and 30 mapped tasks, one try each |

The LocalExecutor leg runs OC0's seven cases with the tasks in the
schedulers (`airflow-local::…`), for comparison.

## Status

Written, **not yet run end to end** (DX1 ruling T6.R9, 2026-10-27). Each
pull request runs only the self-test (`conformance/compute/selftest.sh`: the
Python parses, the effects module copies match, the REST client's log
rendering, and the overlay resolves with no host ports). Both `xfail.toml`
files are empty and strict: the first nightly fails on any failure until its
results are recorded there, each with a reason and an issue.

Rulings: the DX1 plan's T6.R1–T6.R9; endpoint modes:
[endpoint-modes.md](../endpoint-modes.md).
