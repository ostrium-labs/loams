# The Temporal gateway's store

Status: **preview.** The Postgres store and its contract tests run in CI on
stock Postgres 17. The retention sweeper, the page tokens and the run listing
are new in schema version 2. They have not yet run end to end against Loams
Postgres `compute1` or under load.

The Loams Temporal gateway (design 74, plan DX1) keeps every workflow run's
history, its mutable state, the transactional outbox it hands to Resonate,
and the visibility rows in **its own Loams Postgres database**. The code is in
`crates/loams-temporal-store`. This guide covers how to configure that
database, what is stored in it, and how it is cleaned up.

## Where the database must live

- **Use a Loams Postgres project of its own,** for example a database named
  `loams_temporal`, and never a tenant's database (design 74 §5.5).
- **Use a session-pooled or direct endpoint, never PgDog in transaction
  mode** (§5.5 rule 5). A commit holds a row lock across several statements,
  and the outbox wake-up is a `LISTEN` that lasts as long as the store does.
  In the 0.1.x compose this is the direct `compute1` endpoint.
- **Never use Loams SQL (TiDB) or MySQL** (§5.5 rule 6). TiDB ignores
  `LOCK IN SHARE MODE`, and the compare-and-set commit depends on real row
  locks.

## Configuration

These settings go in the gateway's TOML file, either for `loams-temporal
--config` or for `loams-gateway --module temporal`. See
`deploy/release/temporal-gateway.toml.example`.

```toml
shards = 64                       # fixed when the database is created

[store]
kind = "postgres"                 # or "memory" (insecure_dev only)
url_env = "LOAMS_TEMPORAL_DATABASE_URL"

[store.pool]                      # optional; these are the defaults
max_connections = 16              # at least 2: one is the outbox LISTEN
min_connections = 0
acquire_timeout_secs = 10
idle_timeout_secs = 600           # 0: never close idle connections
max_lifetime_secs = 1800          # 0: never recycle
statement_timeout_ms = 30000      # 0: off; migrations always lift it
idle_in_transaction_timeout_ms = 60000
application_name = "loams-temporal"

[retention]                       # optional; these are the defaults
enabled = true
interval_secs = 60
max_runs_per_call = 100
max_outbox_rows_per_call = 1000
dispatched_outbox_ttl_hours = 24
```

The URL comes from the environment, never from the file. For example:
`LOAMS_TEMPORAL_DATABASE_URL=postgres://temporal:…@compute1:55433/loams_temporal`.

**Sizing the pool.** Each gateway replica opens at most `max_connections`
connections. Add up every replica's maximum, then leave room for
`pg-control`, migrations and your own psql sessions under the compute's
`max_connections`.

## Startup and migrations

At startup the gateway connects, checks the shard count, and runs
`PgStore::migrate`. Migrations run in a single transaction under a Postgres
advisory lock, so several replicas can start at the same moment safely. The
applied versions are recorded in `schema_version`.

| Version | What it adds |
|---|---|
| 1 | `namespaces`, `executions` (the compare-and-set row), `current_runs`, `history_events`, `outbox`, `task_queues`, `visibility` |
| 2 | `executions.closed_at`, the outbox and listing indexes, and `history_events` as 16 hash partitions on `shard_id`. Existing rows are copied over (D2062) |

**The shard count is permanent.** It is set when the database is created
(default 64, X1.R6), and it fixes which shard each workflow's rows land in.
A gateway configured with a different count refuses to start
(`ShardCountMismatch`).

## How a state change is written

Every state change is a single transaction, `Store::commit`:

1. Lock the run's `executions` row with `FOR UPDATE`.
2. Check `db_record_version`, which is the compare-and-set version, and
   check that the new events continue `next_event_id`.
3. Insert the history batch.
4. Update the mutable state, and set `closed_at` if the run has closed.
5. Insert the outbox rows, upsert the visibility row, and send `NOTIFY`
   on `loams_temporal_outbox`.

If the version check fails, the commit returns `Conflict` and writes
nothing.

The relay claims outbox rows with a lease, using `FOR UPDATE SKIP LOCKED`,
and hands them to Resonate. It then marks them dispatched. Delivery is at
least once: if a lease expires, the row can be claimed again. Resonate's
promise IDs are deterministic, so a repeated create is harmless.

## Paging

`Store::history_page` and `Store::list_runs` return opaque page tokens
(D2061):

- **A history token is bound to its run.** If you present it for another
  run, the call fails with `InvalidArgument`. If events were written after
  a page was served, the next call with that page's token returns them.
- **A run-listing token is bound to its namespace and filter.** The listing
  is sorted newest start first, then by workflow ID and run ID. The token
  stores the last row's position (a keyset), so runs that start while you
  page never shift a page or repeat a row.
- **Pages hold 1 to 10,000 rows.**

## Retention

A closed run is deleted once **its close time plus its namespace's
retention** has passed (D2063). The default retention is 72 h, the same as
the Temporal CLI's default. Deleting a run removes its history, its
visibility row, its outbox rows, and its `current_runs` row. The
`current_runs` row is only removed if that run is still the workflow's
current run, so a newer run of the same workflow ID is never touched.

These runs are **kept** until a later round:

- runs that still have an **undispatched outbox row**, because Resonate is
  still owed something;
- runs that are being committed or purged by another replica at that moment
  (`SKIP LOCKED`).

Dispatched outbox rows are deleted after `dispatched_outbox_ttl_hours`.

Every replica runs the sweeper; replicas never block each other. Each round
calls the purge until a call deletes less than its limits allow, then sleeps
for `interval_secs`. If you set `enabled = false`, the tables grow without
bound.

The Resonate promises of a deleted run are **not** removed yet. That waits
for the fork's FW5 retention. Until then, a late delivery for a run that no
longer exists is acknowledged and dropped (X1.R16).

## Several relays

`owned_shards(replica, replicas, shards)` splits the shards so that every
shard belongs to exactly one replica (D2064). The DX1 gateway runs a single
replica that owns every shard. The relay starts using this split when
multi-replica support lands in DX3.

## Running the conformance suite

Every contract test runs against every backend: `MemStore` always, and
`PgStore` when a server is named. Each Postgres test creates its own
database and drops it afterwards, so the user in the URL needs `CREATEDB`.

```sh
podman run -d --name lts-pg -e POSTGRES_PASSWORD=pg -p 127.0.0.1:5432:5432 \
  docker.io/library/postgres:17.10
LOAMS_TEMPORAL_TEST_PG_URL=postgres://postgres:pg@127.0.0.1:5432/postgres \
  cargo test -p loams-temporal-store
```

`LOAMS_TEMPORAL_PG_URL` is accepted as an alias. If neither variable is set,
the Postgres leg is skipped and each test prints a line saying so.

In CI, the `Temporal store: Postgres 17` job (`.github/workflows/ci.yml`)
runs the same command against a `postgres:17` service container. It is a
full-tier job: it runs when `crates/loams-temporal-store/**` changes on a
pull request labelled `ci:full` or into `main`, in the nightly on `dev`, and
on a manual dispatch. On an ordinary pull request into `dev`, only the
MemStore leg runs (in `Rust: tests`).

To add a backend, add a `Backend` variant in `tests/store.rs`. Every test in
the file then runs against it too.
