# FC0 exit report — Loams Stream (design 82), 2026-10-29

Track FC, plan [FC1](2026-10-29-fc1-flink-compat-engine.md). The work is in the hard fork `ostrium-labs/streamfusion` (PRs #1–#15). Everything is **preview**.

## The gate (design 82 §16.1)

| Criterion | Result |
|---|---|
| The corpus runs on the oracle with 100 % of its own expectations; failures are listed in `oracle-xfail.toml` | **Met by listing.** The corpus has 373 cases: smoke 20, Nexmark 23 and Flink test programs 330.<br>- Recorded on Apache Flink 2.2.1: 20 + 22 + 205.<br>- 41 Flink test programs ran, but differ from what Flink's own test program expects (mostly TTL timing, and processing-time behaviour that filesystem sources cannot reproduce).<br>- 85 cases are listed as oracle xfails with reasons: Nexmark q6, process table functions, processing-time temporal joins, test-connector metadata columns, jobs that end before the savepoint, and other planner refusals.<br>- Every case that cannot be recorded cleanly is listed (fork #14). |
| The parser accepts every statement in the corpus | **Met.** All 1,379 statements of the 373 cases parse, after fork #15 (UNNEST, process table function calls). 1,287 bind, 91 are refused by name, and 1 fails: Nexmark q6, which Flink rejects too.<br>Flink's own `.q` files: 790 of 806 statements parse; 9 are rejected where Flink rejects them, and 7 are named refusals. |
| Nexmark q0 runs end to end on the standalone engine | **Met at the operator level.** q0 and a q5-like tumbling count run in the Rust `Driver` from hand-written operator specs, with the engine built without JNI (fork #12). Running SQL text end to end needs the planner (Task 10) and the runtime (Task 11). |

## What exists

| Task | PR(s) | State |
|---|---|---|
| 0 Rulings | loams #1524 | merged |
| 1 Pins, `jni` feature, skeletons | fork #1, #4 (CI trim) | merged |
| 2 OpSpec and Driver | fork #5 (WIP), #10, #12 | merged; 9 driver tests and the engine's 980 tests pass |
| 3 Parser | fork #3, #15 | #3 merged; #15 open |
| 4 Types and binder | fork #7, #8, #9 | merged |
| 5 Differential harness | fork #2 | merged |
| 6 Corpus | fork #6, #11, #14 | #14 open; executable `.q` cases moved to Task 22 |
| 7 Profile and compatibility page | fork #13 | merged |
| 8 slatedb spike | branch `fc1-t8-slatedb-spike` | report in `fc1-rulings/t8.md`; RocksDB stays FC1's backend |

## The FC1 bar, re-set (Task 9)

- **In-scope test programs.**
  - Of the 246 Flink test programs with a recorded result, 180 use only operators the profile plans for FC1.
  - The other 66 need FC2 or later operators: over aggregates, temporal and lookup joins, `MATCH_RECOGNIZE`, correlate, multi-way joins, process table functions and batch operators.
  - FC1's bar is therefore **at least 144 of those 180 (80 %)** matching the oracle, plus Nexmark q0–q22 except q6.
- **`.q` files.** They are not executable until Task 22 (approved deferral). Their FC1 bar stays at the parser and binder level: no regression from 790 parsed and 700 bound.
- **Unchanged:** the rest of the FC1 gate (the SQL Client, the JDBC driver and `loams-flink`'s tests; exactly-once kill -9 recovery into Iceberg and the filesystem).

## Next

FC1 Tasks 10–25 in plan order. The planner (Task 10) comes first, lowering bound SQL to `OpSpec`s, then the runtime (Task 11).
