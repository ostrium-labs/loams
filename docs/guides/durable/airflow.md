# Airflow 3.3 with the Edge executor on Loams Postgres

Status: **preview, written but not yet run end to end** (DX0 Task 6, DX1
rulings T6.R1–T6.R9). This is Airflow itself on Loams, not an Airflow
implementation. The general Airflow recipe is
[../compute/airflow.md](../compute/airflow.md). This page adds the Edge
executor.

Endpoint mode: **direct**

## The Edge executor

- The executor is `airflow.providers.edge3.executors.EdgeExecutor` from the
  `apache-airflow-providers-edge3` provider, pinned at 4.3.2. That is the
  version the Airflow 3.3.2 constraints file pins. The stock image does not
  include the provider, so each container installs it at start
  (`_PIP_ADDITIONAL_REQUIREMENTS` with `PIP_CONSTRAINT`).
- The Edge API runs on the API server (`[edge] api_enabled`). It serves
  `/edge_worker/v1/rpcapi` for the workers and `/edge_worker/ui` for the UI.
- Two edge workers, `airflow-edge-worker-1` and `-2`, run on the queue
  `default`. **They reach only the API server**, never the database: they skip
  the image's database check at start.
- An edge worker in maintenance mode takes no new tasks and finishes the one it
  is running. Put a worker into maintenance and take it out through the UI,
  `airflow edge maintenance on|off` on the worker, or the Edge API.

On the conformance stack, from the repository root:

<!-- snippet: snippets/airflow-edge-workers.sh -->
```sh
#!/usr/bin/env bash
# List the Edge workers through the API server's Edge API, from the compose
# network, with the conformance harness's client. Run from the repository root.
set -euo pipefail
. conformance/compute/lib.sh
. conformance/compute/airflow/common.sh
af workers
```

## What the conformance run checks

`conformance/compute/airflow/run.sh` runs OC0's seven cases. Then
`edge/run.sh` adds the Edge cases. Each case checks effects, not exit codes:
every task execution writes one object to the bucket.

- `airflow_edge_workers_register`: both workers are online.
- `airflow_edge_ha_no_double_run`: 500 mapped tasks on two schedulers and two
  workers, with one try and one effect each.
- `airflow_edge_worker_kill_retries`: after a `kill -9`, try 2 runs on the
  other worker within the heartbeat timeout.
- `airflow_edge_maintenance_mode`: a worker in maintenance takes no new tasks
  and finishes the one it is running.
- `airflow_txn_mode_leg`: the schedulers and the triggerer run through PgDog
  transaction pooling. Until this passes in the nightly, use the direct
  endpoint.
