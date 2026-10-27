# Durable execution on Loams

Status: **preview**. Design [§74](../../design/74-durable-execution-compat.md);
plan [DX1](../../plans/2026-10-21-dx1-durable-execution-compat.md) (DX0, the
baselines). Every product below runs **unmodified**, exactly as released, with
its state on **Loams Postgres** and never on Loams SQL. Its results are in the
[compatibility matrix](../compute/compatibility.md).

## Which engine to use

| Use | When |
|---|---|
| **Airflow** | Data pipelines, backfills, lineage, asset-driven schedules and human-in-the-loop on data jobs; teams that already have Dags |
| **Temporal** | Long-running business workflows; teams with existing Temporal code or skills; strict event history and replay semantics. Today that means the Temporal server itself on Loams Postgres (compat mode). The Temporal gateway on Resonate (DX1) is in development |
| **DBOS** | Durable functions inside an application, whose steps are database transactions in the same Postgres; no extra server |
| **Resonate (native)** | Lightweight durable promises and functions, agent runtimes, and cross-language RPC with durable callbacks; Loams' own long operations |

(Design 74 §10, D1530.)

## Where each one stands

| Product | Page | Status |
|---|---|---|
| Temporal server on Loams Postgres (compat mode) | [temporal-compat-mode.md](temporal-compat-mode.md) | preview, **not yet go**: Temporal's persistence suite is green on Loams Postgres 16, but one of two soaks was red ([#1185](https://github.com/ostrium-labs/loams/issues/1185)) |
| DBOS Transact | [dbos.md](dbos.md) | preview, the conformance harness is written but **not yet run** |
| Airflow 3.3 with the Edge executor | [airflow.md](airflow.md) | preview, written but **not yet run end to end** |

## Common ground

- **One database per product, never a shared schema** (D1529), created by
  the one-shot `compute-init` of `deploy/release/compose.compute.yaml`.
- **The direct endpoint.** The release compose offers only direct endpoints
  (`compute1:55433`, or a `pg-control` endpoint's compute). Temporal, DBOS and
  Airflow's scheduler need a session (multi-statement transactions with row
  locks, `LISTEN`/`NOTIFY`, advisory locks), and there is no session-pooled
  endpoint yet ([#1129](https://github.com/ostrium-labs/loams/issues/1129)).
  See [endpoint-modes.md](../../../conformance/compute/endpoint-modes.md).
- **Never public.** No compute profile publishes a host port; the products
  reach Loams on the compose network only.
- **Pinned.** Every image by digest and every package by exact version, each at
  least 14 days old, in `conformance/compute/pins.env`.

## Starting a profile

From `deploy/release`:

<!-- snippet: snippets/compute-profile-up.sh -->
```sh
#!/usr/bin/env bash
# Start one compute profile (temporal or airflow) on the release stack's
# Loams Postgres. Run from deploy/release. The airflow profile also needs the
# rustfs profile for its bucket.
set -euo pipefail
profile=${1:?usage: compute-profile-up.sh <temporal|airflow>}
extra=()
[ "$profile" = airflow ] && extra=(--profile rustfs)
docker compose --env-file .env --env-file ../../conformance/compute/pins.env \
  -f compose.yaml -f compose.compute.yaml --profile "$profile" "${extra[@]}" up -d
```
