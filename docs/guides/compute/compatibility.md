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
Run: local run 2026-10-28 352d77294.

| Product | Version | Endpoint mode | Verdict | Passed | Failed (expected) | Skipped | Expected failures |
|---|---|---|---|---|---|---|---|
| Loams Postgres databases and endpoint modes (DX0 Task 2) | Postgres client postgres:17.10 | direct, txn | **green** | 5 | 1 (1) | 0 | `no-session-endpoint` (https://github.com/ostrium-labs/loams/issues/1129) |
| Temporal server (compat mode), Loams Postgres 16 | Temporal 1.32.1 | direct | **not run** | – | – | – | none |
| Temporal server (compat mode), Loams Postgres 17 (expected-failure leg: Temporal lists Postgres 13-16) | Temporal 1.32.1 | direct | **not run** | – | – | – | none |
| Temporal history corpus replays | Go SDK 1.49.0, TypeScript SDK 1.24.0 | none (files) | **not run** | – | – | – | none |
| DBOS Transact (Python) | dbos-transact-py 3.2.0 | direct | **not run** | – | – | – | `txn-mode` (https://github.com/ostrium-labs/loams/issues/1149) |
| DBOS Transact (TypeScript) | dbos-transact-ts 5.2.11 | direct | **not run** | – | – | – | `txn-mode` (https://github.com/ostrium-labs/loams/issues/1149) |
| DBOS Transact (Go) | dbos-transact-golang 1.6.0 | direct | **not run** | – | – | – | `txn-mode` (https://github.com/ostrium-labs/loams/issues/1149) |
| DBOS Transact (Java) | dbos-transact-java 1.2.0 | direct | **not run** | – | – | – | none |
| Airflow with the Edge executor | Airflow 3.3.2, edge3 4.3.2 | direct | **not run** | – | – | – | none |
| Airflow with the LocalExecutor (comparison) | Airflow 3.3.2 | direct | **not run** | – | – | – | none |

The corpus: 31 scenarios (DX1 9, DX2 15, DX3 3, DX4 4); 100 histories (32 dotnet, 36 go, 32 ts); generated against: 16, start-dev.
<!-- matrix:durable:end -->
