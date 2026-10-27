# DBOS Transact on Loams Postgres

DX1 DX0 Task 5 (design 74 §8). Status: **written, not yet run** (DX1 ruling
T5.R4); the nightly's `dbos` leg runs it. Each SDK (MIT) runs unmodified against Loams
Postgres's direct endpoint (`compute1`), judged by its own suite and by the
Loams cases, which check the effects (rows each step wrote, event timestamps).

| Directory | Suite | Polling switch |
|---|---|---|
| `dbos-py/` | `pdm run pytest tests` (3.2.0) | `use_listen_notify` |
| `dbos-ts/` | `jest ./tests` with `jest-junit` (v5.2, npm 5.2.11) | `useListenNotify` |
| `dbos-go/` | `gotestsum -- ./dbos/...` after `mockery` (v1.6.0) | none |
| `dbos-java/` | `./gradlew clean build` on Temurin 21 (1.2.0) | `withUseListenNotify` |

The suites hard-code `localhost:5432` and the role `postgres`: the `dbos`
profile's runners share `dbos-pg`'s network namespace, where socat forwards
that port to Loams Postgres (or to PgDog for the transaction-mode leg).
`durable-init.sh` gives `postgres` `CREATEDB` and the per-app system
databases `dbos_py`, `dbos_ts`, `dbos_go`, `dbos_java`.

## The Loams cases (`cases.sh`, the apps under `crash/<lang>/`)

| Case | Effect checked |
|---|---|
| `dbos_<l>_crash_recovery_exactly_once` | a five-step workflow; the process kills itself when step 4 starts (steps 1-3 checkpointed); a restart resumes it; each step's row exists exactly once |
| `dbos_<l>_queue_concurrency` | 1,000 workflows on a queue with concurrency 10, dequeued by two processes; each ran exactly once |
| `dbos_<l>_notify_latency` | an event set by one process reaches a waiting process in under 1 s |
| `dbos_<l>_notify_after_compute_resume` | the compute stops and starts while a process waits; the event still arrives (within 30 s of being set) |
| `dbos_<l>_polling_mode` | Python, TypeScript, Java: the crash case on a fresh system database created with LISTEN/NOTIFY off |
| `dbos_<l>_txn_mode_xfail` | the crash case through PgDog transaction pooling; expected to fail |

The Java app stops itself with `Runtime.halt` (no shutdown hooks, like a
`kill -9`); the others send themselves `SIGKILL`. No DBOS Conductor, image or
key is used (D1522).
