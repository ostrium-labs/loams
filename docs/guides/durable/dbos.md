# DBOS Transact on Loams Postgres

Status: **preview, the conformance harness is written but not yet run** (DX0
Task 5, #1392; DX1 ruling T5.R4). Nothing on this page has been checked
against Loams yet: the compute nightly's `dbos` leg runs all four SDKs.

DBOS Transact (MIT) keeps its workflow state in a Postgres system database. On
Loams that database is on Loams Postgres. DBOS needs no extra server, and no
DBOS Conductor or licence key is used.

Endpoint mode: **direct**

## How to set it up

- **The endpoint.** Use the direct endpoint (`compute1:55433`, or a
  `pg-control` endpoint's compute). DBOS's default notification path is
  `LISTEN`/`NOTIFY`, which needs a session that outlives transactions. There
  is no session-pooled endpoint yet
  ([#1129](https://github.com/ostrium-labs/loams/issues/1129)).
- **One database per app** (D1520). The test stack's `compute-init` creates
  `dbos_py`, `dbos_ts`, `dbos_go` and `dbos_java`, one system database for each.
- **Polling instead of `LISTEN`/`NOTIFY`.** This matters for scale-to-zero and
  for a transaction pooler. Python has `use_listen_notify = false`, and
  TypeScript and Java have `useListenNotify: false`. Each is fixed when the
  system database is created. Go has no switch (DX1 ruling R0.11).
- **Scale to zero.** A suspended compute drops the listener's connection. The
  plan's `dbos_<l>_notify_after_compute_resume` case checks that a new
  notification arrives after the compute resumes. It has not run yet.

## Expected failures

These are what the plan expects. They are confirmed only once the suites run.

- Through PgDog transaction pooling, `LISTEN` across transactions fails
  (`dbos_<l>_txn_mode_xfail`).
- Tests that need more than Postgres (Python's Kafka tests, the CockroachDB
  cases) skip themselves when that service is absent, so they are neither
  passes nor failures (DX1 ruling T5.R3).
