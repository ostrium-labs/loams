# Compat mode: the Temporal server on Loams Postgres

DX1 DX0 Task 3 (design 74 D1509; lands OC1's OC0 Task 7). The stock Temporal
server 1.32.1 (MIT), unmodified, as four services (`frontend`, `history`,
`matching`, `worker`; plugin `postgres12`; 16 history shards fixed at creation)
on Loams Postgres through the **direct** endpoint (`compute1`). Never on Loams
SQL. This runs the Temporal server on Loams Postgres; it is not a Loams
implementation of the Temporal API (that is DX1's gateway).

```sh
COMPUTE_PG_MAJOR=16 conformance/compute/temporal/run.sh   # primary leg
COMPUTE_PG_MAJOR=17 conformance/compute/temporal/run.sh   # expected-failure leg (Temporal lists 13-16)
```

Each leg needs fresh volumes: the major is fixed when `compute1`'s timeline is
created. Results: [SPIKE.md](SPIKE.md).

## What runs

| Case | What it checks |
|---|---|
| `temporal_frontend_serving` | the frontend answers `GetSystemInfo` (`temporal operator cluster health`) |
| `temporal_schema_setup` | `temporal` and `temporal_visibility` are at the newest schema version the pinned admin-tools image carries |
| Temporal's persistence suite (`persistence.sh`) | `TestPQ` (`postgres12`), `TestPGX` (`postgres12_pgx`), `TestPGXSP` (`postgres12_pgx`, simple protocol) from `common/persistence/tests` at the pinned commit, one JUnit case per test and subtest |
| `temporal_smoke_hello` | 20 workflows of one activity each complete (`soak/`) |
| `temporal_soak_no_failed_workflows` | `soak/` at `SOAK_RATE` (20) starts per second for `SOAK_DURATION` (30m): every started workflow completed with its result, none failed, none lost |
| `temporal_history_restart_recovers` | the history service restarted halfway; the frontend recovered and the soak lost nothing |
| `temporal_compute_resume_recovers` | Loams Postgres's compute stopped for 30 s at two thirds and started again; Postgres answered and the soak lost nothing |
| `temporal_txn_mode_xfail` | the four services through PgDog transaction pooling, and a smoke. Planned as an expected failure; it passed on 2026-10-24 (SPIKE.md), so it is no longer listed and must pass |

`soak/` is a small Go program on the pinned SDK (one workflow, one activity, a
worker in the same process). It starts each workflow with a fixed ID and
retries starts through restarts, so a retry never starts a second workflow,
and it checks effects rather than an exit code.

Expected failures: `xfail.toml` (16) and `pg17/xfail.toml` (17).
