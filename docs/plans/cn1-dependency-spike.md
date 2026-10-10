# CN1 — the connector dependency spike

Task 0 of [the CN1 plan](2026-10-01-cn1-starred-connectors.md): reconcile CN1 with what is actually built, and measure the dependency stack before any of it is linked. Recorded 2026-10-04 on `cn1-t1-registry`, covering Tasks 0–2 (the registry). Tasks 4–13 append their measurements as they land.

Every version below was read from the crates.io API on 2026-10-04. Every SPDX id is the `license` field of that exact version, not a guess from the crate's repository.

## 1. Cargo dependencies (CN1 Tech Stack)

| Crate | Plan said | Latest | SPDX | Verdict |
|---|---|---|---|---|
| `jsonschema` | 0.30+ | **0.58.5** | MIT | **Bumped to 0.58** (ruling 2). `validator_for` and `is_valid` are unchanged. `default-features = false` drops the HTTP and file resolvers: `connectors/schema/connector.schema.json` has no external `$ref`, and an instance config's `$ref` is a repository path `loams-flow` resolves itself |
| `serde_norway` | "Task 0 picks a maintained YAML crate" | **0.9.42** | MIT OR Apache-2.0 | **Chosen** (ruling 3). The maintained fork; `serde_yaml` is unmaintained and `serde_yaml_ng` 0.10.0 has a smaller ecosystem |
| `rdkafka` | 0.39 | 0.39.0 | MIT | As planned (Task 6). Ships `librdkafka` (BSD-2-Clause) as a static build behind the `kafka` feature |
| `tokio-postgres` | 0.7 | 0.7.18 | MIT OR Apache-2.0 | As planned (Task 7) |
| `mysql_async` | 0.37 | 0.37.1 | MIT OR Apache-2.0 | As planned (Task 7) |
| `object_store` | 0.14 | 0.14.2 | MIT OR Apache-2.0 | As planned (Task 9) |
| `iceberg` | 0.10 | 0.10.1 | Apache-2.0 | As planned (Task 9) |
| `apache-avro` | 0.22 | 0.22.0 | Apache-2.0 | As planned (Task 5) |
| `adbc_core`, `adbc_driver_manager` | 0.24 | 0.24.0 | Apache-2.0 | As planned (Task 11). **Constrains arrow to `>=58, <60`** — see §2 |
| `clickhouse` | 0.15 | 0.15.2 | MIT OR Apache-2.0 | As planned (Task 10) |
| `aws-sdk-kinesis` | 1.x | 1.125.0 | Apache-2.0 | As planned (Task 13) |
| `redis` | 1.7 | 1.7.1 | BSD-3-Clause | As planned (Task 13) |
| `parquet` | 59 | **60.0.0** | Apache-2.0 | **Pinned to 59** — see §2 |
| `arrow-ipc`, `arrow-flight` | 59 | **60.0.0** | Apache-2.0 | **Pinned to 59** — see §2 |

