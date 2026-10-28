# The Temporal server on Loams Postgres (compat mode)

Status: **preview, not yet go** (design 74 D1509; DX0 Task 3,
[SPIKE.md](../../../conformance/compute/temporal/SPIKE.md)).

This page runs the Temporal server itself, unmodified, on Loams Postgres. It is
not a Loams implementation of Temporal. The Temporal gateway on Resonate (DX1)
is a separate piece of work and is not described here.

Endpoint mode: **direct**

## What runs

- The stock `temporalio/server` 1.32.1 image (by digest) as four services:
  `temporal-frontend`, `temporal-history`, `temporal-matching` and
  `temporal-worker`. The profile is `temporal` in
  `deploy/release/compose.compute.yaml`.
- Plugin `postgres12`. The schema is set up by the pinned admin-tools'
  `temporal-sql-tool` (the one-shot `temporal-schema`), in two databases of
  their own, `temporal` and `temporal_visibility`, owned by the role `temporal`.
- **16 history shards, fixed when the database is created.** The shard count
  can never change afterwards. A different count needs a new database.
- **The direct endpoint** (`compute1:55433`). Temporal's SQL persistence holds
  multi-statement transactions with `FOR UPDATE` and `FOR SHARE`. A smoke ran
  through PgDog in transaction pooling, but transaction mode is not
  recommended until a soak passes there too.
- **Never Loams SQL.** Temporal's MySQL plugin is never used on Loams.

## Start it

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

With the profile as its argument, from `deploy/release`: `bash ../../docs/guides/durable/snippets/compute-profile-up.sh temporal`. Then, from the repository root, check the frontend from the compose network:

<!-- snippet: snippets/temporal-health.sh -->
```sh
#!/usr/bin/env bash
# Ask the compat-mode frontend for its health from the compose network
# (no host port is published). Run from the repository root.
set -euo pipefail
. conformance/compute/lib.sh
oc_temporal_wait temporal-frontend:7233 300
oc_run "$TEMPORAL_ADMIN_TOOLS_IMAGE" temporal operator cluster health --address temporal-frontend:7233
```

## Results so far

Recorded on 2026-10-24 on Loams Postgres 16. The details are in SPIKE.md.

- Temporal's own SQL persistence suite passed: 1,236 cases across `postgres12`, `postgres12_pgx` and the simple-protocol variant.
- The soak passed once.
- The soak failed once, because a stopped compute did not start again ([#1185](https://github.com/ostrium-labs/loams/issues/1185)).
- The Postgres 17 leg has not run. Temporal 1.32 lists Postgres 13–16.

Compat mode becomes **go** once two things have happened:
- #1185 is explained or fixed, and the soak then passes three nightly runs in a row;
- the 30-minute soak at 20 starts per second passes once.

The [matrix](../compute/compatibility.md) has the latest nightly.

## Limits worth knowing

These are Temporal 1.32.1's defaults, from its `dynamicconfig` (DX1 ruling R0.9):

| Limit | Default |
|---|---|
| Blob size | 2 MiB error, 512 KiB warning |
| History size | 50 MiB error, 10 MiB warning |
| History event count | 51,200 error, 10,240 warning |
| gRPC request size | 4 MiB |
