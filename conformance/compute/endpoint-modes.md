# Endpoint modes: which Loams Postgres endpoint each product uses, and why

DX1 DX0 Task 2 (design 74 §8.2, design 68 §5.3). Rulings R0.7 and R0.8 of the
[DX1 plan](../../docs/plans/2026-10-21-dx1-durable-execution-compat.md).

## What the release compose offers (0.1.1)

| Mode | `oc_pg_mode` | Where | State |
|---|---|---|---|
| **Direct** | `direct` | the included stack's `compute1` on `compute1:55433` (the `loams_default` network); with `--profile postgres-control`, each `pg-control` endpoint's compute on `ep-<ulid>:55433` (`LOAMS_PG_DIRECT_URL`) | available |
| Session-pooled | `session` | none | **missing**: [#1129](https://github.com/ostrium-labs/loams/issues/1129) (PG2c, GW1). `oc_pg_mode session` fails with that issue, and `pg_modes_reachable_session` is an expected failure |
| Transaction-pooled | `txn` | PgDog (v0.1.60, unmodified, AGPL-3.0) of the test overlay `pgdog-txn.yaml` on `pgdog-txn:6432`, in front of `compute1`. Not in the release compose: no PgDog fronts the computes yet (PG2c) | test overlay only |

Postgres majors: `compute1` runs 17 by default and 16 with `COMPUTE_IMAGE` set
to the pinned `compute-v16` digest and `COMPUTE_PG_MAJOR=16`, on fresh volumes
(R0.6). `pg-control` offers only 17 ([#1130](https://github.com/ostrium-labs/loams/issues/1130)).

## Per product

| Product | Mode | Why |
|---|---|---|
| **Temporal** (server, compat mode) | direct | Its SQL persistence holds multi-statement transactions with row locks (`FOR UPDATE`, `FOR SHARE` in the `postgres12` plugin's `execution.go` and `shard.go`). Whether its plugins survive a transaction pooler is not known yet: `postgres12_pgx` in simple-protocol mode is the configuration Temporal itself tests for PgBouncer in transaction pooling (`TestPGXSP`, R0.5, temporalio/temporal#9804). DX0 Task 3 runs `TestPGXSP` through `txn` to measure it, as an expected failure until it passes |
| **DBOS Transact** (Python, TypeScript, Go, Java) | direct | `LISTEN`/`NOTIFY` (the default notification path) needs a session that outlives transactions. Python, TypeScript and Java can poll instead (`use_listen_notify` / `useListenNotify`, fixed when the system database is created); Go cannot (R0.11). DX0 Task 5's `dbos_<l>_txn_mode_xfail` records the transaction-mode result |
| **Airflow** (schedulers, triggerer, API server, DAG processor) | direct | The scheduler's HA uses `SELECT … FOR UPDATE SKIP LOCKED` inside transactions, and `airflow db migrate` takes a session advisory lock. DX0 Task 6's `airflow_txn_mode_leg` moves only the schedulers and the triggerer to `txn` (DX1 ruling T6.R7); until it has passed in the nightly, everything stays direct. **Not yet run** (T6.R9). The edge workers never reach the database: they talk to the API server only |

## Databases and roles

`deploy/release/compute/durable-init.sh` (the one-shot `compute-init` and
`temporal-schema` services of `compose.compute.yaml`) creates, idempotently:

| Database | Owner | Notes |
|---|---|---|
| `temporal`, `temporal_visibility` | `temporal` (`CREATEDB`) | schema by `temporal-sql-tool` (plugin `postgres12`) to the pinned image's newest version; `CREATEDB` because Temporal's persistence suite creates and drops `test_*` databases |
| `airflow` | `airflow` | migrated by `airflow db migrate` (DX0 Task 6) |
| `dbos_py`, `dbos_ts`, `dbos_go`, `dbos_java` | `postgres` (`CREATEDB`) | one system database per app (D1520); the SDKs' own suites log in as `postgres` on `localhost:5432` and create their own databases (R0.12) |

Each product has its own database, never a schema in a shared one (D1529), and
`PUBLIC` may not connect to any of them. Never Loams SQL.
`conformance/compute/durable/run.sh` checks all of this.