No dependency is refused by D359. The only non-permissive ids CN1 records are the two JDBC drivers it explicitly does not ship, both in `connectors/licences.toml` with the reason: `mariadb-java-client` (LGPL-2.1-or-later, loaded and never modified, Ruling 8) and `mysql-connector-j` (GPL-2.0-with-classpath-exception, the user's choice, Ruling 8). Neither is a Rust dependency, so `fabric/deny.toml` — a copy of the root's permissive allow-list — is unaffected.

## 2. The Arrow version conflict

`arrow`, `parquet`, `arrow-ipc` and `arrow-flight` are all at **60.0.0**. `adbc_core` 0.24.0 declares `arrow-array >=58, <60` and `arrow-schema >=58, <60`, so **arrow 60 cannot be used** by Task 11's ADBC path without an `adbc_core` release that widens the bound.

The Tech Stack's "parquet 59, arrow-ipc/arrow-flight 59" is therefore the only version that satisfies both Task 5's format codecs and Task 11's ADBC bulk loads, and **the `fabric/` workspace pins 59** (ruling 4). The engine workspace stays on `arrow` 58.4; the two are separate workspaces with separate lockfiles (`fabric/Cargo.lock`), so there is no resolution conflict today.

The open question this defers: FL1's `loams-fabric-envelope` passes `RecordBatch` values across the Fluss and Iggy boundary, so its `arrow` major must also be one `fluss-rs` accepts. If `fluss-rs` 1.0.0 turns out to accept only one major, the Fabric workspace's Arrow is that one and 59 may have to move. FL1's spike records it; CN1 Task 5 and Task 11 read the answer.

## 3. `rdkafka`'s static build

Not yet measured. `rdkafka` 0.39 with `cmake-build` and `ssl-vendored` builds librdkafka from source, and CN1 makes it the heaviest addition to the Fabric workspace. It sits behind the `kafka` feature (on in CI and release, off for local builds of other tasks), so the measurement belongs to CN1 Task 6, which is where the first build happens. Recorded here as outstanding rather than guessed.

## 4. ADBC drivers (Q349, Ruling 9)

Not yet verified. Task 0 also asks for per-platform availability of the Snowflake, BigQuery, Postgres, SQLite, DuckDB and Flight SQL drivers, their download sources and SHA-256s, and a load test per platform through `adbc_driver_manager` 0.24 with arrow 59. Those checks need network fetches of Go- and C++-built shared libraries and belong to CN1 Task 11, which is the first task to load one. `deploy/fabric/adbc/drivers.toml` (names, versions, URLs, SHA-256s) is Task 11's file; nothing is written before then.

## 5. Debezium Server, Camel and Iggy

Not yet verified. Task 0 asks for the Debezium Server 3.7 image, its HTTP sink and CloudEvents converter property names, whether its offset store can be other than a file; the Camel version to pin and `camel-iggy`'s status; and the Iggy plugin list and config keys. All three need the FL1 stack running, which does not exist yet, so these checks belong to CN1 Tasks 8, 10 and 12 respectively.

What **is** settled now, from the drift checks of Task 2:

- **Camel 4.22.1 is the pin** (`loams-connect`, Ruling 7). Its component inventory at tag `camel-4.22.1` is 319 directories under `components/` plus the group sub-module trees (`camel-aws` 37 entries, `camel-azure`, `camel-debezium`), and `camel-iggy`, `camel-clickhouse`, `camel-jdbc`, `camel-sql`, `camel-cloudevents` and `camel-kafka` are all present — so CN-R5's "camel-iggy is a Preview component" risk is confirmed and the contract tests on every Camel bump (CN-R5's mitigation) are the right response.
- **`camel-catalog`'s JSON is not a complete inventory of Camel's components.** `kubernetes`, `mail`, `parquet-avro`, `jackson-avro`, `jackson-protobuf`, `azure-eventgrid` and `aws2-redshift` exist as modules but have no catalog JSON. `camel_catalog.py` therefore treats `apache/camel`'s module tree as the authority for existence and the catalog JSON as the authority for producer/consumer direction, and reports a module-without-catalog-JSON as a warning rather than an error. Without this, CN-R1's mitigation would report seven false positives every week.
- **`mcp-server` does not exist** in `apache/camel` at that tag, so Appendix A's `mcp-server` cell (A.16) is the one genuine drift finding from the Camel check. Every other named Camel cell resolves.

## 6. Iggy's message and batch limits (Ruling 3)

Not yet measured. Ruling 3 makes `batch_rows` (default 65 536) and `batch_bytes` (default 8 MiB) split a batch that exceeds Iggy's limit, and Task 0 records the limit itself. Iggy 0.9.0's maximum message size is measured in CN1 Task 5, the first task that emits a batch. Both limits are recorded in the ★ manifests now (`limits.batch_rows: 65536`, `limits.batch_bytes: 8388608`) so the registry is honest about the intent before the measurement lands.

## 7. What this spike does not cover

CN1 is planned after FL1 and needs FL1's `ingest` core, its provisioning and its plugins. Nothing in FL1 exists on this branch, so CN1 Task 0's first instruction — "read FL1 as built" — could not be carried out, and every check in §3–§6 that needs the running stack is deferred to the task that first needs it. `loams-flow` was written against the plan's `Produces` blocks and design §33 rather than against FL1's code; CN1 Task 3's `IngestCore` integration is the first point where the two meet, and the plan's own Global Constraints say CN1 "changes no engine code", which held.
