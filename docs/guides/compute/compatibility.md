# Compute compatibility matrix

Status: **preview**. Each product runs unmodified on Loams Postgres and is
judged by its own suite plus the Loams cases, strictly against its
`xfail.toml` (design 68 §12, design 74 §13). The section below is rendered
by `scripts/compute/matrix.py durable` from a run of the local nightly
([`scripts/nightly/run-local.sh`](../nightly-local.md)) and committed by hand. Do not edit it by hand. The Temporal
gateway's own matrix is in [temporal-gateway.md](../durable/temporal-gateway.md).

Verdicts:
- **green**: every failure is a listed expected failure;
- **red**: an unlisted failure, an unexpected pass or a stale entry;
- **not run**: there is no result in this run;
- **not merged**: the product's harness is not on this branch.

## Durable execution

<!-- matrix:durable:begin -->
Run: 2026-10-28 (run 40618803289, the first); Loams dev (commit `ae90681df`).

| Product | Version | Endpoint mode | Verdict | Passed | Failed (expected) | Skipped | Expected failures |
|---|---|---|---|---|---|---|---|
| Loams Postgres databases and endpoint modes (DX0 Task 2) | Postgres client postgres:17.10 | direct, txn | **green** | 5 | 1 (1) | 0 | `no-session-endpoint` (https://github.com/ostrium-labs/loams/issues/1129) |
| Temporal server (compat mode), Loams Postgres 16 | Temporal 1.32.1 | direct | **green** | 1579 | 0 (0) | 6 | none |
| Temporal server (compat mode), Loams Postgres 17 (expected-failure leg: Temporal lists Postgres 13-16) | Temporal 1.32.1 | direct | **red** | 1575 | 4 (0) | 6 | none |
| Temporal history corpus replays | Go SDK 1.49.0, TypeScript SDK 1.24.0 | none (files) | **green** | 2 | 0 (0) | 0 | none |
| DBOS Transact (Python) | dbos-transact-py 3.2.0 | direct | **red** | 0 | 2 (0) | 0 | `txn-mode` (https://github.com/ostrium-labs/loams/issues/1149) |
| DBOS Transact (TypeScript) | dbos-transact-ts 5.2.11 | direct | **red** | 0 | 2 (0) | 0 | `txn-mode` (https://github.com/ostrium-labs/loams/issues/1149) |
| DBOS Transact (Go) | dbos-transact-golang 1.6.0 | direct | **red** | 3 | 1 (0) | 0 | `txn-mode` (https://github.com/ostrium-labs/loams/issues/1149) |
| DBOS Transact (Java) | dbos-transact-java 1.2.0 | direct | **not run** | – | – | – | `txn-mode` (https://github.com/ostrium-labs/loams/issues/1149) |
| Airflow with the Edge executor | Airflow 3.3.2, edge3 4.3.2 | direct | **red** | 1 | 0 (0) | 0 | none |
| Airflow with the LocalExecutor (comparison) | Airflow 3.3.2 | direct | **red** | 1 | 0 (0) | 0 | none |

The corpus: 31 scenarios (DX1 9, DX2 15, DX3 3, DX4 4); 68 histories (36 go, 32 ts); generated against: start-dev.

Problems:

- Temporal server (compat mode), Loams Postgres 17: unlisted failure: temporal-pg17::temporal_soak_no_failed_workflows
- Temporal server (compat mode), Loams Postgres 17: unlisted failure: temporal-pg17::temporal_history_restart_recovers
- Temporal server (compat mode), Loams Postgres 17: unlisted failure: temporal-pg17::temporal_compute_resume_recovers
- Temporal server (compat mode), Loams Postgres 17: unlisted failure: temporal-pg17::temporal_txn_mode_xfail
- DBOS Transact (Python): compute-dbos/dbos-py-suite.junit.xml: cannot read JUnit: no element found: line 1, column 0
- DBOS Transact (Python): stale entry: txn-mode: 'dbos-py::dbos_py_txn_mode_xfail' matched no test in this run (remove it, or fix the name)
- DBOS Transact (Python): unlisted failure: dbos-py::dbos_py_suite_ran
- DBOS Transact (Python): unlisted failure: dbos-py::dbos_py_crash_recovery_exactly_once
- DBOS Transact (TypeScript): compute-dbos/dbos-ts-suite.junit.xml: cannot read JUnit: no element found: line 1, column 0
- DBOS Transact (TypeScript): stale entry: txn-mode: 'dbos-ts::dbos_ts_txn_mode_xfail' matched no test in this run (remove it, or fix the name)
- DBOS Transact (TypeScript): unlisted failure: dbos-ts::dbos_ts_suite_ran
- DBOS Transact (TypeScript): unlisted failure: dbos-ts::dbos_ts_crash_recovery_exactly_once
- DBOS Transact (Go): compute-dbos/dbos-go-suite.junit.xml: cannot read JUnit: no element found: line 1, column 0
- DBOS Transact (Go): stale entry: txn-mode: 'dbos-go::dbos_go_txn_mode_xfail' matched no test in this run (remove it, or fix the name)
- DBOS Transact (Go): unlisted failure: dbos-go::dbos_go_suite_ran
- Airflow with the Edge executor: the leg exited 1 before its strict check finished
- Airflow with the LocalExecutor (comparison): the leg exited 1 before its strict check finished
<!-- matrix:durable:end -->
