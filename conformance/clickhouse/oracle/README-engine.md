The Rust engine oracle (HE1 Task 23) runs without libchdb. Build from `fabric/`:

```sh
cargo build --locked -p loams-house-engine --bin loams-house-engine-dev -p loams-house-mine --bin loams-house-mine
```

Start the reference server from this directory with `docker compose up -d --wait clickhouse-26-9`. Then, from the repository root, in separate terminals:

```sh
mkdir -p /tmp/house-oracle-fixtures
fabric/target/debug/loams-house-engine-dev --fixtures /tmp/house-oracle-fixtures
```

```sh
LOAMS_ORACLE_PASSWORD=oracle fabric/target/debug/loams-house-mine oracle \
  --only house-rust --corpora conformance/clickhouse/oracle/he1.toml \
  --fixtures /tmp/house-oracle-fixtures --task 23 --check
```

Use a dedicated reference server with synthetic data only. After setup on the
baseline, the miner exports all non-system tables to a new generation directory
and publishes `snapshot.json` atomically. The development server reads that
manifest for each query; both processes must use the same fixture directory.
Setup never runs on a `seed_from` target. Defaults, engine, sorting key and
partition key are retained in the manifest; the current reader exposes the
captured rows as memory tables. `FINAL` remains refused until Task 27. Enum labels are restored recursively to
numeric enum arrays. AggregateFunction columns export their serialized state
through `toString`, with their original types preserved in the manifest.

Snapshot database/table path components are hex encoded. The loader validates
column order, restores exact ClickHouse field metadata and refuses Parquet
paths outside the snapshot root. Previous generations remain available until
the caller removes its temporary fixture directory.

The server accepts POST bodies and `query`, `database`, and `param_*` URL
parameters. It writes `JSONCompactEachRowWithNamesAndTypes`; other formats and
settings fail explicitly pending Tasks 36–37. It binds only loopback and is not
included in release images. `version()` is not implemented yet; the oracle's
version diagnostic may report code 48, independently of case comparisons.

Rust targets are opt-in through `--only house-rust`, preserving the existing
HE0 default target selection. `--task N` additionally fails on known bugs whose
triage entry has `task = "N"`; other existing triage rules still apply. A run
that selects no cases fails. The separate `he1.toml` manifest holds the initial
front-end and seed round-trip cases.

`.github/workflows/house-engine-oracle.yml` runs the comparison on GitHub-hosted
runners with the existing digest pin. Its ten-minute budget and required-check
promotion must be measured on GitHub before claiming the Task 23 exit gate.

Local verification on 2026-10-25: all 11 HE1 cases match ClickHouse 26.9.2.8
with zero mismatches, including the seeded type and aggregate-state tables.
The four-crate regression suite has 106 passing tests and one ignored fixture
recorder. Strict Clippy, formatting, and dependency license/source checks pass.
