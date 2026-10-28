# DX0 exit report (draft)

Status: **draft, DX0 has not exited.** Written on 2026-10-27 with DX0 Task 7.
Updated on 2026-10-28 with the first compute nightly (run 40618803289, DX1
ruling T7.R7).
This report is updated from that nightly's [matrix](../guides/compute/compatibility.md)
before the owner reviews it. Plan: [DX1](2026-10-21-dx1-durable-execution-compat.md)
(DX0, Tasks 0–7). Design: [§74](../design/74-durable-execution-compat.md).

## Exit criteria

| Criterion (plan, "DX0 exit") | State |
|---|---|
| Every product has a nightly run with a committed `xfail.toml` | **not met yet.** The first nightly ran every leg on 2026-10-28. Durable, the corpus and Temporal on 16 were green. The DBOS and Airflow legs stopped on harness defects, which are now fixed (T5.R5, T6.R10). The Postgres 17 leg hit #1185 (fix #1475). The second run decides |
| The corpus has at least the DX1 and DX2 scenarios in both languages and replays clean | **met against start-dev.** 31 scenarios (DX1 9, DX2 15, DX3 3, DX4 4) and 68 histories (36 Go, 32 TypeScript) replayed clean locally (#1207). They were generated against `temporal server start-dev`, not compat mode (T4.R1) |
| `SPIKE.md` and this report give the go or no-go | compat mode is **not yet go** (below) |
| The owner reviews the report before DX1 starts | superseded: the owner started DX1 in parallel on 2026-10-24 (D1590) |

## Results by task

| Task | PR | Result |
|---|---|---|
| 0, rulings | #1131 | pins, endpoint modes, Temporal limits and the DBOS switches recorded (R0.1–R0.15) |
| 1, harness | #1135 | `conformance/compute/` and `compose.compute.yaml`. The self-test runs per pull request |
| 2, databases and endpoint modes | #1168 | one database and role per product. Only **direct** endpoints exist. There is no session-pooled endpoint (#1129), and `pg-control` offers only Postgres 17 (#1130) |
| 3, Temporal compat mode | #1187 | Loams Postgres 16, direct: Temporal's persistence suite passed 1,236 cases (run serially). The soak was green once and red once, because a stopped compute did not start again (#1185). The transaction-mode smoke passed through PgDog. The 17 leg has not run |
| 4, the corpus | #1207 | above |
| 5, DBOS | #1392 | the per-SDK harness and the crash apps are written but not run (T5.R4) |
| 6, Airflow with the Edge executor | #1294 | OC0's seven cases plus five Edge cases are written but **not yet run end to end** (T6.R9) |
| 7, matrix, docs, nightly, this report | this PR | `scripts/compute/matrix.py`, `docs/guides/durable/`, `docs/guides/compute/`, `compute-compat.yml` |

## Go or no-go

- **Compat mode (the Temporal server on Loams Postgres 16, direct): not yet go, one step left.**
  - The 30-minute soak at 20 starts per second passed in the first nightly, with a history restart and a compute stop and start: 35,999 workflows, 0 failed.
  - #1185 is explained: a stopped container kept its stale Postgres socket lock. #1475 fixes it.
  - What remains is three nightly runs in a row that pass the compute stop and start, after #1475 merges.
- **DX1, the Temporal gateway: go, and already running** (D1590). Its oracle is the start-dev corpus. All 18 DX1 histories replayed through the gateway on the fork's Resonate server (X1.R21). When compat mode is go, the corpus is regenerated against it (T4.R1).
- **DX1A, `LoamsResonateExecutor`: not yet.** The Edge executor baseline (Task 6) must first run green in the nightly, because DX1A is judged against it.

## Facts DX1 needs

- **Temporal 1.32.1's limits** (R0.9):
  - blob 2 MiB, with a warning at 512 KiB;
  - memo 2 MiB;
  - history 50 MiB or 51,200 events, with a warning at 10 MiB or 10,240 events;
  - 2,000 pending activities, children, signals or cancels each;
  - mutable state 8 MiB;
  - search attributes: 100 keys, 2 KiB per value, 40 KiB in total;
  - gRPC requests up to 4 MiB.
- **Default retention.** The server sets none. The minimum is 1 h for a local namespace and 24 h for a global one. The CLI's `operator namespace create` defaults to 72 h.
- **Endpoint modes** (`conformance/compute/endpoint-modes.md`): only direct exists. Temporal's four services and a smoke worked through PgDog transaction pooling, but that is not yet a recommendation.
- **DBOS polling switches** (R0.11): Python `use_listen_notify`, TypeScript and Java `useListenNotify`. Each is fixed when the system database is created. Go has none.
- **Shards:** 16 history shards, fixed at creation.

## Recommendations

1. Merge #1475 (#1185's fix) and the nightly fixes, then run the nightly again by pushing to a `compute-compat/` branch (T7.R6). Record any remaining failures in `xfail.toml`.
2. Run three more nightlies with green compute resumes on 16. Compat mode then becomes go and is offered as a documented option (owner ruling below).
3. Record DBOS's first results (Task 5): its `txn_mode` entries may turn into unexpected passes, as Temporal's did (T3.R6).
4. Before DX1A, run the Airflow Edge baseline green twice. If the start-up `edge3` install is slow or flaky there, switch to a built image (T6.R2).

## Owner rulings (2026-10-27, defaults approved, relayed by the coordinator)

- **The matrix page is refreshed by hand**, as OC0 specifies: a pull request
  from the nightly's `compatibility-matrix` artifact. No bot pull requests.
- **Compat mode becomes a documented option once it is go.** Design 74 makes the
  upstream Temporal server on Loams Postgres both the oracle and the compat
  mode (D1509). `docs/guides/durable/temporal-compat-mode.md` drops its
  "not yet go" label then.
