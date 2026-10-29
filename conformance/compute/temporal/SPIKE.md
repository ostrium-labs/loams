# Compat mode spike: the Temporal server on Loams Postgres

DX1 DX0 Task 3 (design 74 D1509; OC1 OC0 Task 7). Status: **the Loams Postgres 16 leg ran
twice locally; the Postgres 17 leg, the full 30-minute soak and a soak
through PgDog have not run** (they belong to the nightly that DX0 Task 7
adds).

## Setup

- Temporal server 1.32.1 (`temporalio/server` by digest), four services,
  plugin `postgres12`, 16 history shards; schema by the pinned admin-tools'
  `temporal-sql-tool` (both databases at the image's newest version).
- Loams Postgres 16.15 (`compute1`, the pinned `compute-v16` image), through
  the **direct** endpoint; the stack's pageserver and safekeeper on local
  folders. Never Loams SQL.
- Machine: one shared 14-core, 15 GB workstation with about 20 other jobs
  running (load average 8 to 10), rootless Podman, compose project `dx0` with
  no host ports. Not a GitHub runner: the nightly is the reference.

## Results (2026-10-24, Loams Postgres 16)

| Case | Run 1 (persistence suites in parallel) | Run 2 (`-parallel 1`) |
|---|---|---|
| `temporal_frontend_serving` | pass | pass |
| `temporal_schema_setup` | pass | pass |
| Persistence suite, `TestPQ` (`postgres12`) | 410 pass, 2 fail, 2 skip | 412 pass, 0 fail, 2 skip |
| Persistence suite, `TestPGX` (`postgres12_pgx`) | 411 pass, 1 fail, 2 skip | 412 pass, 0 fail, 2 skip |
| Persistence suite, `TestPGXSP` (simple protocol) | 408 pass, 4 fail, 2 skip | 412 pass, 0 fail, 2 skipSP |
| `temporal_smoke_hello` (20 workflows) | pass | pass |
| `temporal_soak_no_failed_workflows` (10 min, 20/s) | pass: 11,999 started, 11,999 completed, 0 failed, 0 lost | **fail**: 7,759 started, 4,240 starts given up, after the compute did not come back (#1185) |
| `temporal_history_restart_recovers` | pass | pass on its own (the frontend recovered); counted as fail because the soak lost workflows |
| `temporal_compute_resume_recovers` (compute stopped 30 s) | pass | **fail**: `compute1` exited 1 when started again; Postgres never answered (#1185) |
| `temporal_txn_mode_xfail` (10 workflows through PgDog) | **unexpected pass** | fail, inconclusive: Postgres was down (#1185) |

Run 1's persistence failures were three tests, each a deadline or a timing
assumption:
- `TestClusterMembershipUpsertCanPageRead` (all three plugins): 70 of 100
  membership rows read back where the test expects 100 (rows carry a short
  expiry, and slow writes let some expire before the read);
- `TestScanAllTrees` (`TestPQ`, `TestPGXSP`): the test's 30 s context
  deadline;
- `TestInsertSelect_Single` and `_Multiple` (`TestPGXSP`): the test's 2 s
  deadline.

Run 2 ran the three plugin variants one after another (`-parallel 1`): all
1,236 cases passed. Run 1's failures came from running them together on a
loaded machine, not from Loams Postgres (T3.R5).

Run 2's soak failed for one reason: when the soak started `compute1` again
after its 30-second stop, the container exited with code 1 within 40 s and
Postgres never answered; its output was not captured. Run 1 survived the same
stop and start, and a manual start of the same container later worked. Filed
as #1185 (transient compute restart failure, cause unknown).

Soak details (run 1): 9,908 start retries, all during the history restart and
the compute outage; the longest start took 94.6 s and the longest
start-to-close 475 s, both across the outages; 6 activities ran twice
(at-least-once delivery after the history restart, as Temporal documents);
achieved rate 19.5 starts per second.

## The first nightly (2026-10-28, run 40618803289, GitHub-hosted runner)

These results are from the Loams Postgres 16 leg.

- **Every case passed (1,579 cases).**
- **The full soak passed.** It ran for 30 minutes at 20 starts per second, with a history restart and a 30-second compute stop:
  - 35,999 workflows started and 35,999 completed;
  - 0 failed;
  - 0 activities ran twice;
  - the longest start took 44 s, across the outage.
- **The transaction-mode smoke passed again.**

On the Postgres 17 leg, the persistence suite passed (1,575 cases). Every case after the compute stop failed, because the compute did not start again.

**#1185's cause** is in `compute1`'s log:

```
FATAL:  lock file "/tmp/.s.PGSQL.55433.lock" already exists
HINT:  Is another postmaster (PID 26) using socket file "/tmp/.s.PGSQL.55433"?
```

A container that is stopped and started again keeps its `/tmp`. When the new Postgres gets the PID that the stale lock records, it refuses to start. That explains why the failure came and went. #1475 removes the stale lock at start.

## Transaction-mode finding

The four services ran through PgDog v0.1.60 in transaction pooling, and the
10-workflow smoke completed. That is an unexpected pass of
`temporal_txn_mode_xfail`, so the entry is removed and the case now must
pass. It is **not** a recommendation yet: a smoke is not the persistence
suite or a soak. Before the recipe may offer transaction mode, the soak must
pass through PgDog too (left for the nightly; the persistence suite cannot
run there, T3.R3).

## Recommendation for compat mode (D1509)

**Not yet go.** Temporal's own SQL persistence suite passes on Loams
Postgres 16 through the direct endpoint (all three plugins, run 2), and the
stock server survived a history-service restart and a compute stop/start
with no lost or failed workflow (run 1). But the plan's gate also needs the
soak green, and run 2's soak failed because a stopped compute did not start
again (#1185). Compat mode becomes **go** for Loams Postgres 16 through the
direct endpoint when:

1. #1185 is explained or fixed, and the soak passes with the compute
   stop/start, three nightly runs in a row;
2. the full 30-minute soak at 20 starts per second passes once.

Until then the matrix shows "Temporal on Loams Postgres 16: persistence
suite green, soak red (#1185)". The 17 leg stays an expected-failure leg until
it has run. Transaction mode is not recommended (see above).
