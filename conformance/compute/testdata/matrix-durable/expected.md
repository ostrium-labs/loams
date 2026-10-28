Run: 2026-10-27; Loams 0.0.0-fixture (commit `0000000`).

| Product | Version | Endpoint mode | Verdict | Passed | Failed (expected) | Skipped | Expected failures |
|---|---|---|---|---|---|---|---|
| Loams Postgres databases and endpoint modes (DX0 Task 2) | Postgres client postgres:17.10 | direct, txn | **green** | 2 | 1 (1) | 0 | `no-session-endpoint` (#1129) |
| Temporal server (compat mode), Loams Postgres 16 | Temporal 1.32.1 | direct | **red** | 2 | 1 (0) | 1 | none |
| Temporal server (compat mode), Loams Postgres 17 (expected-failure leg: Temporal lists Postgres 13-16) | Temporal 1.32.1 | direct | **not merged** | – | – | – | – |
| Temporal history corpus replays | Go SDK 1.49.0, TypeScript SDK 1.24.0 | none (files) | **green** | 2 | 0 (0) | 0 | none |
| DBOS Transact (Python) | dbos-transact-py 3.2.0 | direct | **not run** | – | – | – | none |
| DBOS Transact (TypeScript) | dbos-transact-ts ? | direct | **not merged** | – | – | – | – |
| DBOS Transact (Go) | dbos-transact-golang ? | direct | **not merged** | – | – | – | – |
| DBOS Transact (Java) | dbos-transact-java ? | direct | **not merged** | – | – | – | – |
| Airflow with the Edge executor | Airflow 3.3.2, edge3 4.3.2 | direct | **red** | 2 | 0 (0) | 0 | `txn-mode` (#1) |
| Airflow with the LocalExecutor (comparison) | Airflow 3.3.2 | direct | **not merged** | – | – | – | – |

The corpus: 3 scenarios (DX1 1, DX2 1, DX4 1); 5 histories (3 go, 2 ts); generated against: start-dev.

Problems:

- Temporal server (compat mode), Loams Postgres 16: unlisted failure: temporal-pg16::temporal_soak_no_failed_workflows
- Airflow with the Edge executor: unexpected pass: airflow-edge::airflow_txn_mode_leg is listed by txn-mode (#1); remove the entry
