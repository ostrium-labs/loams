# Compute conformance: unmodified products on Loams

Design [§68](../../docs/design/68-open-protocol-compute.md) §12 and design
[§74](../../docs/design/74-durable-execution-compat.md) §13–§14; plans
[OC1](../../docs/plans/2026-10-20-oc1-open-protocol-compute.md) (OC0) and
[DX1](../../docs/plans/2026-10-21-dx1-durable-execution-compat.md) (DX0).

Each product runs exactly as released, against Loams Postgres (and, where it
needs one, the bucket), and is judged by **its own** test suite plus the few
cases Loams' primitives must survive. Every result is checked against a strict
list of expected failures, so a surprise in either direction fails the run.

Status: the harness (DX0 Task 1), the databases and endpoint modes (Task 2,
`durable/`), Temporal compat mode (Task 3, `temporal/`), the history corpus
(Task 4, `temporal/corpus/`) and Airflow with the Edge executor (Task 6,
`airflow/`, written but not yet run end to end). DBOS (Task 5) is not merged
yet.

## Layout

| Path | What |
|---|---|
| `pins.env` | every image by digest and package by exact version (D1274), read by the scripts, the compose override (`--env-file`) and the workflow |
| `licences.toml` | the licence of each pin and where it was read; `scripts/compute/licence_gate.sh` refuses AGPL, BUSL, SSPL, ELv2, FSL, CCL, RSAL, Commons Clause and unlicensed pins (D1271), with PgDog the one recorded exception (D318) |
| `lib.sh` | the shared functions (below) |
| `xfail.schema.json` | the format of `<product>/xfail.toml` |
| `pgdog-txn.yaml`, `pgdog/` | the transaction-mode overlay: PgDog in transaction pooling in front of Loams Postgres, for the legs that show what fails there. Never distributed; PgDog is AGPL-3.0 and runs unmodified as a separate process ([notice](../../deploy/pgdog/LICENSE-NOTICE.md)) |
| `selftest.sh`, `testdata/` | the harness's own tests; the pull-request CI runs them |
| `<product>/` | `run.sh`, `xfail.toml`, `README.md` per product (with its task) |
| `../../deploy/release/compose.compute.yaml` | the opt-in compose override: profiles `temporal`, `dbos`, `airflow` |
| `../../scripts/compute/` | `xfail_check.py`, `junit.py`, `licence_gate.{sh,py}` |

## Endpoint modes

The 0.1.1 release compose has only **direct** Loams Postgres endpoints
(DX1 ruling R0.7): the included stack's `compute1` (`compute1:55433` on the
`loams_default` network) and, with `--profile postgres-control`, each
`pg-control` endpoint's compute (`ep-<ulid>:55433`). Every Temporal, DBOS and
Airflow suite runs on `direct`. `oc_pg_mode session` fails until a
session-pooled endpoint exists
([#1129](https://github.com/ostrium-labs/loams/issues/1129)); `oc_pg_mode txn`
is the PgDog of the overlay.

Never Loams SQL: no product here keeps state there (`no_loams_sql_in_dx0`).

## Running one product

```sh
. conformance/compute/lib.sh
oc_up temporal                 # the stack + the profile, pins applied
oc_temporal_wait
conformance/compute/temporal/run.sh
oc_down temporal               # that profile's services and volumes only
```

`lib.sh` provides:

- `oc_up <profile> [txn]`, `oc_down <profile> [txn]`, `oc_compose [--txn] <args>`
- `oc_pg_mode <txn|session|direct> [database] [user:password]`
- `oc_psql <url> <sql>`, `oc_wait_pg [url]`, `oc_wait_http <url>`,
  `oc_temporal_wait [address]`, `oc_bucket_ls s3://bucket/prefix`: all from a
  container on the stack's network, since the compute services publish no
  host port
- `oc_junit_case <suite> <name> <pass|fail|error|skip> [message]` (writes
  `$OC_OUT/<suite>.junit.xml`)
- `oc_xfail_check <product> <junit.xml...>`

## Adding a product

1. Pin it in `pins.env` (digest or exact version, at least 14 days old) and
   record its licence in `licences.toml` (`licence_gate.sh --online` re-reads
   it).
2. Add its services to `compose.compute.yaml` under a profile, with no
   `ports:`.
3. Write `<product>/run.sh`: run the product's own suite, write JUnit, then
   `oc_xfail_check <product> …`.
4. List expected failures in `<product>/xfail.toml`, each with a reason and an
   issue (`[[xfail]] id, tests, reason, issue, since`).
5. Run `conformance/compute/selftest.sh`.
