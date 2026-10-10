# CN2 — Loams Flow Connectors in Production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact names, paths, event types or defaults, use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Planned** (2026-10-10), **revised the same day** to the owner's direction on connectors (see "Owner rulings 2026-10-10" below). Track CN, design [§33](../design/33-connectors.md) (D352–D359, Q348–Q359, and §33's revision note of 2026-10-10), after [CN1](2026-10-01-cn1-starred-connectors.md). §33 §8 originally defined CN2 as "every P2 row of Appendix A through stock Camel components (`loams-connect`) or Iggy plugins". The owner's direction replaces that: Loams builds **its own first-party source and sink connectors**, in Rust, in-tree, on a shared connector SDK, for every P1 and P2 system, and tests each one against the **real** external system. Camel, Debezium Server and Iggy's upstream plugins stay only as declared fallbacks, each with a task that replaces the most-used ones. This plan also carries the production work §33 §9–§11 leave open: the fleet on Kubernetes, connector observability and the Q359 activity counters, egress and secret hardening, remote ingress once the auth plan (Q30) allows it, and the upstream contributions of Q357.
>
> **As built at `1dc6e8a3` (dev, 2026-10-10).** The plan is reconciled with this state. Task 0 re-checks it.
> - **CN1 Tasks 1–2 are merged.** `fabric/crates/loams-flow` has `manifest.rs`, `registry.rs` and `validate.rs` (`ConnectorSpec`, `RuntimeSpec { kind, reference, version }`, `Registry::{load, get, list, starred, by_category}`, `validate_manifest`, `semantic_rules`, `check_use`, `LicenceGate`). `fabric/proto/loams/flow/v1/{connector,instance}.proto` exist, and there is no `flow.proto` yet. `connectors/registry/` holds **204 manifests**: 21 ★, 3 unstarred P1 (Zulip, ItsPlane, Forgejo, CN1 Task 15), Grafeo (D634), the rest generated stubs. `catalog.csv` assigns the **73 P2 rows** like this: 63 `camel`, 9 `openapi` (Delta Lake, Kestra companion, Segment, SendGrid, PostHog, HubSpot, Shopify, GitLab, Prometheus) and 1 `native` (Grafeo). `scripts/connectors/{gen_registry.py,camel_catalog.py,kestra_catalog.py,matrix.py,appendix.py}` and the `fabric.yml` jobs `connectors-unit` and `connectors-drift` run.
> - **Not built:** CN1 Task 3 (instances, `FlowService`, runtime supervisors, `SecretStore`, `loams-flow-conformance`), Tasks 4–14 (the ★ connectors and the gate), Task 12 (`loams-connect`; `fabric.yml`'s `connect-routes` job is a switched-off placeholder) and the Rust half of Task 15. Only `connect/routes/templates/{zulip,itsplane,forgejo}-import.yaml.tmpl` exist.
> - **FL1 is Planned, not built.** There is no `loams-fabric` binary, no `ingest`, no `loams-fabric-envelope`, no `deploy/fabric/` and no Iggy or Fluss stack in the tree. The `fabric/` workspace holds `loams-flow`, `loams-flow-proto` and HS1's House crates (`loams-chdb*`, `loams-house*`).
> - **Grafeo moved.** D741 and §48 §4.4 moved `loams-graph` and `loams.graph.v1` into the engine workspace (`proto/loams/graph/v1/graph.proto`). `connectors/registry/grafeo.yaml` still describes the embedded, auth-`none` engine of D634 (b).
>
> CN2a (registry truth) and the SDK core of CN2b (Tasks 4–6) can start now: neither needs FL1, because the SDK reaches the Fabric through a trait with an in-memory implementation. End-to-end tests through `ingest` wait for FL1 (see Execution order).

## Owner rulings 2026-10-10

The owner's direction, verbatim: *"no, redpanda etc are for real — build our own source and sink connectors for Loams; use proprietary in CI; do the best for others"*. Read with the orchestrator's brief: Redpanda, Valkey, R2, OpenSearch, MariaDB and the like are real external systems that need real connectors; Loams builds its own first-party connectors in Rust on its connector runtime rather than defaulting to Camel; Camel and Debezium remain only where breadth or CDC needs them, marked as fallbacks with a replacement task; proprietary test images may run as CI-only services and are never shipped; every other open question is settled here by the best default. These rulings bind every task below. The decision log is **not** edited by this plan; the rulings that change a D-number's meaning (OR-1, OR-4, OR-9) are listed under "Follow-ups outside this plan".

| # | Ruling | Replaces or answers |
|---|---|---|
| OR-1 | **Native first for P1 and P2.** Every P1 and P2 system of §33 Appendix A gets a first-party Rust source and/or sink, written by Loams, in-tree under `fabric/crates/`, on the shared SDK `loams-connector-sdk` (OR-5), run by `NativeRuntime`. "Buy first" (D354) now means *reuse protocol libraries* (a maintained client crate for the wire protocol), never *reuse a connector runtime*. The only P1/P2 directions that are not native by default are those OR-4 names | D354's "buy first" for P1/P2; §33 §2.2's first non-goal for P1/P2; §33 §8's CN2 line |
| OR-2 | **Real systems, real tests.** Each external system is its own manifest with its own suites, run against that system: its official or vendor image, its vendor's emulator, a fixture recorded from the real system, or a real account in the nightly job. Systems that speak a shared protocol share code (one Kafka-protocol implementation serves Kafka, Redpanda and Event Hubs' Kafka endpoint), declared as a **target profile** (formerly "twin"), but the target's suite always runs against the real target. A Loams module (collections' Elasticsearch and Qdrant APIs, House as a ClickHouse endpoint, Loams Postgres, the Fabric's Iggy) **never stands in** for an external system in a suite; it may be an additional target, tested after the real one | CN2-Q2 (twins); CN1 Task 10's "Loams' own ES gateway" as a stand-in |
| OR-3 | **Proprietary and source-available images run in CI only.** As `ci-service` (Task 3): Oracle Database Free, SQL Server Developer, the Azure Event Hubs and Service Bus emulators, Splunk Enterprise (trial licence), DynamoDB Local, CockroachDB, MongoDB (SSPL), Redpanda (BSL), MinIO (AGPL), Elasticsearch (AGPL/ELv2/SSPL), Redis 8 (tri-licence). Never in an image Loams builds, never in the default compose profile. Real cloud and SaaS accounts (R2, GCS, Azure, AWS, Snowflake, Databricks, Salesforce, Stripe test mode, and the rest) run in the scheduled `connectors-nightly` job with repository secrets. Oracle's `ojdbc` and Instant Client remain `user-supplied` (CN1 Ruling 8) | CN2-Q6, CN2-Q7 |
| OR-4 | **Fallbacks, declared and temporary.** Camel (`loams-connect`), Debezium Server and Iggy's upstream sink plugins remain only as **fallback** runtimes, never the default for an instance (an instance opts in with `runtime: fallback`). The fallback set is: Debezium for **Oracle CDC** (default until Task 23's verdict) and as opt-in fallback for Postgres, MySQL/MariaDB, SQL Server and MongoDB CDC; Camel for **Oracle batch and upsert** (until Task 23), **Salesforce CDC** (until Task 23), **JMS providers that do not speak AMQP 1.0** (JMS is a Java API), the **JDBC ★** (generic JDBC is Java by nature) and P3 rows CN3's generator does not fit; Iggy's `elasticsearch_*`, `clickhouse_sink`, `s3_sink`, `iceberg_sink`, `postgres_sink`, `postgres_source` and `mongodb_sink` as fallbacks for the native connectors that replace them. Every fallback is a manifest `runtime.fallback` block with `replaced_by` naming its replacement task. Iggy itself stays the Fabric's bus and the home of Loams' own plugins (`fluss_sink`, `loams_sink`, §32 D336) | D354's runtime table for P1/P2; CN2-Q5's framing; Q351 |
| OR-5 | **One connector SDK.** The crate `fabric/crates/loams-connector-sdk` owns the source and sink traits, splits, positions, checkpoints, delivery modes (at-least-once floor, effectively-once and exactly-once where possible), backpressure, retries and error classes, schema mapping, the CloudEvents envelope builder, and the hooks for metrics, secrets and egress. The conformance kit stays `loams-flow-conformance` (CN1's name) and grows the suites of Task 6. CN1 Task 3's `Source` and `Sink` move into the SDK with their method names kept, and `loams-flow` re-exports them. If CN1 Task 3 has not merged when Task 4 starts, Task 4 writes the traits in the SDK first and CN1 Task 3 consumes them | new |
| OR-6 | **One crate per connector family**, `fabric/crates/loams-connector-<family>` (Shared contracts lists them), each depending only on the SDK and its protocol crates. A family task builds and tests one crate (the build machine's one-build rule), heavy dependencies (`rdkafka`, `deltalake`, the AWS SDKs) stay out of every other crate, and each crate can later be wrapped as an Iggy plugin (Task 29, Q357). `loams-flow` links the families behind cargo features `conn-<family>`, all on in release builds and off by default in dev builds | CN1's `loams_flow::connectors::*` layout (Task 8 moves the ★ modules) |
| OR-7 | **Delivery.** At-least-once is the floor for every native connector. `effectively_once` (a new enum value) is declared when the target deduplicates by `ce_id` or by a natural key (Elasticsearch `_id`, ClickHouse dedup tokens, NATS `Nats-Msg-Id`, deterministic object names). `exactly_once` is declared only through `TransactionalSink`, where the target stores Loams' checkpoint atomically with the data: Postgres, MySQL/MariaDB, SQL Server, MongoDB replica sets, Kafka/Redpanda transactions, Delta Lake's `txn` action and Iceberg snapshot properties. Sources are at-least-once into `ingest`, whose `(source, id)` dedup (D334) makes them effectively once; their manifests say `effectively_once` only when their ids are stable across re-reads (§33 §6) | §33 §2.2 "exactly-once only where the system deduplicates" (kept, made precise) |
| OR-8 | **★ connectors that CN1 runs on a non-native runtime move to native in CN2**: Elasticsearch (Iggy → native, Task 15), the ClickHouse sink (Task 17), the S3 sink (Task 13), the Postgres streaming sink (Task 10), the Iceberg raw-topic sink (Task 17), and Debezium-Postgres and Debezium-MySQL (native CDC on the `postgresql` and `mysql` manifests becomes the default, Tasks 10–11; the two Debezium manifests stay as fallbacks). JDBC ★ stays on Camel (OR-4) | CN1 Tasks 8, 9, 10 runtimes (CN1's plan itself is not edited here) |
| OR-9 | **Native CDC** for Postgres (logical replication, `pgoutput`), MySQL and MariaDB (row binlog, GTID), SQL Server (CDC tables), MongoDB (change streams), DynamoDB Streams and CockroachDB (core changefeeds). It supersedes Q351's Iggy `postgres_source` default, which becomes a fallback. Oracle CDC stays on Debezium Server (LogMiner needs Oracle's Instant Client, proprietary and `user-supplied`); Task 23 runs a native spike and records a verdict | Q351; D357's "external databases go through Debezium Server" for these sources |
| OR-10 | **One manifest per system, one runtime per direction** (`runtime.source`, `runtime.sink`) | CN2-Q1 |
| OR-11 | **SaaS and HTTP APIs are declarative HTTP-API profiles** on the native HTTP and webhook connectors, for every P2 SaaS row, including those Camel covers (GitHub, Jira, Slack, Stripe, Zendesk, ServiceNow, Twilio, Google Workspace, Keycloak, Vault, Splunk, Salesforce REST and Bulk 2.0), and for the nine rows with no component. CN3's generator (Q358) emits the same profile format | CN2-Q3 |
| OR-12 | **gRPC is native**: a dynamic client (`tonic` with `prost-reflect`) over a user-supplied `FileDescriptorSet` or server reflection. Protobuf is a native codec | CN2-Q4 |
| OR-13 | **The Debezium row splits** into `debezium-sqlserver` and `debezium-oracle` (205 manifests). `debezium-oracle` is Oracle's default CDC path (OR-9); `debezium-sqlserver` is a fallback for Task 12's native CDC | CN2-Q5 |
| OR-14 | **Neo4j is native in both directions** over Neo4j's HTTP Query API (no Bolt, D634 (c)); `camel-neo4j` is not used. Task 2 amends Appendix A.3's note | CN2-Q8 |
| OR-15 | **MCP moves to P3** (no Camel component, no launch customer, Q355's rule). P2 becomes 73 rows after the Debezium split | CN2-Q9 |
| OR-16 | **CN2 renders the Kubernetes fleet** and holds a fenced instance lease on `_fabric.flow_objects`, plus per-split leases for native sources; FL3's metastore leases replace both behind the same trait | CN2-Q10 |
| OR-17 | **Q30's auth plan stays the gate** for remote ingress; no interim static-token route | CN2-Q11 |
| OR-18 | **Delta Lake is native in both directions** with the `deltalake` crate: the sink is exactly-once through Delta's `txn` action, and the source reads versions and, when enabled, the change data feed | CN2-Q12 |
| OR-19 | **AI and function connectors are sinks with `sink.reply`** now; FL3's enrich step can wrap them later | CN2-Q13 |
| OR-20 | **Grafeo's `loams.graph.v1` client** is generated across workspaces if `build.rs` can include the engine proto; else a checked copy with a drift test | CN2-Q14 |
| OR-21 | **`KubeSecretStore` now**, Dapr (D189) when `loams-dapr` exists | CN2-Q15 |
| OR-22 | **Azure Event Hubs runs on the native Kafka family** through its Kafka endpoint (SASL PLAIN with the connection string), and **Azure Service Bus and ActiveMQ** run on a native AMQP 1.0 client. Both avoid a second, Azure-specific SDK | new |
| OR-23 | **Status ladder.** `preview` = every declared capability green in PR CI on a container, an emulator or a recorded fixture of the real system. `stable` = `preview` plus 14 consecutive green nightlies against the real system (for cloud and SaaS systems, the real account). The CN2 gate requires `preview`; `stable` follows per connector | sharpens D353 rule 3 for CN2 |
| OR-24 | **Crate choices are defaults that Task 0 verifies** (licence, age ≥ 14 days, maintenance, build cost). If a crate fails, the replacement is a minimal client over the system's documented wire or HTTP protocol, written in the family crate; a silent switch to a fallback runtime is not allowed | new |

## Follow-ups outside this plan

- **Decision log.** OR-1, OR-4 and OR-9 change what D354, D357 and D358's rollout line mean for P1 and P2. The decision log needs entries (new D-numbers, or amendments) recording them; until then §33's revision note and this section are the record.
- **CN1.** CN1 Tasks 8, 9 (S3 sink), 10 and 13 are not built. OR-8 means their non-native halves would be built twice. CN1's plan should be revised to build them once on the SDK (Task 8 below describes the port either way).
- **`docs/plans/README.md`** still lists CN2 as "Not yet planned" with the old Camel description.

**Goal:** every P1 and P2 connector of §33 Appendix A built first-party in Rust and proven against its real system, and the whole connector set runnable in production:
- `loams-connector-sdk`: source and sink traits with splits, checkpoints, delivery modes up to exactly-once where the target allows it, backpressure, retries, schema mapping and the envelope; and a conformance kit whose suites run against real containers, recorded fixtures and nightly real accounts;
- native family crates for streaming (Kafka, Redpanda, Event Hubs, Kinesis, RabbitMQ, AMQP 1.0, NATS, MQTT, Pulsar, WebSocket, SQS, SNS, EventBridge, Pub/Sub, Event Grid), CDC (Postgres, MySQL, MariaDB, SQL Server, MongoDB, DynamoDB Streams, CockroachDB), databases (Postgres, MySQL, MariaDB, SQL Server, CockroachDB, Aurora, pgvector, MongoDB, Cassandra, DynamoDB, Redis, Valkey), search and vector (Elasticsearch, OpenSearch, Qdrant), analytics (ClickHouse, Redshift, Databricks, Trino, DuckDB, Delta Lake, Iceberg), objects (S3, R2, MinIO/RustFS, GCS, Azure Blob, ADLS Gen2, SFTP), HTTP-API profiles for SaaS, and gRPC, GraphQL, Neo4j, Grafeo, Prometheus, functions and AI;
- Camel, Debezium Server and Iggy plugins as declared fallbacks only, with the most-used ones replaced natively;
- the fleet on Kubernetes with a Helm chart, connector metrics and alerts, the Q359 activity counters (no billing), an egress guard, production secret stores and a threat model;
- remote ingress for webhooks, OTLP and Prometheus when Q30 lands, and the Q357 upstream plugins.

The exit is the "Exit criteria for production" checklist at the end of this plan, with the owning tasks.

**Architecture** (§33 §5 as revised, CN1's Architecture):
- **`loams-connector-sdk`** is the connector runtime contract: a source is a `SplitEnumerator` plus one `Source` per split; a sink is a `Sink`, optionally a `TransactionalSink`. The SDK's `Pipeline` drives both with bounded budgets (backpressure), retries by error class, checkpointing fenced by a lease epoch, and the envelope of §33 §6. It reaches the Fabric through `FabricWriter` and `FabricReader` traits, implemented over `ingest` and Iggy consumer groups by `loams-flow` (and in memory for tests).
- **Family crates** (`loams-connector-<family>`) implement the traits with a protocol crate each. A **target profile** lets one family implementation serve a protocol-compatible system (Redpanda on the Kafka family), with the target's own manifest, narrowing and suite (OR-2). An **HTTP-API profile** describes a REST API declaratively, run by the HTTP family (OR-11).
- **`loams-flow`** stays the one owner of the registry, instance validation and runtime supervision. `NativeRuntime` runs SDK connectors as tasks inside `loams-fabric flow`; `CamelRuntime`, `DebeziumRuntime` and `IggyRuntime` (CN1 Task 3) run fallbacks only (OR-4).
- **Production fleet.** Native connectors run inside `loams-fabric flow` pods with per-split leases; fallback runtimes run in their own pods. Supervision is single-writer per instance, enforced by a fenced lease on `_fabric.flow_objects` (Task 26), and every checkpoint write carries the lease epoch.

**Tech Stack:**
- Rust 1.97 (`fabric/` workspace `rust-version`), edition 2024, workspace lints, arrow 59 (CN1 Ruling 4).
- Existing `fabric/` dependencies: `jsonschema` 0.58, `serde_norway`, `connectrpc` 0.9 and `buffa` 0.9.2, `prost` 0.14.
- Protocol crates, defaults per OR-24 (Task 0 verifies version, licence, release date and build cost; each must be at least 14 days old; one family crate each):
  - streaming: `rdkafka` 0.39 (CN1; MIT, librdkafka BSD-2-Clause), `lapin` (MIT), `fe2o3-amqp` (MIT OR Apache-2.0, verify), `async-nats` (Apache-2.0), `rumqttc` (Apache-2.0), `pulsar` (verify), `tokio-tungstenite` (MIT);
  - CDC and databases: `tokio-postgres` 0.7 plus a `pgoutput` replication client (Task 0 picks between `supabase/etl`'s crates (Apache-2.0) and a fork of `rust-postgres` with replication support; verify), `mysql_async` 0.37 binlog streams and `mysql_common` (MIT OR Apache-2.0), `tiberius` (MIT OR Apache-2.0), `mongodb` 3.x (Apache-2.0), `scylla` 1.x (MIT OR Apache-2.0, for Cassandra), `redis` (CN1; BSD-3-Clause);
  - cloud: `aws-sdk-{sqs,sns,eventbridge,dynamodb,dynamodbstreams,kinesis,lambda,redshiftdata,sesv2}` (Apache-2.0), a Pub/Sub client (Task 0 picks between the official `google-cloud-pubsub` and `yoshidan/google-cloud-rust`, verify), `object_store` 0.14 with `aws`, `gcp` and `azure` (Apache-2.0), `russh` and `russh-sftp` (verify);
  - search, vector, analytics: plain `reqwest` for Elasticsearch, OpenSearch, Trino, Databricks, Neo4j and HTTP-API profiles; `qdrant-client` (Apache-2.0), `clickhouse` (CN1), `deltalake` (Apache-2.0), iceberg-rust 0.10 (CN1);
  - codecs and protocols: `arrow-csv` and `arrow-json` 59 (already in `fabric/Cargo.lock`), `prost-reflect` (MIT OR Apache-2.0, verify), `tonic` (MIT), `snap` (BSD-3-Clause, Prometheus remote-write);
  - fleet: `kube` and `k8s-openapi` (Apache-2.0; Task 0 picks the newest release at least 14 days old).
- Fallback runtimes, pinned by digest (OR-4 only): Apache Camel 4.22.x (the CN1 Task 12 patch), Debezium Server 3.7.0.Final, the Iggy connectors runtime at FL1's pinned `dina-kar/iggy` revision with Iggy 0.9.0.
- Real systems for tests, pinned by digest (OR-2, OR-3; `ci-service` marks a proprietary or source-available image):
  - streaming: Apache Kafka 4.x (Apache-2.0), Redpanda (BSL, `ci-service`), RabbitMQ (MPL-2.0), ActiveMQ Artemis (Apache-2.0), NATS with JetStream (Apache-2.0), Mosquitto (EPL-2.0), Pulsar (Apache-2.0), the Event Hubs and Service Bus emulators (Microsoft EULA, `ci-service`), the Pub/Sub emulator, floci 2.1.0 (D60) for SQS, SNS, EventBridge, Kinesis, DynamoDB and Lambda (Task 0 records which floci serves);
  - databases: Postgres 17 with `wal_level=logical` and pgvector, MySQL 8.4, MariaDB 11 LTS (GPL-2.0), CockroachDB (`ci-service`), SQL Server Developer (`ci-service`), Oracle Database Free (`ci-service`), MongoDB 8 replica set (SSPL, `ci-service`), Apache Cassandra 5, Valkey 8 (BSD-3-Clause), Redis 8 (`ci-service`), DynamoDB Local (`ci-service`);
  - search and analytics: Elasticsearch 9 (`ci-service`), OpenSearch 3 (Apache-2.0), Qdrant (Apache-2.0), ClickHouse (Apache-2.0), Trino (Apache-2.0), Lakekeeper (Apache-2.0);
  - objects: RustFS (Apache-2.0), MinIO (AGPL, `ci-service`), fake-gcs-server (BSD-2-Clause), Azurite (MIT), `atmoz/sftp` (MIT, verify);
  - other: Neo4j Community (GPL-3.0), Prometheus (Apache-2.0), OpenBao (MPL-2.0), Keycloak (Apache-2.0), Kestra OSS (Apache-2.0), stripe-mock (MIT), Splunk Enterprise (`ci-service`).
- Tools: compose (docker or podman) for the `connectors`, `connectors-p2` and `connectors-ci-only` profiles; kind for Kubernetes e2e (CI only); `camel` JBang inside the Camel image for fallback route validation; `buf`.

**Spec:**
- [§33](../design/33-connectors.md), all of it, with its revision note of 2026-10-10, D352–D359 and Q348–Q359 in the [decision log](../design/13-decision-log.md), and this plan's Owner rulings.
- [§32](../design/32-loams-flow-fabric-house.md): §5.4 (envelope, D334), §5.5 (adapters table), §5.7 (bridges), §6 (Flow; FL3's routes and leases).
- [CN1](2026-10-01-cn1-starred-connectors.md), its rulings 1–9 and its execution rulings 1–13 (carried in, except where OR-8 moves a runtime), and [`cn1-dependency-spike.md`](cn1-dependency-spike.md).
- [FL1](2026-10-01-fl1-fabric-foundation.md), as built when the end-to-end tests run.
- [§48](../design/48-loams-graph-production.md) §4.4 and D741 (Grafeo is a `loams.graph.v1` client), D634 (GQL, no Bolt), [`graph-db-rust-spike.md`](graph-db-rust-spike.md).
- [§27](../design/27-usage-hooks.md) §3.7 and D548–D552 (observers carry plain structs; no billing in this repository), D60 (CI-only services), D111 and Q30 (loopback until the auth plan), D189 (secrets), D270 (CloudEvents Kafka binary layout).

## Global Constraints

- **Worktree and branches.** Work in `~/Documents/Ostriumlabs/loams-wt/cn2-connectors`. One branch per milestone: `feat/cn2a-registry`, `feat/cn2b-sdk`, `feat/cn2c-<family>` (one per family task), `feat/cn2d-fallbacks` and `feat/cn2e-production`. Each is based on `dev`, with stacked PRs targeting `dev`; split a PR that passes about 1,500 lines. Use `git commit -s` (DCO). Commit areas: `sdk`, `flow`, `connectors`, `connect`, `deploy`, `ci`, `docs`.
- **Rust builds** use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR` and never build in `/tmp`. Run one cargo build at a time (jobs and linker from `~/.cargo/config.toml`). Build the touched crate only: `cargo test --manifest-path fabric/Cargo.toml -p loams-connector-<family>`, never `--workspace`. `rdkafka` (librdkafka), `deltalake` and the AWS SDKs are compiled only by the family crates that need them (OR-6). Compose stacks and kind run in CI, or locally only while no cargo build runs.
- **Native first** (OR-1). Before writing a connector, the task reads the system's wire protocol documentation and picks the protocol crate from Tech Stack; it does not wrap a connector runtime. A fallback (OR-4) is added only where Task 2's assignment table says so.
- **Real systems** (OR-2). Every suite names its `ServiceKind` and the real system behind it. A Loams module is never the system under test for an external connector; Task 6's `suite_service_is_the_real_system` enforces this.
- **Java is deferred** (owner, 2026-10-01). No `.java`, `.kt`, `.groovy` or `pom.xml` anywhere. Fallback Camel YAML may use only the languages `simple`, `constant`, `header`, `jq`, `jsonpath` and `jsonata`, and no `beans:` entry with a `type:` or `#class:` reference (Task 22's `templates_use_only_allowed_languages`).
- **Every manifest validates, and every declared capability has a test** (D353 rule 3). CN2 extends `every_declared_capability_has_a_test` to every manifest whose `status` is `preview` or `stable`. A P2 row is `preview` only when its suite is green (OR-23), and stays `planned` otherwise.
- **Licence gate** (D359). Every crate compiled into a family crate passes D359 (`cargo deny`-style check in Task 3). `ci-service` and `user-supplied` components (Task 3) never appear in an image Loams builds or in the default compose profile.
- **No secret** in a manifest, an instance spec, a profile, a rendered config (native, Camel, Debezium, Iggy), a Kubernetes object other than a `Secret`, a log line, a metric label, a checkpoint, a `Position::display` or an error (CN1 Task 3's `no_secret_in_rendered_configs_or_logs`, extended in Tasks 4 and 25).
- **Loopback only** (D111) for `FlowService` and every Loams-served connector endpoint until Task 28's gate (Q30) opens.
- **No billing** (D548–D552). Q359's counters are open observability, delivered to `ConnectorActivityObserver` as plain structs with no wire format, buffer or persistence. No field, metric or file name contains `price`, `invoice`, `credit`, `billable` or `meter`. (The SDK's backpressure unit is named `Budget`, not "credit", for this reason.) `scripts/ci/no-metering.sh` stays green, and this plan does not touch its allowlist.
- **Pins.** Exact versions for every new crate and image, images pinned by digest, each new dependency at least 14 days old. Record each pin in the task's commit message and in `cn2-dependency-spike.md`.
- **Tests skip without services**, as in CN1 (`LOAMS_CONNECTORS_STACK`; CN2 adds `LOAMS_CONNECTORS_P2_STACK` for `connectors-p2`, `LOAMS_CONNECTORS_CI_ONLY_STACK` for `connectors-ci-only`, and `LOAMS_<CONNECTOR>_*` credentials for nightly real-account jobs). A skipped suite never counts toward `preview`.

## Review Focus

1. **A manifest claims a capability its implementation or target cannot deliver**: a sink declared `exactly_once` without a transactional checkpoint, a target claiming what the real system lacks, a fallback Camel component declared in a direction its catalog refuses. Expected: refused by validation or caught by the suite. Tests: Task 1 `exactly_once_requires_transactional_sink`; Task 6 `every_declared_capability_has_a_test`, `target_suite_runs_against_target_system`; Task 22 `camel_direction_matches_catalog`.
2. **A Loams module or a mock stands in for a real external system** in a suite that counts toward `preview`. Expected: CI fails. Test: Task 6 `suite_service_is_the_real_system`.
3. **An exactly-once claim fails under a crash** at any point of the two-phase write. Expected: no loss, no duplicate. Tests: Task 6's `exactly-once` suite (`crash_at_every_fault_point`), run by Tasks 9, 10, 11, 12, 16 and 17.
4. **Loss or unbounded duplication on restart** of a native task, a split reassignment or a fallback pod. Expected: at-least-once, duplicates only within the declared delivery. Tests: Task 6 `kill-restart` and `dup-check` suites; Task 4 `split_reassignment_resumes_from_checkpoint`.
5. **Unbounded memory or a stalled pipeline under a slow sink or a fast source.** Expected: bounded by the instance's memory budget, the source paused, not dropped. Tests: Task 4 `pipeline_memory_bounded_under_slow_sink`, `source_respects_budget`; Task 6's `backpressure` suite.
6. **CDC resource leaks**: an abandoned replication slot filling the source's disk, a purged binlog or a trimmed oplog silently skipped. Expected: slot dropped on delete, lag alerted, a purged position fatal with a re-snapshot hint. Tests: Task 10 `pg_cdc_slot_dropped_on_delete`, `pg_cdc_idle_heartbeat_advances_slot`; Task 11 `binlog_purged_is_fatal_with_resnapshot_hint`; Task 16 `mongodb_oplog_window_metric`.
7. **Schema drift silently coerces data.** Expected: additive changes evolve, incompatible ones pause with an error. Tests: Task 5 `incompatible_change_pauses`, `lossy_mapping_refused`.
8. **A secret leaks** through a config, a checkpoint, a `Position::display`, a profile, a fallback's own logs, a Kubernetes object or an error. Expected: never. Tests: Task 4 `no_secret_in_position_or_error`; Task 7 `profile_secrets_by_reference_only`; Task 25 `no_secret_in_fallback_logs`, `no_secret_in_rendered_kube_objects`.
9. **SSRF**: an instance reaching the metadata endpoint, loopback, the Kubernetes API or another tenant through an HTTP profile, a hostname or a redirect. Expected: refused at validation and at connect time, after DNS. Tests: Task 25 `egress_guard_refuses_metadata_loopback_and_cluster`, `egress_guard_rechecks_after_dns`, `redirect_rechecked`.
10. **CloudEvents already produced upstream lose their identity.** Expected: pass-through keeps `type`, `source` and `id` (§33 §6). Tests: Task 9 `redpanda_ce_headers_pass_through`; Task 19 `eventgrid_cloudevents_pass_through`, `pubsub_ce_attributes_pass_through`.
11. **Loams-written Java, or a CI-only or user-supplied component, ships.** Expected: CI fails. Tests: Task 22 `no_java_sources_in_tree`; Task 3 `no_ci_only_or_user_supplied_in_images`.
12. **Two supervisors drive the same instance or split** after a restart or a reschedule. Expected: the stale holder is fenced, and its checkpoint write is refused. Tests: Task 4 `checkpoint_save_fenced_by_epoch`; Task 26 `two_flow_processes_one_instance_runner`.
13. **Billing creeps in** through the activity observer. Expected: plain structs only. Tests: Task 24 `activity_observer_has_no_wire_format`, `no_metering_guard_green`.

---

## File structure

```
connectors/schema/connector.schema.json                    Task 1 (runtime per direction, fallback, profile, sink.reply, notes, effectively_once, new suites)
connectors/schema/profile.schema.json                      Task 7
fabric/proto/loams/flow/v1/connector.proto                  Task 1 (mirrors the schema)
connectors/registry/catalog.csv                             Task 2 (columns source_runtime, sink_runtime, profile, fallback)
connectors/registry/<id>.yaml                               Task 2; Tasks 8–23 (detailed, status preview)
connectors/registry/handwritten.txt                         Tasks 2, 8–23
connectors/schemas/<id>.config.json                         Tasks 8–23
connectors/profiles/<id>.yaml                               Tasks 7, 9–21 (target and http-api profiles)
connectors/licences.toml                                    Tasks 0, 3
connectors/vendor/camel-catalog-4.22.x.json.zst             Task 22 (fallback rows only)
docs/design/33-connectors.md                                Task 2 (Appendix A cells), Task 30 (status)
scripts/connectors/{profiles.py,image_licences.py,crate_licences.py,no_java.py,camel_manifest.py}   Tasks 3, 7, 22
fabric/crates/loams-connector-sdk/src/
  lib.rs source.rs sink.rs txn.rs split.rs position.rs checkpoint.rs delivery.rs
  pipeline.rs budget.rs retry.rs ratelimit.rs error.rs envelope.rs fabric.rs
  secrets.rs egress.rs observe.rs                           Task 4
  schema/{mod.rs,arrow.rs,evolution.rs,registry.rs}         Task 5
  codecs/{csv.rs,ndjson.rs,protobuf.rs}                     Task 5
  testing/{memory.rs,reference.rs}                          Task 4 (in-memory Fabric and a reference connector)
fabric/crates/loams-flow-conformance/src/
  suites/{contract,roundtrip,kill,dup,bulk,cdc,schema,backpressure,exactly_once,auth}.rs
  services.rs fixture.rs crash.rs                           Task 6
fabric/crates/loams-connector-http/          HTTP ★, webhooks ★, the HTTP-API profile engine     Tasks 7, 8, 20
fabric/crates/loams-connector-kafka/         Kafka ★, Redpanda, Event Hubs                      Tasks 8, 9
fabric/crates/loams-connector-postgres/      Postgres ★ batch, sink and CDC; CockroachDB, Aurora, pgvector   Tasks 8, 10
fabric/crates/loams-connector-mysql/         MySQL ★ batch, sink and CDC; MariaDB, Aurora      Tasks 8, 11
fabric/crates/loams-connector-sqlserver/     SQL Server, Azure SQL                              Task 12
fabric/crates/loams-connector-objects/       S3 ★, R2, MinIO/RustFS, GCS, Azure Blob, ADLS, SFTP Tasks 8, 13
fabric/crates/loams-connector-kv/            Redis ★, Valkey, Redis Streams, DynamoDB(+Streams), Cassandra   Tasks 8, 14
fabric/crates/loams-connector-search/        Elasticsearch ★, OpenSearch, Qdrant                Task 15
fabric/crates/loams-connector-mongodb/       MongoDB                                             Task 16
fabric/crates/loams-connector-analytics/     ClickHouse ★, Redshift, Databricks, Trino, DuckDB  Tasks 8, 17
fabric/crates/loams-connector-lakehouse/     Iceberg ★ (CN1's source; native raw-topic sink), Delta Lake   Tasks 8, 17
fabric/crates/loams-connector-brokers/       RabbitMQ, AMQP 1.0 (Artemis, Service Bus), NATS, MQTT, Pulsar, WebSocket   Task 18
fabric/crates/loams-connector-cloud/         Kinesis ★, SQS, SNS, EventBridge, Pub/Sub, Event Grid   Tasks 8, 19
fabric/crates/loams-connector-protocols/     gRPC, GraphQL, Neo4j, Grafeo, Prometheus, functions, OpenAI, Anthropic, OTLP ★   Tasks 8, 21
fabric/crates/loams-connector-iggy-adapter/  SDK connector → Iggy plugin wrapper               Task 29
fabric/crates/loams-flow/src/
  manifest.rs validate.rs registry.rs profile.rs            Tasks 1, 7
  runtime/native.rs                                         Task 4 (drives SDK connectors)
  runtime/{camel.rs,debezium.rs,iggy.rs}                    Task 22 (fallbacks; extend CN1 Task 3's)
  runtime/kube/{mod.rs,native.rs,connect.rs,debezium.rs,iggy.rs,lease.rs}   Task 26
  fabric_io.rs                                              Task 4 (FabricWriter/FabricReader over ingest and Iggy)
  activity.rs metrics.rs egress.rs secrets/{file.rs,kube.rs}   Tasks 24, 25
connect/routes/{templates,overlays}/                        Task 22 (fallback rows only)
deploy/fabric/connectors/debezium/{oracle,sqlserver,mongodb,mariadb}.properties.tmpl   Task 22
deploy/fabric/compose.yaml (profiles connectors-p2, connectors-ci-only)   Task 6
deploy/helm/loams-connectors/                               Task 26
deploy/observability/connectors/{dashboards/,alerts.yaml}   Task 24
conformance/connectors/fixtures/<id>/                       Tasks 6, 13, 17, 19, 20, 21 (recorded from the real system)
docs/security/connectors-threat-model.md                    Task 25
docs/runbooks/connectors/                                   Task 30
docs/guides/connectors/<id>.md, index.md (generated)        Tasks 8–23, 30
docs/plans/cn2-dependency-spike.md  docs/plans/cn2-fallback-queue.md  docs/plans/cn2-exit-report.md   Tasks 0, 23, 30
.github/workflows/fabric.yml (sdk-unit, connectors-p2, connectors-ci-only, connectors-nightly, connect-routes, connectors-kind)
```

## Shared contracts (all tasks use these names)

### Manifest additions (Task 1 writes them; YAML is the source, and the proto mirrors it one to one)

```yaml
runtime:
  kind: native                    # default for both directions
  ref: loams_connector_postgres::Postgres
  version: "loams-connector-postgres 0.1"
  source:                         # optional: overrides kind/ref for the source direction
    kind: native
    ref: loams_connector_postgres::cdc::PgCdc
  sink:                           # optional: overrides kind/ref for the sink direction
    kind: native
    ref: loams_connector_postgres::sink::PgSink
  profile: null                   # optional: a profile id under connectors/profiles/ (target or http-api)
  fallback:                       # optional (OR-4); never the default; an instance opts in with `runtime: fallback`
    kind: debezium                # camel | debezium | iggy
    ref: io.debezium.connector.postgresql.PostgresConnector
    directions: [source]
    replaced_by: "CN2 Task 10"    # the task that made it a fallback, or "CN2 Task 23" for the replacement queue
capabilities:
  sink:
    reply: false                  # the sink emits one reply event per input event to a declared reply topic
  delivery: { source: effectively_once, sink: exactly_once }   # enum gains effectively_once (OR-7)
conformance: [contract, roundtrip, kill-restart, dup-check, cdc, schema-evolution, backpressure, exactly-once, auth]   # slugs gain the last four
notes: []                         # free-form; prefixes: "narrows: …", "fallback-only: …", "java-deferred: …"
```

Semantic rules added to `validate::semantic_rules`:
- R-CN2-1: a direction override names a direction the manifest declares.
- R-CN2-2: `profile` is set only when the direction's kind is `native`; the profile exists and validates; a `target` profile's `module` is a family module, and an `http-api` profile's `module` is `http`.
- R-CN2-3: `sink.reply` implies `envelope.emits` is non-empty and the config schema has `reply_topic`.
- R-CN2-4: a `camel` direction or fallback names a component in the vendored catalog snapshot with the matching consumer or producer (Task 22).
- R-CN2-5: `status: preview` or `stable` implies a `conformance` that includes `contract`.
- R-CN2-6 (OR-1): for a P1 or P2 manifest, each declared direction's default runtime is `native`, unless `notes` has a `fallback-only:` entry naming one of OR-4's reasons.
- R-CN2-7 (OR-7): `delivery.sink: exactly_once` requires `sink.transactional: true` and `exactly-once` in `conformance`; `effectively_once` requires `sink.idempotent` or `sink.upsert`, or (for a source) `envelope.stable_ids: true`.
- R-CN2-8: `runtime.fallback.kind` is `camel`, `debezium` or `iggy`, its `directions` are declared, and `replaced_by` is non-empty.

`proto_and_yaml_agree` covers the new fields. The bump is `specVersion` minor for every manifest that gains a field; a manifest whose default runtime changes (OR-8) bumps minor too, because no capability narrows (D353 rule 4); a narrowed capability bumps major.

### Profiles (Task 7)

```yaml
# connectors/profiles/<id>.yaml
apiVersion: loams.flow/v1
kind: Profile
id: redpanda
profile_kind: target              # target | http-api
module: kafka                     # the family module ("http" for http-api)
system:                           # the real system the suite runs against (OR-2)
  name: Redpanda
  service: { compose: redpanda }  # or { fixture: conformance/connectors/fixtures/redpanda } or { nightly: [LOAMS_REDPANDA_BROKERS] }
defaults: {}                      # instance-config defaults merged under the user's config
narrows: []                       # capability paths the target does NOT have, each with a notes entry
# http-api profiles only:
http:
  base_url: "https://api.hubspot.com"
  auth: { scheme: oauth2 | bearer | basic | header | aws-sigv4, header: "Authorization" }
  resources:
    contacts:
      list: { method: GET, path: "/crm/v3/objects/contacts", items: "/results", cursor: { next: "/paging/next/after", param: "after" } }
      upsert: { method: POST, path: "/crm/v3/objects/contacts/batch/upsert", batch: 100, key: "/id" }
      async_job: null             # Bulk-style APIs: { create, poll, results } (Salesforce Bulk 2.0, Redshift-style statements)
  rate_limit: { requests: 100, per_seconds: 10, honour_headers: ["Retry-After", "X-HubSpot-RateLimit-Remaining"] }
  webhook: { scheme: hmac-sha256, header: "X-HubSpot-Signature-v3" }   # handled by the webhooks source
```

```rust
pub struct Profile { pub id: String, pub kind: ProfileKind, pub module: String, pub system: SystemRef,
                     pub defaults: serde_json::Value, pub narrows: Vec<String>, pub http: Option<HttpProfile> }
pub fn load_profiles(dir: &Path) -> Result<BTreeMap<String, Profile>, ProfileError>;
pub fn effective_capabilities(spec: &ConnectorSpec, p: Option<&Profile>) -> Capabilities; // spec minus `narrows`
```

### The connector SDK (Task 4; `loams-connector-sdk`)

```rust
// Splits: the unit of parallelism and of checkpointing (a Kafka partition, a Kinesis shard, a key range,
// a replication slot, an object prefix, a DynamoDB scan segment). One split is read by one task at a time.
pub struct Split { pub id: SplitId, pub spec: serde_json::Value }
#[async_trait] pub trait SplitEnumerator: Send {
    async fn discover(&mut self, known: &[SplitState]) -> Result<Vec<Split>, ConnectorError>;   // re-run on a timer and on demand
}

// A source reads one split. CN1 Task 3's method names are kept.
#[async_trait] pub trait Source: Send {
    async fn open(&mut self, split: &Split, pos: Option<Position>) -> Result<(), ConnectorError>;
    async fn next(&mut self, budget: Budget) -> Result<Option<SourceBatch>, ConnectorError>;   // never exceeds budget
    async fn committed(&mut self, upto: &Position) -> Result<(), ConnectorError>; // after the Fabric acks: commit upstream
    async fn close(&mut self) -> Result<(), ConnectorError>;                       // (Kafka offsets, PG flush LSN, XACK, SQS delete…)
}
pub struct SourceBatch { pub events: EventBatch, pub position: Position }
pub enum EventBatch { Events(Vec<CloudEvent>), Arrow { batch: RecordBatch, envelope: CloudEvent } }   // D356 bulk path

#[async_trait] pub trait Sink: Send {
    async fn write(&mut self, batch: EventBatch) -> Result<SinkAck, ConnectorError>;
    async fn flush(&mut self) -> Result<SinkAck, ConnectorError>;
}
// Exactly-once where the target stores Loams' checkpoint atomically with the data (OR-7).
#[async_trait] pub trait TransactionalSink: Sink {
    async fn recover(&mut self) -> Result<Option<Checkpoint>, ConnectorError>;           // the last checkpoint the target committed
    async fn prepare(&mut self, cp: &Checkpoint) -> Result<TxnToken, ConnectorError>;    // data + checkpoint staged
    async fn commit(&mut self, t: TxnToken) -> Result<(), ConnectorError>;                // idempotent; retried after a crash
    async fn abort(&mut self, t: TxnToken) -> Result<(), ConnectorError>;
}

pub struct Position { pub opaque: bytes::Bytes, pub display: String }   // display is for humans and never holds a secret
pub struct Checkpoint { pub instance: InstanceKey, pub epoch: u64, pub splits: BTreeMap<SplitId, Position> }
#[async_trait] pub trait CheckpointStore: Send + Sync {                  // default: _fabric.flow_positions (CN1) + an epoch column
    async fn load(&self, i: &InstanceKey) -> Result<Option<Checkpoint>, CheckpointError>;
    async fn save(&self, cp: &Checkpoint) -> Result<(), CheckpointError>; // Err(Fenced) when cp.epoch < the stored epoch
}

pub enum Delivery { AtMostOnce, AtLeastOnce, EffectivelyOnce, ExactlyOnce }
pub enum ErrorClass { Retryable { after: Option<Duration> }, Throttled { after: Duration }, Poison /* → DLQ */,
                      Auth, SchemaIncompatible, PositionLost /* purged binlog, trimmed shard, dropped slot */, Fatal }
pub struct ConnectorError { pub class: ErrorClass, pub message: String /* no secrets */, pub source: Option<BoxError> }

// The Fabric seen from a connector (implemented over ingest and Iggy by loams-flow, in memory by testing::memory).
#[async_trait] pub trait FabricWriter: Send + Sync { async fn append(&self, b: &SourceBatch) -> Result<FabricAck, FabricError>; }
#[async_trait] pub trait FabricReader: Send { async fn poll(&mut self, budget: Budget) -> Result<Option<(EventBatch, Position)>, FabricError>;
                                              async fn commit(&mut self, upto: &Position) -> Result<(), FabricError>; }

pub struct Budget { pub max_rows: usize, pub max_bytes: usize }
pub struct PipelineConfig {                   // defaults; per-instance overrides in config `pipeline.*`
    pub max_batch_rows: usize,                // 65_536 (D356)
    pub max_batch_bytes: usize,               // 8 MiB
    pub linger: Duration,                     // 200 ms
    pub max_in_flight_batches: usize,         // 4
    pub memory_budget_bytes: usize,           // 64 MiB per instance
    pub retry: RetryPolicy,                   // 100 ms → 30 s exponential, full jitter, 10 attempts, honours Retry-After
    pub checkpoint_interval: Duration,        // 5 s, or every acknowledged batch for transactional sinks
}
pub struct Pipeline;                          // drives SplitEnumerator + Sources → FabricWriter, and FabricReader → Sink
pub struct EnvelopeBuilder;                   // §33 §6 ids, extensions loamsconnector/loamsinstance/loamsop/loamslsn, pass-through
pub trait ConnectorFactory: Send + Sync {     // one per manifest id, registered by each family crate
    fn id(&self) -> &'static str;
    fn source(&self, ctx: &ConnectorContext) -> Result<Option<(Box<dyn SplitEnumerator>, Box<dyn SourceFactory>)>, ConnectorError>;
    fn sink(&self, ctx: &ConnectorContext) -> Result<Option<Box<dyn SinkFactory>>, ConnectorError>;
}
pub struct ConnectorContext { pub instance: InstanceKey, pub config: serde_json::Value, pub secrets: Arc<dyn SecretResolver>,
                              pub egress: Arc<dyn EgressCheck>, pub observe: Arc<dyn ConnectorObserver>, pub profile: Option<Profile> }
```

Delivery mechanics, the same in every family:
- **Source:** read → `FabricWriter::append` → on `FabricAck`, `Source::committed` (upstream) and `CheckpointStore::save` (fenced). A crash between the two re-reads from the last committed position; `ingest` dedups by `(source, id)`.
- **Sink, at-least-once or effectively-once:** `FabricReader::poll` → `Sink::write` → `Sink::flush` → `FabricReader::commit`.
- **Sink, exactly-once:** on open, `recover()` returns the target's checkpoint, and the reader seeks to it; per batch, `prepare(checkpoint)` → `commit(token)` → `FabricReader::commit`. A crash after `prepare` aborts or commits by the token's recorded state; the Fabric offset is advisory.

### Schema mapping (Task 5)

```rust
pub trait TypeMapper: Send + Sync {                      // one per family: PG OIDs, MySQL column types, BSON, CQL, ClickHouse,
    fn to_arrow(&self, native: &NativeType) -> Result<arrow::datatypes::Field, SchemaError>;   // ES mappings, Delta, …
    fn from_arrow(&self, field: &arrow::datatypes::Field) -> Result<NativeType, SchemaError>;
}
pub enum Evolution { Additive /* default */, Widen, Pause }
pub struct SchemaChange { pub table: String, pub before: SchemaRef, pub after: SchemaRef, pub at: Position }
// Emitted as io.loams.dev.flow.<connector>.schema-change.v1 to <db>.schema_changes (§33 §7).
// A mapping that loses information (precision, timezone, unsigned overflow) is refused unless the instance opts into `lossy: [<path>]`.
```

### Conformance kit (Task 6; `loams-flow-conformance`)

```rust
pub enum Suite { Contract, Roundtrip, KillRestart, DupCheck, Bulk, Cdc, SchemaEvolution, Backpressure, ExactlyOnce, Auth }
pub enum ServiceKind {
    Container { compose_service: &'static str, system: &'static str },    // a real system's image (OR-2)
    Emulator  { compose_service: &'static str, system: &'static str, vendor: &'static str }, // the vendor's or a widely used emulator
    Fixture   { dir: &'static str, recorded_from: &'static str },         // recorded from the real system
    Nightly   { env: &'static [&'static str], system: &'static str },     // a real account
}
pub trait ConnectorHarness: Send + Sync {
    fn services(&self) -> &[ServiceKind];
    async fn seed(&self, n: usize) -> Result<Seeded, HarnessError>;         // write n records into the real system
    async fn read_back(&self) -> Result<Vec<Observed>, HarnessError>;      // read the real system's state
    async fn kill(&self, what: KillTarget) -> Result<(), HarnessError>;     // the task, the pod, or the system itself
    async fn inject(&self, at: FaultPoint) -> Result<(), HarnessError>;
}
pub enum FaultPoint { BeforeFabricAck, AfterFabricAckBeforeUpstreamCommit, AfterPrepare, AfterCommitBeforeFabricCommit, MidBatch }
pub async fn run(spec: &ConnectorSpec, profile: Option<&Profile>, h: &dyn ConnectorHarness, suites: &[Suite]) -> SuiteReport;
```

### Event names (extends CN1 Ruling 2; `io.loams.dev.flow.<connector-id>.<event>.v1`)

| Family | Events |
|---|---|
| Brokers (AMQP, AMQP 1.0, NATS, Pulsar, MQTT, SQS, SNS, Pub/Sub, Event Hubs, Service Bus, WebSocket, Kafka-protocol targets) | `message` (Kafka-protocol targets keep CN1's `record`) |
| EventBridge, Event Grid | pass-through when the input is a CloudEvent, else `event` |
| Relational and warehouse batch reads | `rows` (Arrow batches on the bulk path; JSON rows only from HTTP-API profiles) |
| CDC (Postgres, MySQL, MariaDB, SQL Server, MongoDB, CockroachDB, DynamoDB Streams; Debezium fallbacks) | `change` (with `loamsop`, `loamslsn`); `schema-change` |
| Documents (MongoDB, Cassandra, DynamoDB, Elasticsearch, OpenSearch, Qdrant) | `document` |
| Graph (Neo4j, Grafeo) | `rows` (GQL or Cypher result batches) |
| Objects (S3, R2, MinIO/RustFS, GCS, Azure Blob, ADLS, SFTP) | `object`, `object-deleted` |
| SaaS and HTTP-API profiles | `item` (polled), `delivery` (webhooks) |
| AI, functions, gRPC unary sinks | `reply` |
| Observability (Splunk, Prometheus) | `log`, `metric` |

### Activity observer (Task 24; Q359)

```rust
pub trait ConnectorActivityObserver: Send + Sync + 'static {
    fn on_activity(&self, a: &ConnectorActivity);          // plain struct, no wire format (D548)
}
pub struct ConnectorActivity { pub namespace: u64, pub instance: String, pub connector: String,
    pub runtime: RuntimeKind, pub fallback: bool, pub direction: Direction, pub events: u64, pub bytes_in: u64,
    pub bytes_out: u64, pub api_calls: u64, pub window_end: SystemTime }
```

The SDK's `ConnectorObserver` feeds it for native connectors; the fallback runtimes' own metrics feed it for fallbacks. `loams-platform` links its own implementation. This repository persists nothing.

### Egress guard (Task 25)

```rust
pub struct EgressPolicy { pub allow_cidrs: Vec<IpNet>, pub deny_cidrs: Vec<IpNet>, pub allow_hosts: Vec<String> }
impl EgressPolicy {
    pub fn check_config(&self, spec: &ConnectorSpec, config: &serde_json::Value) -> Result<(), EgressError>; // hosts named in config
    pub fn check_addr(&self, addr: &SocketAddr) -> Result<(), EgressError>;                                  // after DNS, at connect
}
// Default deny: 127.0.0.0/8, ::1, 169.254.0.0/16, fe80::/10, fd00:ec2::254, the cluster's pod and service CIDRs,
// and the Loams control-plane Services, unless the namespace admin allowlists them.
// The SDK's EgressCheck wraps it; every family crate connects through the SDK's resolver hook.
```

### Fallback runtime contract (Task 22; OR-4 only)

- Camel route id `loams-<namespace>-<instance>-<source|sink>`, file `/etc/loams-connect/routes/<namespace>/<instance>.<direction>.yaml`, the generic `source.yaml.tmpl` and `sink.yaml.tmpl` with one overlay per fallback component (`connect/routes/overlays/<id>.yaml`), CloudEvents headers set by `setHeader` steps, `camel-iggy` at the Fabric end, the consumer offset committed only after the producer step returns (Task 0 verifies `camel-iggy`'s manual commit; without it the fallback sink declares `at_most_once`).
- Debezium Server: CN1 Task 8's model (properties rendered per instance, HTTP sink to `ingest`, offsets and history on a volume, Q350).
- Iggy plugins: CN1 Task 3's `IggyRuntime` TOML rendering.

---

## Execution order

1. Task 0.
2. **CN2a** (Tasks 1–3) at once: it needs only `loams-flow` as built and the registry scripts.
3. **CN2b** (Tasks 4–8). Tasks 4–6 start now (no FL1 dependency; the in-memory Fabric serves the tests). Task 4's `NativeRuntime` integration and `fabric_io.rs` wait for CN1 Task 3 and FL1's `ingest`. Task 7 needs Task 4. Task 8 ports each CN1 ★ module as it merges; for a CN1 task not yet started, CN1 builds that module directly in its family crate (Follow-ups).
4. **CN2c** (Tasks 9–21) after Tasks 4–7. Family tasks are independent and may run in parallel worktrees, one cargo build at a time on the build machine. A family whose ★ module is ported in Task 8 starts after that port.
5. **CN2d** (Tasks 22–23) after CN1 Tasks 8 and 12 (the Debezium and Camel runtimes). Task 23 also needs Task 24's counters.
6. **CN2e** (Tasks 24–30): Tasks 24–27 beside CN2c. Task 28 is gated on Q30's auth plan. Task 29 is gated on the owner's go-ahead for upstream PRs (§23 §2.2). Task 30 comes last.

---

### Task 0: Reconcile with the code as built, and the dependency spike

**Files:** `docs/plans/cn2-dependency-spike.md`; `connectors/licences.toml` (entries for every crate and image found); this plan's "Rulings made during execution".

Steps:
1. Answer each of the following and record the answer, with file paths or URLs, as a ruling:
   - Is the "As built" block above still true? Check `fabric/crates/`, `fabric/proto/loams/flow/v1/`, `connect/`, `deploy/fabric/` and FL1's and CN1's plan status lines. Name which CN1 and FL1 tasks have merged since `1dc6e8a3`, and therefore which ★ modules Task 8 ports and which CN1 builds directly on the SDK.
   - For every protocol crate in Tech Stack: version, licence (SPDX), release date, maintenance (last release, open security advisories), MSRV against Rust 1.97, and the build cost of its family crate alone (one measured build). For each P2 row, the protocol path it will use: the crate, the wire protocol, and the operations needed per declared capability. Output: a table in the spike document that Task 2 consumes.
   - The `pgoutput` client choice (OR-24): does the candidate support `CREATE_REPLICATION_SLOT … EXPORT_SNAPSHOT`, `START_REPLICATION` with proto version 1 and 2+ (streaming of in-progress transactions), and Standby Status Update with an explicit flush LSN?
   - `mysql_async`'s binlog stream: GTID positions for MySQL and MariaDB (MariaDB's domain-server-sequence form), `TABLE_MAP` and row event decoding in `mysql_common`, and the `binlog_row_metadata=FULL` column names.
   - Which real systems support what each family assumes: Redpanda's transactions and `ce_` header handling; the Event Hubs emulator's Kafka endpoint and Event Hubs' Kafka transaction support; the Service Bus emulator's AMQP 1.0 endpoint; ActiveMQ Classic and Artemis AMQP 1.0 acknowledgement modes; OpenSearch's point-in-time API; CockroachDB's core changefeeds and `COPY … TO STDOUT` binary support; R2's conditional `PUT` (`If-None-Match`); Azurite's hierarchical-namespace support; floci 2.1.0's coverage of SQS, SNS, EventBridge, Kinesis, DynamoDB, DynamoDB Streams, Lambda and SES. Where an emulator falls short, name the fixture or nightly fallback.
   - The licences and CI terms of every test image in Tech Stack (OR-3), each recorded as `ci-service` or as a plain service.
   - For the fallback rows only (OR-4): the Camel components' consumer and producer support in `camel-catalog` 4.22.x, `camel-iggy`'s commit behaviour, and Debezium Server 3.7.0.Final's Oracle connector settings (LogMiner, `ojdbc` placement).
   - Whether `loams-flow-proto` can generate a `loams.graph.v1` client from `proto/loams/graph/v1/graph.proto` across workspaces (OR-20).
2. Commit `docs(connectors): cn2 task 0 rulings and dependency spike`.

## CN2a — Registry truth (Tasks 1–3)

### Task 1: Manifest additions — per-direction runtimes, fallbacks, profiles, delivery and suites

**Files:** modify `connectors/schema/connector.schema.json`, `fabric/proto/loams/flow/v1/connector.proto`, and `fabric/crates/loams-flow/src/{manifest.rs,validate.rs,registry.rs}`. Tests go in `fabric/crates/loams-flow/tests/{validate.rs,registry.rs}`.

**Interfaces:**
- `RuntimeSpec` gains `source: Option<RuntimeOverride>`, `sink: Option<RuntimeOverride>`, `profile: Option<String>` and `fallback: Option<FallbackSpec { kind, reference, directions, replaced_by }>`, plus `RuntimeSpec::for_direction(Direction, Choice::{Default, Fallback}) -> Option<(RuntimeKind, &str)>`.
- `Delivery` gains `EffectivelyOnce`; the `conformance` enum gains `schema-evolution`, `backpressure`, `exactly-once` and `auth`; `SinkCaps.reply: bool` (serde default `false`); `EnvelopeSpec.stable_ids: bool`; `ConnectorSpec.notes: Vec<String>`.
- Rules R-CN2-1 to R-CN2-8 in `semantic_rules`. R-CN2-4 reads Task 22's snapshot; until Task 22 lands it is a no-op behind a `cfg` guard and a `TODO(CN2 Task 22)`. R-CN2-6 is a warning until Task 2 has assigned the runtimes, then an error.
- `Filter` gains `runtime_for_direction` and `has_fallback`.

Tests:
- `override_direction_must_be_declared`
- `profile_requires_native_direction`
- `reply_requires_reply_topic`
- `preview_requires_contract_suite`
- `p1_p2_default_runtime_is_native_or_explained`
- `exactly_once_requires_transactional_sink`
- `effectively_once_requires_dedup_or_stable_ids`
- `fallback_needs_replaced_by`
- `for_direction_falls_back_only_when_asked`
- `proto_and_yaml_agree` (extended to the new fields)
- `existing_204_manifests_still_validate`

Steps: tests (FAIL) → schema → proto → Rust → PASS → `gen_registry.py --check` still green → commit `flow: per-direction runtimes, fallbacks, profiles, effectively-once delivery and new suites in manifests`.

### Task 2: The native-first runtime assignment

**Files:** `connectors/registry/catalog.csv` (adds the columns `source_runtime`, `sink_runtime`, `profile`, `fallback`; `gen_registry.py` reads them), `scripts/connectors/{gen_registry.py,matrix.py}`, `connectors/registry/*.yaml` (regenerated), `docs/design/33-connectors.md` (Appendix A legend, cells, the A.3 Neo4j note and the totals line), and `fabric/crates/loams-flow/tests/registry.rs`.

**Semantics.** Every P1 and P2 row gets `native` in each declared direction except where this table says otherwise (OR-1, OR-4). Task 0's protocol table settles each `ref`.

| Rows | Default runtime and family | Fallback (OR-4) | Task |
|---|---|---|---|
| Kafka ★, Redpanda, Azure Event Hubs | native `kafka`; Redpanda and Event Hubs as target profiles | none | 9 |
| PostgreSQL ★ (gains `cdc`), Aurora, CockroachDB, pgvector | native `postgres`; targets `aurora-postgres`, `cockroachdb`, `pgvector` (Aurora's engine chosen by config `engine`) | `debezium-postgres` (opt-in); Iggy `postgres_sink`, `postgres_source` | 10 |
| MySQL ★ (gains `cdc`), MariaDB, Aurora (MySQL engine) | native `mysql`; targets `mariadb`, `aurora-mysql` | `debezium-mysql` (opt-in); Debezium MariaDB (opt-in) | 11 |
| SQL Server, Azure SQL | native `sqlserver`; target `azure-sql` | `debezium-sqlserver` (opt-in) | 12 |
| S3 ★ (sink becomes native), Cloudflare R2, MinIO / RustFS, GCS, Azure Blob, ADLS Gen2, SFTP | native `objects`; targets `cloudflare-r2`, `minio-rustfs` | Iggy `s3_sink` | 13 |
| Redis ★, Valkey, Redis Streams, DynamoDB, DynamoDB Streams, Cassandra | native `kv`; target `valkey` | none | 14 |
| Elasticsearch ★ (becomes native), OpenSearch, Qdrant | native `search`; target `opensearch` | Iggy `elasticsearch_sink`, `elasticsearch_source` | 15 |
| MongoDB | native `mongodb` (batch, CDC, sink) | `debezium-mongodb` (opt-in); Iggy `mongodb_sink` | 16 |
| ClickHouse ★ (sink becomes native), Redshift, Databricks, Trino (source only), DuckDB | native `analytics`; DuckDB as an ADBC target profile | Iggy `clickhouse_sink` | 17 |
| Iceberg ★ (raw-topic sink becomes native), Delta Lake (both directions, OR-18) | native `lakehouse` | Iggy `iceberg_sink` | 17 |
| RabbitMQ / AMQP, ActiveMQ / JMS, NATS, Pulsar, MQTT, Azure Service Bus, WebSocket | native `brokers` (AMQP 0-9-1, AMQP 1.0, NATS, MQTT 5, Pulsar, WebSocket) | Camel `jms` for JMS providers without AMQP 1.0 (`fallback-only` per instance) | 18 |
| Kinesis ★, AWS SQS, AWS SNS, EventBridge, Google Pub/Sub, Azure Event Grid | native `cloud` | none | 19 |
| Kestra companion, Segment, SendGrid, PostHog, HubSpot, Shopify, GitLab, Amazon SES, Twilio SMS, Salesforce, Zendesk, ServiceNow, Stripe, GitHub, Jira, Slack, Google Workspace, Keycloak, Vault, Splunk | native `http` + an `http-api` profile each (OR-11) | Camel `salesforce` for Salesforce CDC only (Task 23 replaces it) | 20 |
| gRPC, GraphQL, Neo4j, Grafeo, Prometheus, Lambda / Cloud Run / Azure Functions, OpenAI, Anthropic | native `protocols` | none | 21 |
| CSV / NDJSON, Protobuf | native SDK codecs | none | 5 |
| Oracle | `camel` `sql` with `user-supplied` `ojdbc`, `notes: ["fallback-only: Oracle client is proprietary (OR-4)"]` | — | 22, 23 |
| Debezium-SQL Server / Oracle | **split** (OR-13): `debezium-sqlserver` (fallback for Task 12) and `debezium-oracle` (Oracle's default CDC, `fallback-only`) | — | 22 |
| Debezium-Postgres ★, Debezium-MySQL ★, Debezium-MongoDB | stay `debezium`, marked fallbacks of the native CDC (`replaced_by` Tasks 10, 11, 16) | — | 22 |
| JDBC ★ | stays `camel` (`fallback-only: generic JDBC is a Java API`) | — | 22 |
| MCP | `priority: P3` (OR-15) | — | — |

Totals after the split and the MCP move: 205 manifests, 21 ★, 73 P2 rows. Appendix A's legend for P2 changes from "stock Camel or Iggy plugins" to "first-party native, with declared fallbacks", the Debezium row becomes two rows, the A.3 Neo4j note says HTTP Query API (OR-14), and `matrix.py --check` keeps the appendix and the CSV identical.

Tests:
- `registry_has_205_entries_and_21_starred`
- `p2_rows_are_native_or_fallback_only_explained`
- `p2_has_no_openapi_runtime`
- `every_target_names_an_existing_family_and_profile`
- `every_fallback_has_replaced_by`
- `debezium_rows_run_on_debezium_server`
- `appendix_matches_csv`

Commit `connectors: native-first runtime assignment, fallbacks, the Debezium split and target profiles`.

### Task 3: The licence gate for crates, CI-only services and user-supplied drivers

**Files:** `connectors/licences.toml`, `fabric/crates/loams-flow/src/validate.rs` (`LicenceGate`), `scripts/connectors/{image_licences.py,crate_licences.py}`, and `.github/workflows/fabric.yml`.

**Semantics:**
- Two new component kinds: `ci-service` (image, digest, SPDX id or `LicenseRef-<vendor>-EULA`, reason; allowed only in compose profiles whose name starts with `connectors` other than the default, and in CI workflow files) and `user-supplied` (driver, SPDX id or `LicenseRef-*`, mount path; allowed only as a documented mount, for example Oracle's `ojdbc` and Instant Client).
- `crate_licences.py` reads `cargo metadata` for each `loams-connector-*` crate and refuses any dependency whose licence is denied or missing (D359), so a family crate cannot pull in a denied crate.
- `image_licences.py` lists each image Loams builds (`loams-fabric`, `loams-connect`, any Debezium wrapper) and the default compose profile, and fails on any `ci-service` or `user-supplied` component. A `ci-service` may carry a denied id (the D60 precedent, OR-3).

Tests:
- `ci_service_allowed_only_in_ci_profiles`
- `user_supplied_never_in_image`
- `no_ci_only_or_user_supplied_in_images`
- `family_crates_pass_licence_gate`
- `denied_ids_still_refused_for_shipped`
- `licence_gate_refuses_flagged` (CN1's test, unchanged)

Commit `connectors: licence gate for family crates, CI-only services and user-supplied drivers`.

## CN2b — The connector SDK and test kit (Tasks 4–8)

### Task 4: `loams-connector-sdk` — traits, splits, checkpoints, delivery and backpressure

**Files:** `fabric/crates/loams-connector-sdk/**` (Shared contracts), `fabric/Cargo.toml` (member), `fabric/crates/loams-flow/src/{runtime/native.rs,fabric_io.rs}`, `.github/workflows/fabric.yml` (job `sdk-unit`). Tests go in `fabric/crates/loams-connector-sdk/tests/{pipeline.rs,checkpoint.rs,delivery.rs,retry.rs,envelope.rs}` and `fabric/crates/loams-flow/tests/native_runtime.rs`.

**Semantics:**
- The traits, `Pipeline`, `PipelineConfig`, `CheckpointStore`, `EnvelopeBuilder` and `ConnectorFactory` exactly as in Shared contracts. `Source` and `Sink` keep CN1 Task 3's method names (OR-5); `loams-flow` re-exports them.
- **Splits.** The enumerator runs on the instance's lease holder; each split runs as one tokio task; a split's position is checkpointed separately; split loss (a partition removed) and split addition are handled without a restart; with FL3, splits are leased across `flow` processes (OR-16).
- **Backpressure.** The pipeline hands each source a `Budget` from the free space of `max_in_flight_batches` and `memory_budget_bytes`; a source never returns more; when the Fabric or a sink is slow, budgets shrink to zero and sources stop reading (they do not buffer). Push sources (webhooks, MQTT, WebSocket) apply backpressure by their protocol (TCP window, 429 with `Retry-After`, QoS flow control) and declare it.
- **Retries** by `ErrorClass`: `Retryable` and `Throttled` back off per `RetryPolicy` and honour `Retry-After`; `Poison` sends the event to the instance's DLQ topic with the error; `Auth` and `Fatal` stop the instance with `last_error`; `PositionLost` stops it with a re-snapshot hint; `SchemaIncompatible` pauses (Task 5).
- **Rate limiting.** A token bucket per instance (requests and bytes), shared by its splits, adjusted by `Throttled`.
- **Envelope.** `EnvelopeBuilder` derives ids per §33 §6, sets `loamsconnector` and `loamsinstance`, and passes through valid CloudEvents keeping `type`, `source` and `id`.
- **Secrets and egress.** `SecretResolver` returns `Secret` (CN1's zeroizing type); `EgressCheck` is the hook Task 25 fills; every family crate connects through `sdk::net::connect` and `sdk::http::client`, which call it after DNS.
- **`NativeRuntime`** (CN1 Task 3's) runs any registered `ConnectorFactory`; `fabric_io.rs` implements `FabricWriter` over `ingest`'s in-process core and `FabricReader` over an Iggy consumer group `flow-<instance>`.
- `testing::memory` provides an in-memory Fabric and checkpoint store; `testing::reference` is a reference source and sink (a counter source and a map sink with transactional mode) used by the SDK's tests and by Task 6's self-test.

Tests:
- `source_respects_budget`
- `pipeline_memory_bounded_under_slow_sink`
- `upstream_commit_only_after_fabric_ack`
- `checkpoint_save_fenced_by_epoch`
- `split_reassignment_resumes_from_checkpoint`
- `split_added_and_removed_without_restart`
- `transactional_sink_recovers_and_skips_committed`
- `retry_classes_behave` (one case per `ErrorClass`)
- `poison_goes_to_dlq_with_error`
- `rate_limit_honours_retry_after`
- `envelope_ids_stable_across_reread`
- `passthrough_keeps_type_source_id`
- `no_secret_in_position_or_error`
- `native_runtime_runs_reference_connector` (in `loams-flow`)

Commit `sdk: loams-connector-sdk with splits, fenced checkpoints, delivery modes and backpressure`.

### Task 5: Schema mapping and codecs

**Files:** `fabric/crates/loams-connector-sdk/src/{schema/**,codecs/**}`, `connectors/registry/{csv-ndjson,protobuf}.yaml`, tests in `fabric/crates/loams-connector-sdk/tests/{schema.rs,codecs.rs}`, guides.

**Semantics:**
- `TypeMapper`, `Evolution` and `SchemaChange` as in Shared contracts. Arrow is the canonical schema (D356); every family supplies a mapper and a table of its types (in its guide).
- Evolution: `Additive` accepts new nullable columns and new tables; `Widen` also accepts lossless widenings (int32 → int64, varchar(n) → varchar(m > n), decimal precision up); anything else is `SchemaIncompatible` and pauses the instance with the before and after schemas in `last_error`. Schema changes are emitted as `schema-change` events (§33 §7).
- Lossy mappings (unsigned 64-bit to signed, timestamps without timezone, decimals over 38 digits) are refused unless the instance lists the path in `lossy`.
- Schema registry hooks: Avro and Protobuf with Confluent framing at Kafka boundaries (D356; CN1's registry client stub until Q344).
- Codecs: `arrow-csv` and `arrow-json` 59 for CSV and NDJSON, decoding straight to `RecordBatch`; `prost-reflect` over a user-supplied `FileDescriptorSet` for Protobuf.

Tests:
- `additive_change_evolves`
- `widen_accepted_only_when_configured`
- `incompatible_change_pauses`
- `lossy_mapping_refused`
- `schema_change_event_emitted`
- `csv_ndjson_roundtrip_types`
- `protobuf_descriptor_roundtrip`
- `bulk_path_has_no_row_decode` (CSV and NDJSON)

Commit `sdk: schema mapping, evolution policy, and CSV, NDJSON and Protobuf codecs`.

### Task 6: The conformance kit and the real-system test estate

**Files:** `fabric/crates/loams-flow-conformance/src/**` (Shared contracts), `deploy/fabric/compose.yaml` (profiles `connectors-p2` for open images and `connectors-ci-only` for `ci-service` images, every image pinned by digest), `conformance/connectors/fixtures/README.md`, `conformance/connectors/systems.toml` (each manifest id → the real system and its `ServiceKind`s), and `.github/workflows/fabric.yml` (jobs `connectors-p2` and `connectors-ci-only`, path-filtered per family crate and sharded by family; `connectors-nightly`, scheduled, with secrets).

**Semantics:**
- The suites of `Suite`: `contract` exercises each declared capability; `roundtrip`, `kill-restart`, `dup-check`, `bulk` and `cdc` as §33 §4 rule 3; `schema-evolution` applies an additive and an incompatible change; `backpressure` throttles the target and the Fabric and checks bounded memory and no loss; `exactly-once` crashes at every `FaultPoint` and checks the target equals the input exactly; `auth` runs each declared auth method that the service supports.
- `FixtureProxy`: record-and-replay HTTP for cloud and SaaS systems. Fixtures are recorded only from the real system (`recorded_from` names it); secrets are scrubbed by header and JSON-pointer rules; replay matches method, path and a canonical body and fails on an unmatched request.
- `systems.toml` is the OR-2 truth: for each manifest, the system name and its allowed services. Loams endpoints (collections' ES and Qdrant APIs, House, Loams Postgres) may appear only under `extra_targets`.
- `every_declared_capability_has_a_test` covers every `preview` manifest.

Tests:
- `suite_service_is_the_real_system`
- `target_suite_runs_against_target_system`
- `fixture_replay_rejects_unrecorded_request`
- `fixture_recording_scrubs_secrets`
- `crash_at_every_fault_point` (against `testing::reference`'s transactional sink)
- `skipped_suite_never_counts_as_green`
- `every_p2_preview_manifest_has_a_harness`
- `every_declared_capability_has_a_test`

Commit `ci: conformance suites and the real-system test estate for connectors`.

### Task 7: Profiles — targets and HTTP-API profiles

**Files:** `fabric/crates/loams-flow/src/profile.rs`, `connectors/schema/profile.schema.json`, `connectors/profiles/README.md`, `scripts/connectors/profiles.py`, `fabric/crates/loams-connector-http/src/profile/**` (the HTTP-API engine). Tests go in `fabric/crates/loams-flow/tests/profiles.rs` and `fabric/crates/loams-connector-http/tests/profile_engine.rs`.

**Semantics:**
- `Profile`, `load_profiles` and `effective_capabilities` as in Shared contracts. A target profile names its family module, its real system and its `narrows`; a narrowing found by a suite is added with a `narrows:` note, never silently skipped.
- The HTTP-API engine is a `ConnectorFactory` over a profile: `list` resources become polling sources (cursor, items pointer, `Link` headers, `Retry-After`; CN1 Task 4's polling engine), `upsert` and `create` resources become sinks (batches up to `batch`, `Idempotency-Key: <ce_id>` where accepted, else dedup by `upsert.key`, which makes them `effectively_once`), `async_job` resources run create → poll → fetch results, and `webhook` routes to the webhooks source with the named scheme. Auth schemes: `oauth2` (client credentials and refresh token), `bearer`, `basic`, `header` and `aws-sigv4`.

Tests:
- `target_inherits_family_minus_narrows`
- `http_profile_paginates_and_resumes`
- `http_profile_honours_rate_limit_headers`
- `http_profile_upsert_idempotent_on_retry`
- `http_profile_async_job_polls_to_completion`
- `aws_sigv4_signs_like_reference_vectors`
- `profile_secrets_by_reference_only`
- `unknown_profile_refused`

Commit `flow: target profiles and the declarative HTTP-API profile engine`.

### Task 8: The ★ native connectors on the SDK

**Files:** the family crates `loams-connector-{http,kafka,postgres,mysql,objects,kv,analytics,lakehouse,cloud,protocols}` (moving CN1's `fabric/crates/loams-flow/src/connectors/{http,webhook,kafka,postgres,mysql,s3,iceberg,redis,kinesis,clickhouse,adbc,otlp,zulip,itsplane,forgejo}.rs` into them as each exists), `fabric/crates/loams-flow/Cargo.toml` (features `conn-<family>`), and the ★ manifests' `runtime.ref`.

**Semantics:** each ★ native module of CN1 becomes a `ConnectorFactory` in its family crate on the SDK, keeping its behaviour and its CN1 tests (moved, not rewritten). CN1 modules not yet built are built there directly by CN1 (Follow-ups). ADBC and the Loams application sinks (Zulip, ItsPlane, Forgejo) live in `analytics` and `http` respectively. Nothing changes in the manifests but `runtime.ref` and `specVersion` minor.

Tests: every CN1 ★ test, green from its new crate; `star_manifests_point_at_family_crates`; `loams_flow_builds_with_no_conn_features` (dev build, no family linked).

Commit `connectors: move the starred native connectors onto the SDK, one crate per family`.

## CN2c — First-party native connectors (Tasks 9–21)

Every family task follows the same shape, and each connector is done when:
1. its manifest is detailed and in `handwritten.txt`, with a native default runtime (and its fallback block, if Task 2 gives one);
2. its family crate implements it on the SDK, with a `TypeMapper` where it carries rows;
3. its config schema marks secrets write-only;
4. its suites are green against the real system (OR-2) in PR CI, with `roundtrip` where both directions exist, `kill-restart` for every streaming source, `cdc` for CDC sources, `exactly-once` where declared, and `backpressure` for every streaming source;
5. it has a guide in `docs/guides/connectors/<id>.md` naming the real systems and versions tested, the narrowings and the fallback;
6. its `status` moves to `preview` (and to `stable` per OR-23 later).

Each task's tests list only what goes beyond those suites.

### Task 9: Kafka-protocol family — Kafka ★, Redpanda and Azure Event Hubs

**Files:** `fabric/crates/loams-connector-kafka/src/{source.rs,sink.rs,txn.rs}`, `connectors/profiles/{redpanda,azure-event-hubs}.yaml`, manifests `{kafka,redpanda,azure-event-hubs}.yaml`, tests `fabric/crates/loams-connector-kafka/tests/{kafka.rs,redpanda.rs,eventhubs.rs}`, guides.

**Semantics:**
- Splits are topic-partitions; the enumerator follows partition additions. The source commits consumer-group offsets after the Fabric acknowledges (CN1 Task 6).
- The sink keeps CN1's idempotent producer (`effectively_once`) and adds a transactional mode (`exactly_once`): `transactional.id = loams-<ns>-<instance>-<split>`, the checkpoint written to the compacted topic `__loams_checkpoints` (key = instance and split) inside the same Kafka transaction, and `recover()` reading it with `isolation.level=read_committed`.
- **Redpanda** runs the same code against real Redpanda, including transactions and D270's `ce_` header layout; anything that fails is narrowed with a note.
- **Event Hubs** runs through its Kafka endpoint (OR-22): SASL PLAIN with `$ConnectionString`, TLS on 9093, consumer groups as Event Hubs consumer groups. Transactions and compaction are narrowed if Task 0 confirms Event Hubs lacks them. Tested on the Event Hubs emulator (`ci-service`) and nightly on a real namespace.

Tests:
- `kafka_txn_sink_exactly_once_under_crash`
- `kafka_txn_recover_reads_checkpoint`
- `partition_added_is_discovered`
- `redpanda_suite_against_redpanda`
- `redpanda_ce_headers_pass_through`
- `eventhubs_kafka_endpoint_suite`
- `eventhubs_narrows_are_declared`

Commit `connectors: Kafka-protocol family with transactional sinks, Redpanda and Event Hubs`.

### Task 10: Postgres family — native CDC, exactly-once sink, Aurora, CockroachDB and pgvector

**Files:** `fabric/crates/loams-connector-postgres/src/{cdc.rs,snapshot.rs,slot.rs,sink.rs,types.rs}`, `connectors/profiles/{aurora-postgres,cockroachdb,pgvector}.yaml`, manifests `{postgresql,aurora,cockroachdb,pgvector}.yaml` (and `debezium-postgres.yaml` marked fallback), tests `fabric/crates/loams-connector-postgres/tests/{cdc.rs,sink.rs,targets.rs}`, guides, `docs/runbooks/connectors/postgres-cdc.md`.

**Semantics** (OR-9, §33 §7):
- **CDC source.** Publication `loams_<instance>` over the configured tables (or a user-named publication), slot `loams_<ns>_<instance>` with `pgoutput`. The slot is created at `CREATE_REPLICATION_SLOT … EXPORT_SNAPSHOT`; the initial snapshot reuses CN1's parallel `COPY` batch reader on that exported snapshot (`loamsop = r`, Arrow batches), and streaming starts at the slot's consistent point, so there is no gap and no overlap.
- Each row change is one `change` event with `loamsop`, `loamslsn` (commit LSN and ordinal) and `id` `"<slot>/<commit_lsn>/<ordinal>"`. Large transactions stream with protocol version 2+ where the server supports it.
- The position is the last commit LSN whose events the Fabric acknowledged; `committed()` sends a Standby Status Update with that flush LSN, so WAL is released only after the Fabric has the data.
- `Relation` messages drive Task 5's evolution. Unchanged TOAST values are marked, and the guide recommends `REPLICA IDENTITY FULL` where full rows are needed.
- An idle-database heartbeat (`pg_logical_emit_message`, configurable) keeps the slot advancing. The slot is dropped on `DeleteInstance` (not on `StopInstance`). Slot lag in bytes and seconds and `max_slot_wal_keep_size` headroom are metrics.
- **Sink.** CN1's upsert sink gains `exactly_once`: a `_loams_checkpoints` table (instance, split, position, epoch) in the target schema, written in the same transaction as each batch; `recover()` reads it.
- **Targets.** Aurora PostgreSQL (nightly real cluster, IAM tokens). CockroachDB: CDC through core changefeeds over the SQL connection (`EXPERIMENTAL CHANGEFEED FOR … WITH resolved, cursor = …`, the cursor as position), and a `SELECT` fallback batch path if `COPY … TO STDOUT` binary is missing (Task 0). pgvector: `vector` ↔ Arrow `FixedSizeList<Float32>`.
- Iggy `postgres_source` and `postgres_sink` and Debezium-Postgres become fallbacks (OR-4, OR-8).

Tests:
- `pg_cdc_converges` (a random workload of 10,000 operations; the Fluss `_current` table equals the source)
- `pg_cdc_snapshot_then_stream_no_gap_no_overlap`
- `pg_cdc_flush_lsn_only_after_fabric_ack`
- `pg_cdc_kill_restart_resumes`
- `pg_cdc_slot_dropped_on_delete`
- `pg_cdc_idle_heartbeat_advances_slot`
- `pg_cdc_ddl_add_column_evolves`
- `pg_sink_exactly_once_under_crash`
- `cockroach_changefeed_cdc_converges`
- `cockroach_batch_uses_select_fallback`
- `pgvector_roundtrip_fixed_size_list`
- `aurora_iam_token_nightly`

Commit `connectors: native Postgres CDC, an exactly-once Postgres sink, and Aurora, CockroachDB and pgvector`.

### Task 11: MySQL family — native binlog CDC, MariaDB and Aurora MySQL

**Files:** `fabric/crates/loams-connector-mysql/src/{cdc.rs,snapshot.rs,history.rs,sink.rs,types.rs}`, `connectors/profiles/{mariadb,aurora-mysql}.yaml`, manifests `{mysql,mariadb}.yaml` (and `debezium-mysql.yaml` marked fallback), tests `fabric/crates/loams-connector-mysql/tests/{cdc.rs,mariadb.rs,sink.rs}`, guides, `docs/runbooks/connectors/mysql-cdc.md`.

**Semantics:**
- **CDC source** from the row binlog (`binlog_format=ROW`, `binlog_row_image=FULL`; `binlog_row_metadata=FULL` where available), as a replica with a server id derived from the instance. The position is the GTID set (MySQL) or the MariaDB GTID list, with `file:pos` only when GTIDs are off.
- **Snapshot without global locks** by the DBLog watermark algorithm: chunked `SELECT`s by primary key, bracketed by low and high watermark rows written to a `_loams_watermarks` table (created by the runbook), with binlog events inside each window reconciled against the chunk. Where the user cannot grant write access, `snapshot.locking = global-read-lock` takes a brief `FLUSH TABLES WITH READ LOCK` (declared in the guide).
- **Schema history** keyed by position (DDL from `QUERY_EVENT`s parsed with `sqlparser`'s MySQL dialect, checked against `information_schema` at start) maps `TABLE_MAP` columns to names at each position, and is stored with the checkpoint.
- A purged binlog position is `PositionLost`: the instance stops with a re-snapshot hint.
- **Sink.** CN1's upsert sink gains `exactly_once` through a `_loams_checkpoints` table in the same transaction.
- **MariaDB** (real MariaDB 11 LTS) runs the same code with MariaDB's GTID format and event differences; **Aurora MySQL** runs nightly.

Tests:
- `mysql_cdc_converges`
- `mariadb_cdc_converges`
- `dblog_snapshot_interleaves_without_loss`
- `binlog_resume_by_gtid`
- `mariadb_gtid_position_roundtrip`
- `ddl_history_maps_columns_at_position`
- `binlog_purged_is_fatal_with_resnapshot_hint`
- `mysql_sink_exactly_once_under_crash`

Commit `connectors: native MySQL and MariaDB binlog CDC with lock-free snapshots and an exactly-once sink`.

### Task 12: SQL Server family — batch, MERGE sink and native CDC; Azure SQL

**Files:** `fabric/crates/loams-connector-sqlserver/src/{batch.rs,sink.rs,cdc.rs,types.rs}`, `connectors/profiles/azure-sql.yaml`, manifests `{sql-server,azure-sql}.yaml` (and `debezium-sqlserver.yaml` as fallback), tests `fabric/crates/loams-connector-sqlserver/tests/{batch.rs,cdc.rs,sink.rs}`, guides, `docs/runbooks/connectors/sqlserver-cdc.md` (enabling CDC per database and table).

**Semantics** (`tiberius`):
- **Batch source:** key-range splits under snapshot isolation, Arrow batches; incremental by a cursor column.
- **Sink:** bulk-load into a session temp table, then `MERGE` by key, with `_loams_checkpoints` in the same transaction (`exactly_once`); deletes for `loamsop = d`.
- **CDC source:** polls `cdc.fn_cdc_get_all_changes_<capture_instance>(from_lsn, to_lsn, N'all update old')` up to `sys.fn_cdc_get_max_lsn()`. The position is the LSN triple (commit LSN, sequence value, operation). The snapshot runs under snapshot isolation at a recorded max LSN. A second capture instance created after DDL is followed and emits a `schema-change`. CDC cleanup-job retention against the position's age is a metric.
- **Azure SQL** runs the same code with Entra ID tokens, nightly on a real database. SQL Server Developer is a `ci-service` (OR-3).

Tests:
- `sqlserver_cdc_converges`
- `sqlserver_cdc_resume_lsn_triple`
- `sqlserver_capture_instance_switch_on_ddl`
- `sqlserver_merge_exactly_once_under_crash`
- `sqlserver_cdc_retention_metric`
- `azure_sql_entra_token_nightly`

Commit `connectors: native SQL Server batch, MERGE sink and CDC, and Azure SQL`.

### Task 13: Objects family — S3 ★ sink, R2, MinIO/RustFS, GCS, Azure Blob, ADLS Gen2 and SFTP

**Files:** `fabric/crates/loams-connector-objects/src/{list.rs,sink.rs,notify.rs,sftp.rs}`, `connectors/profiles/{cloudflare-r2,minio-rustfs}.yaml`, manifests, tests `fabric/crates/loams-connector-objects/tests/{s3.rs,r2.rs,gcs.rs,azure.rs,sftp.rs}`, fixtures `conformance/connectors/fixtures/cloudflare-r2/`, guides.

**Semantics:**
- One implementation over `object_store` (`aws`, `gcp`, `azure`) plus SFTP over `russh-sftp`.
- **Sources:** listing with a high-water key per prefix split (CN1's S3), ids `"<bucket>/<key>@<etag>"`, and decoded rows through the SDK codecs and CN1's formats. Event notifications where the cloud has them: S3 through SQS, GCS through Pub/Sub, Blob through Event Grid, all nightly for the cloud services.
- **Sink:** native (replacing Iggy's `s3_sink` as the default, OR-8): Parquet, NDJSON or raw objects rolled by size (128 MiB) or time (5 min), with deterministic names `<prefix>/<instance>/<split>/<first>-<last>.<ext>` and a conditional `PUT` (`If-None-Match: *`) where the store supports it, which makes the sink `effectively_once`.
- **Targets:** R2 is tested on fixtures recorded from real R2 in PR CI and against a real bucket nightly (polling only, no notifications). MinIO/RustFS runs against both real RustFS and real MinIO (`ci-service`).
- **GCS:** fake-gcs-server in PR CI, a real bucket nightly. **Azure Blob and ADLS Gen2:** Azurite in PR CI, a real account nightly (hierarchical-namespace renames nightly only, if Task 0 finds Azurite lacks them).
- **SFTP:** a polling source; the sink writes to a temporary name and renames.

Tests:
- `s3_native_sink_deterministic_names_effectively_once`
- `conditional_put_refuses_overwrite`
- `r2_fixture_replay_and_nightly`
- `rustfs_and_minio_suites`
- `gcs_and_blob_high_water_resume`
- `adls_hns_rename_nightly`
- `sftp_sink_atomic_rename`

Commit `connectors: native object storage for S3, R2, MinIO/RustFS, GCS, Azure Blob, ADLS and SFTP`.

### Task 14: Key-value and wide-column family — Valkey, Redis Streams, DynamoDB, DynamoDB Streams and Cassandra

**Files:** `fabric/crates/loams-connector-kv/src/{redis/**,dynamodb.rs,ddbstreams.rs,cassandra.rs}`, `connectors/profiles/valkey.yaml`, manifests `{valkey,redis-streams,dynamodb,dynamodb-streams,cassandra}.yaml`, tests `fabric/crates/loams-connector-kv/tests/{valkey.rs,dynamodb.rs,cassandra.rs}`, guides.

**Semantics:**
- **Valkey** runs CN1's Redis module (ported in Task 8) against real Valkey 8; **Redis Streams** is the Streams source and `XADD` sink as its own manifest, tested against Valkey and Redis 8 (`ci-service`).
- **DynamoDB:** parallel `Scan` segments as splits (`LastEvaluatedKey` as the position); a `BatchWriteItem` sink that retries `UnprocessedItems`, with an optional version attribute for conditional, idempotent writes.
- **DynamoDB Streams:** shards as splits with lineage (parents before children), sequence numbers as positions, `loamsop` from `eventName`; a trimmed shard (past 24 h) is `PositionLost`. PR CI on DynamoDB Local (`ci-service`) or floci per Task 0, nightly on AWS.
- **Cassandra** (`scylla` driver): token-range splits with paging state as position; upsert sink, idempotent by primary key. Real Cassandra 5.

Tests:
- `valkey_suite_against_valkey`
- `redis_streams_group_ack_after_fabric` (Valkey and Redis)
- `dynamodb_parallel_scan_segments`
- `dynamodb_unprocessed_items_retried`
- `dynamodb_streams_lineage_followed`
- `dynamodb_streams_ops_map_to_loamsop`
- `cassandra_token_range_partitions`
- `cassandra_paging_state_resumes`

Commit `connectors: native Valkey, Redis Streams, DynamoDB, DynamoDB Streams and Cassandra`.

### Task 15: Search and vector family — Elasticsearch ★, OpenSearch and Qdrant

**Files:** `fabric/crates/loams-connector-search/src/{bulk.rs,pit.rs,qdrant.rs}`, `connectors/profiles/opensearch.yaml`, manifests `{elasticsearch,opensearch,qdrant}.yaml`, tests `fabric/crates/loams-connector-search/tests/{elasticsearch.rs,opensearch.rs,qdrant.rs}`, guides.

**Semantics:**
- **Elasticsearch** becomes native (OR-8): a `_bulk` sink with `_id` = the document key or `ce_id` (`effectively_once`), deletes on `loamsop = d`, retrying only the failed items of a partial failure, and shrinking its batch on `429` rejections. The source reads with a point-in-time and `search_after` (tiebreaker `_shard_doc`), incrementally by a cursor field; the position is the cursor plus the last sort values. Tested against real Elasticsearch (`ci-service`).
- **OpenSearch** is a target on the same code against real OpenSearch 3, mapping its point-in-time API and narrowing what differs; AWS SigV4 for Amazon OpenSearch Service runs nightly.
- **Qdrant** (`qdrant-client`): an upsert sink with point ids as UUID v5 of the key (idempotent), vectors and payload from the instance's mapping; a scroll source with the offset as position. Real Qdrant.
- Loams' collections (ES API) and Qdrant gateway (`crates/loams-qdrant`) are extra targets, run after the real systems (OR-2). Iggy's ES plugins become fallbacks.

Tests:
- `es_bulk_partial_failure_retries_only_failed`
- `es_source_pit_search_after_resumes`
- `opensearch_suite_against_opensearch`
- `opensearch_sigv4_nightly`
- `qdrant_upsert_idempotent_uuid_v5`
- `qdrant_scroll_resumes`
- `loams_endpoints_are_extra_targets_only`

Commit `connectors: native Elasticsearch, OpenSearch and Qdrant`.

### Task 16: MongoDB — batch, change-stream CDC and an exactly-once sink

**Files:** `fabric/crates/loams-connector-mongodb/src/{batch.rs,cdc.rs,sink.rs,types.rs}`, manifests `mongodb.yaml` (and `debezium-mongodb.yaml` marked fallback), tests `fabric/crates/loams-connector-mongodb/tests/{batch.rs,cdc.rs,sink.rs}`, guides, `docs/runbooks/connectors/mongodb-cdc.md`.

**Semantics** (`mongodb` crate; real MongoDB 8 replica set, `ci-service`):
- **Batch source:** `_id`-range splits with snapshot reads (`readConcern: snapshot` at a recorded cluster time); BSON mapped to Arrow by Task 5's mapper (extended JSON for what does not map).
- **CDC source:** change streams on the configured database or collections with `fullDocument: updateLookup` or pre- and post-images (6.0+), started at the snapshot's cluster time, so the handoff has no gap. The resume token is the position; `loamsop` comes from `operationType`; `invalidate` pauses the instance. Oplog window against the token's age is a metric, and a lost token is `PositionLost`.
- **Sink:** `bulkWrite` upserts by key and deletes; on replica sets, a multi-document transaction including a `_loams_checkpoints` document gives `exactly_once`; standalone servers narrow to `effectively_once`.
- Debezium-MongoDB and Iggy's `mongodb_sink` become fallbacks.

Tests:
- `mongodb_cdc_converges`
- `mongodb_snapshot_stream_handoff_no_gap`
- `mongodb_resume_token_restart`
- `mongodb_oplog_window_metric`
- `mongodb_invalidate_pauses`
- `mongodb_sink_exactly_once_under_crash`
- `mongodb_standalone_narrows_to_effectively_once`

Commit `connectors: native MongoDB batch, change-stream CDC and exactly-once sink`.

### Task 17: Analytics and lakehouse — ClickHouse ★ sink, Redshift, Databricks, Trino, DuckDB, Delta Lake and the Iceberg sink

**Files:** `fabric/crates/loams-connector-analytics/src/{clickhouse_sink.rs,redshift.rs,databricks.rs,trino.rs}`, `fabric/crates/loams-connector-lakehouse/src/{delta.rs,iceberg_sink.rs}`, `connectors/profiles/duckdb.yaml`, manifests, tests in each crate, fixtures `conformance/connectors/fixtures/{redshift,databricks}/`, guides.

**Semantics:**
- **ClickHouse sink** (native, OR-8): `INSERT … FORMAT ArrowStream` (or RowBinary) per batch with `insert_deduplication_token` = the batch id (`effectively_once` within ClickHouse's dedup window, declared). Real ClickHouse; Loams House is an extra target.
- **Redshift:** source through the Data API (`ExecuteStatement`, `GetStatementResult` paging), and `UNLOAD` to S3 Parquet for the bulk path (Arrow). Sink: Parquet staged to S3, then `COPY` and `MERGE` through the Data API. Fixtures recorded from real Redshift; nightly real.
- **Databricks:** the SQL Statement Execution API with `format=ARROW_STREAM` and external links (Arrow). Sink: files uploaded to a Unity Catalog volume, then `COPY INTO` or `MERGE`. Fixtures and nightly.
- **Trino:** the client REST protocol (`POST /v1/statement`, following `nextUri`); source only. Real Trino.
- **DuckDB:** a target profile on CN1's ADBC module with the DuckDB driver.
- **Delta Lake** (OR-18, `deltalake`): the source reads table versions (position = version) and the change data feed when enabled (`cdc`). The sink appends or merges, with Delta's `txn` action (`appId` = instance, `version` = checkpoint sequence) for `exactly_once`. Storage on RustFS.
- **Iceberg raw-topic sink** (native, OR-8): iceberg-rust appends with the checkpoint in the snapshot summary property `loams.checkpoint` (`exactly_once`). Lakekeeper catalog.

Tests:
- `clickhouse_sink_dedup_token_on_retry`
- `clickhouse_house_extra_target`
- `redshift_unload_bulk_arrow_fixture`
- `redshift_copy_merge_sink_fixture`
- `databricks_arrow_external_links_fixture`
- `trino_source_only`
- `trino_nexturi_paging_resumes`
- `duckdb_adbc_target`
- `delta_txn_exactly_once_under_crash`
- `delta_cdf_source_ops`
- `iceberg_sink_snapshot_checkpoint_exactly_once`

Commit `connectors: native ClickHouse sink, Redshift, Databricks, Trino, DuckDB, Delta Lake and the Iceberg sink`.

### Task 18: Brokers family — RabbitMQ, AMQP 1.0 (ActiveMQ, Service Bus), NATS, MQTT, Pulsar and WebSocket

**Files:** `fabric/crates/loams-connector-brokers/src/{amqp091.rs,amqp10.rs,nats.rs,mqtt.rs,pulsar.rs,websocket.rs}`, manifests `{rabbitmq-amqp,activemq-jms,azure-service-bus,nats,mqtt,pulsar,websocket}.yaml`, tests `fabric/crates/loams-connector-brokers/tests/*.rs`, guides.

**Semantics:**
- **RabbitMQ** (`lapin`, AMQP 0-9-1): manual acks after the Fabric acknowledges, prefetch as the source's budget (backpressure), publisher confirms in the sink, and `ce_` headers ↔ AMQP headers.
- **AMQP 1.0** (`fe2o3-amqp`, OR-22): ActiveMQ Artemis and Classic (`activemq-jms` manifest; real Artemis) and Azure Service Bus (peek-lock as unsettled deliveries accepted after the Fabric acknowledges; SAS and Entra ID; the emulator (`ci-service`) in PR CI, a real namespace nightly). JMS providers without AMQP 1.0 use the Camel `jms` fallback per instance (Task 22).
- **NATS** (`async-nats`): JetStream durable pull consumers (ack after the Fabric, `resumable: true`), core NATS declared `resumable: false`; the JetStream sink sets `Nats-Msg-Id` = `ce_id` (`effectively_once` within the stream's dedup window).
- **MQTT 5** (`rumqttc`): QoS 1 both ways, persistent sessions (`clean_start = false`), manual acks after the Fabric, user properties ↔ `ce_` headers. Real Mosquitto.
- **Pulsar:** failover or shared subscriptions, ack after the Fabric, Pulsar's CloudEvents properties passed through, producer deduplication by sequence id (`effectively_once`). Real Pulsar.
- **WebSocket** (`tokio-tungstenite`): the source is declared `resumable: false`; the sink sends text or binary frames.

Tests:
- `amqp091_ack_after_fabric`
- `amqp091_publisher_confirms`
- `prefetch_is_backpressure_budget`
- `artemis_amqp10_redelivers_on_kill`
- `servicebus_peek_lock_redelivery`
- `nats_core_declares_not_resumable`
- `nats_jetstream_resumes_and_dedupes_msg_id`
- `mqtt5_qos1_no_loss_on_restart`
- `pulsar_ce_properties_pass_through`
- `pulsar_producer_dedup`
- `websocket_declares_not_resumable`

Commit `connectors: native RabbitMQ, AMQP 1.0, NATS, MQTT, Pulsar and WebSocket`.

### Task 19: Cloud messaging family — SQS, SNS, EventBridge, Pub/Sub and Event Grid

**Files:** `fabric/crates/loams-connector-cloud/src/{sqs.rs,sns.rs,eventbridge.rs,pubsub.rs,eventgrid.rs}`, manifests `{aws-sqs,aws-sns,eventbridge,google-pub-sub,azure-event-grid}.yaml`, tests `fabric/crates/loams-connector-cloud/tests/*.rs`, fixtures, guides.

**Semantics:**
- **SQS:** long polling, visibility extended while a message is in flight, deleted only after the Fabric acknowledges; the FIFO sink uses `ce_id` as the deduplication id.
- **SNS:** a `Publish` sink with message attributes ↔ `ce_`; a source as an HTTP subscription on the webhooks route, verifying SNS message signatures and completing the subscription-confirmation handshake.
- **EventBridge:** a `PutEvents` sink that retries partial failures; the source is a rule targeting an SQS queue that the instance reads (the guide and a runbook set it up).
- **Pub/Sub:** streaming pull, ack after the Fabric with deadline extension, ordering keys; `ce-` attributes pass through; publish with ordering keys. The emulator in PR CI, a real project nightly.
- **Event Grid:** a sink posting the CloudEvents schema (SAS key or Entra ID); a webhook source with Event Grid's validation handshake, input passing through.
- Sources that receive pushes from the cloud (SNS, Event Grid) run on fixtures until Task 28 opens remote ingress, then nightly.

Tests:
- `sqs_delete_after_ack`
- `sqs_visibility_extended_while_in_flight`
- `sqs_fifo_dedup_id_is_ce_id`
- `sns_signature_and_subscription_confirmation`
- `eventbridge_put_events_partial_failure`
- `pubsub_ack_after_fabric_and_deadline_extension`
- `pubsub_ce_attributes_pass_through`
- `eventgrid_validation_handshake`
- `eventgrid_cloudevents_pass_through`

Commit `connectors: native SQS, SNS, EventBridge, Pub/Sub and Event Grid`.

### Task 20: SaaS and services as HTTP-API profiles

**Files:** `connectors/profiles/{kestra-companion,segment,sendgrid,posthog,hubspot,shopify,gitlab,amazon-ses,twilio-sms,salesforce,zendesk,servicenow,stripe,github,jira,slack,google-workspace,keycloak,vault,splunk}.yaml`, the webhooks schemes added in `fabric/crates/loams-connector-http/src/webhook/`, manifests, tests `fabric/crates/loams-connector-http/tests/saas.rs`, fixtures `conformance/connectors/fixtures/<id>/`, guides, and the import routes (below).

**Semantics** (OR-11; Task 7's engine):
- Every row is an HTTP-API profile, tested on fixtures recorded from the real service, with nightly runs on real sandboxes where credentials exist. Real images where the service ships one: stripe-mock, Keycloak, OpenBao (for Vault), Kestra OSS and Splunk (`ci-service`).
- **Salesforce:** REST (SOQL with `nextRecordsUrl`) and Bulk API 2.0 (`async_job`) natively; CDC on the Camel `salesforce` fallback until Task 23. No generated DTOs are needed, because the engine is JSON.
- **Splunk:** a HEC sink with indexer acknowledgement (channel plus ack ids, `effectively_once` as declared), and a search-export source.
- **SES** uses `aws-sigv4`. **Vault**'s `notes` say it is not Loams' own secret store. **Keycloak**: admin events as a source, user and group upserts as a sink. **Kestra companion**: a sink that triggers flows through Kestra's webhook trigger, and a source that receives Kestra's webhooks (no Kestra plugin, Q356).
- Webhook schemes for Stripe, GitHub, Slack and Shopify are CN1 Task 4's; HubSpot v3, Zendesk, Twilio and Segment signatures are added here with fixtures.
- **Import routes.** CN1 Task 15's Slack → Zulip, GitHub → Forgejo and Jira → ItsPlane imports run on these native profile sources. The Camel route templates `connect/routes/templates/{zulip,itsplane,forgejo}-import.yaml.tmpl` are replaced by `connectors/routes/{zulip,itsplane,forgejo}-import.yaml` (instance pairs through a Fabric topic until FL3's route format exists) and deleted.

Tests:
- `stripe_mock_roundtrip`
- `salesforce_bulk2_async_job_fixture`
- `hubspot_profile_upsert_batches`
- `shopify_webhook_verifies`
- `hubspot_zendesk_twilio_segment_signatures`
- `splunk_hec_ack_declared`
- `ses_sigv4_send_fixture`
- `openbao_kv_roundtrip`
- `keycloak_admin_events_source`
- `kestra_companion_triggers_flow`
- `github_to_forgejo_import_replays`
- `jira_to_itsplane_import_replays`
- `slack_to_zulip_import_replays`

Commit `connectors: SaaS and services as native HTTP-API profiles, and the import routes on native sources`.

### Task 21: Protocols, graph, observability, functions and AI

**Files:** `fabric/crates/loams-connector-protocols/src/{grpc.rs,graphql.rs,neo4j.rs,graph.rs,prometheus.rs,functions.rs,llm.rs}` (plus CN1's `otlp.rs` from Task 8), `fabric/crates/loams-flow-proto/build.rs` (the `loams.graph.v1` client per OR-20), manifests `{grpc,graphql,neo4j,grafeo,prometheus,lambda-cloud-run-azure-functions,openai,anthropic}.yaml`, tests `fabric/crates/loams-connector-protocols/tests/*.rs`, guides.

**Semantics:**
- **gRPC** (OR-12): `tonic` with `prost-reflect` over a user-supplied `FileDescriptorSet` or server reflection. Unary calls are a sink with `sink.reply`; server-streaming calls are a source, declared `resumable` only when the request names a cursor field.
- **GraphQL:** a query source with Relay-style cursor pagination and a mutation sink; subscriptions over `graphql-ws` as a streaming source, `resumable: false`.
- **Neo4j** (OR-14): both directions over the HTTP Query API (`POST /db/{db}/query/v2`, verify the minimum server version): a parameterised `MERGE` sink and a source with a cursor property. Real Neo4j Community; no Bolt (D634 (c)).
- **Grafeo** (§48 §4.4, D741): `ExecuteBatch` of parameterised GQL for the sink and `ExecuteStream` with a cursor property for the source; auth `key` (a Loams API key from the `SecretStore`); the manifest drops D634 (b)'s in-process claim.
- **Prometheus:** a remote-write receiver on `ingest`'s listener under `/v1/namespaces/{ns}/fabric/prometheus/{instance}/write` (snappy protobuf, one `metric` event per sample batch per series) and a remote-write sink, tested with real Prometheus as both sender and receiver.
- **Functions:** Lambda `Invoke` (floci, nightly AWS), Cloud Run and Azure Functions over HTTP with ID tokens or function keys; all sinks with `sink.reply`.
- **OpenAI and Anthropic:** native HTTP sinks with `sink.reply`, rate limits from the providers' headers, prompt templates in the instance config, and no key ever in a reply event. Fixtures recorded from the real APIs; nightly real.

Tests:
- `grpc_dynamic_unary_reply`
- `grpc_server_stream_source`
- `graphql_cursor_pagination_resumes`
- `neo4j_sink_merge_idempotent`
- `neo4j_source_http_query_api_cursor`
- `no_bolt_dependency` (a `cargo tree` check)
- `grafeo_sink_batches_through_loams_graph_v1`
- `grafeo_source_cursor_resumes`
- `grafeo_requires_api_key`
- `prometheus_remote_write_roundtrip`
- `lambda_invoke_emits_reply`
- `openai_reply_has_no_key_and_keeps_subject`
- `anthropic_reply_fixture`

Commit `connectors: native gRPC, GraphQL, Neo4j, Grafeo, Prometheus, functions and AI`.

## CN2d — Fallbacks (Tasks 22–23)

### Task 22: Fallback runtimes — Camel, Debezium Server and Iggy plugins, narrowed

**Files:** `connect/routes/templates/{source.yaml.tmpl,sink.yaml.tmpl}`, `connect/routes/overlays/{oracle,salesforce-cdc,jms}.yaml` and `overlay.schema.json`, `connectors/vendor/camel-catalog-4.22.x.json.zst` with its README and SHA-256, `scripts/connectors/{camel_manifest.py,no_java.py}`, `fabric/crates/loams-flow/src/runtime/{camel.rs,debezium.rs,iggy.rs}`, `deploy/fabric/connectors/debezium/{oracle,sqlserver,mongodb,mariadb}.properties.tmpl`, manifests `{oracle,debezium-oracle,debezium-sqlserver,debezium-postgres,debezium-mysql,debezium-mongodb,jdbc}.yaml`, `.github/workflows/fabric.yml` (`connect-routes` switched on), tests `fabric/crates/loams-flow/tests/{fallback.rs,camel.rs,cdc_fallback.rs}`, guides.

**Semantics** (OR-4; the Fallback runtime contract):
- **Selection.** An instance runs a fallback only with `runtime: fallback`; `ValidateInstance` warns, the metrics carry `runtime="fallback"`, and the guide names the replacing task.
- **Camel** carries only Oracle batch and `MERGE` upsert (`sql` with the `user-supplied` `ojdbc`), Salesforce CDC (until Task 23), generic JMS (providers' client jars `user-supplied`) and the JDBC ★. The catalog snapshot and `camel_direction_matches_catalog` cover these rows only. No Java (Global Constraints).
- **Debezium Server** carries Oracle CDC (LogMiner; `ojdbc` mounted at `/debezium/lib/ojdbc*.jar`; supplemental logging per the runbook; archive-log retention against the LogMiner position as a metric) and the opt-in fallbacks for Postgres, MySQL, MariaDB, SQL Server and MongoDB.
- **Iggy plugins** (CN1's renderers) carry the fallbacks Task 2 lists.
- Oracle Database Free runs as a `ci-service` (OR-3).

Tests:
- `fallback_never_default`
- `fallback_metrics_labelled`
- `templates_use_only_allowed_languages`
- `no_java_sources_in_tree`
- `camel_direction_matches_catalog`
- `camel_source_kill_restart_no_loss`
- `oracle_merge_upsert`
- `oracle_cdc_converges` (Debezium, Oracle Free)
- `oracle_without_ojdbc_refused_with_message`
- `debezium_fallback_equivalent_to_native` (the same Postgres workload through Task 10 and through Debezium yields the same `_current` table)

Commit `connect: Camel, Debezium and Iggy as declared fallbacks for Oracle, Salesforce CDC, JMS and JDBC`.

### Task 23: Replace the most-used fallbacks natively

**Files:** `docs/plans/cn2-fallback-queue.md` (the ranked queue), `fabric/crates/loams-connector-http/src/salesforce_pubsub.rs` (or `loams-connector-protocols`, beside gRPC), `fabric/crates/loams-connector-oracle/` (only if the spike passes), manifests, tests, guides.

**Semantics:**
- **The queue.** Fallbacks are ranked by, in order: a named launch customer (Q355's rule), Task 24's activity counters across the instances that run them (open counters; the platform may share aggregates), and Appendix A priority. The queue lists each fallback, its rank, its native replacement and its status. A fallback block is removed from a manifest one release after its replacement reaches `stable` (OR-23).
- **Salesforce CDC** (first in the queue): native over Salesforce's Pub/Sub API (gRPC with `tonic`, Avro payloads with `apache-avro`), the replay id as position; fixtures and a nightly developer org.
- **Oracle spike:** evaluate the `oracle` crate (ODPI-C) with the `user-supplied` Instant Client for batch and `MERGE` upsert, and LogMiner through `DBMS_LOGMNR` queries for CDC, against Oracle Free (`ci-service`). Outcome recorded as a ruling: native batch and upsert if the spike passes (then `loams-connector-oracle` is built here with its suites), and a CDC verdict (native now, or Debezium stays with a reason).
- **JMS** stays a permanent Camel fallback for providers without AMQP 1.0 while Java is deferred; recorded in the queue.

Tests:
- `fallback_queue_matches_manifests`
- `salesforce_pubsub_cdc_replay_id_resumes` (fixture; nightly real)
- `oracle_native_batch_and_merge` (only if the spike passes)

Commit `connectors: the fallback replacement queue, native Salesforce CDC and the Oracle verdict`.

## CN2e — Production (Tasks 24–30)

### Task 24: Connector metrics, alerts and the activity observer

**Files:** `fabric/crates/loams-flow/src/{metrics.rs,activity.rs}`, `fabric/crates/loams-connector-sdk/src/observe.rs`, `deploy/observability/connectors/{dashboards/connectors.json,alerts.yaml}`, the fallback runtimes' metrics scrapes. Tests go in `fabric/crates/loams-flow/tests/activity.rs`.

**Interfaces:**
- Metric families, labelled `namespace`, `instance`, `connector`, `runtime`, `fallback` and `direction` (and `split` only where splits are bounded, capped at 1,024 per instance), never with a secret or a free user string:
  - `loams_flow_connector_events_total`
  - `loams_flow_connector_bytes_total{dir="in|out"}`
  - `loams_flow_connector_api_calls_total`
  - `loams_flow_connector_errors_total{class}`
  - `loams_flow_connector_lag_seconds`
  - `loams_flow_connector_budget_wait_seconds_total` (time sources spent paused by backpressure)
  - `loams_flow_connector_restarts_total`
  - `loams_flow_connector_dlq_total`
  - `loams_flow_cdc_position_risk_ratio` (slot WAL, binlog, oplog, CDC retention or stream trim headroom, 0–1)
- `ConnectorActivityObserver`, as in Shared contracts, called once per window (default 60 s) per instance.
- `InstanceStatus` (CN1 Task 3) gains `last_error`, `lag_seconds`, `restarts` and `runtime_choice`.
- Alerts: instance down for more than 5 min; lag above `lag_alert_seconds` (default 300); CDC position risk above 0.8; DLQ growth; a restart loop (more than 5 in 10 min); sustained backpressure (more than 50 % of a window paused for 15 min).

Tests:
- `metrics_labels_bounded`
- `activity_observer_called_per_window`
- `activity_observer_has_no_wire_format` (no `Serialize`, no socket or file opened)
- `each_alert_fires_in_a_test` (promtool rule tests)
- `no_metering_guard_green`

Commit `flow: connector metrics, alerts and the activity observer`.

### Task 25: Security — egress guard, secret stores, log scrubbing and the threat model

**Files:** `fabric/crates/loams-flow/src/egress.rs`, `fabric/crates/loams-connector-sdk/src/egress.rs` (the hook), `fabric/crates/loams-flow/src/secrets/{file.rs,kube.rs}` (CN1 Task 3's `FileSecretStore` moves here), the fallback renderers (NetworkPolicy), `docs/security/connectors-threat-model.md`, tests `fabric/crates/loams-flow/tests/{egress.rs,secrets_p2.rs}`.

**Interfaces:**
- `EgressPolicy`, as in Shared contracts, applied at `ValidateInstance` to every host in the config, and at connect time after DNS through the SDK's resolver hook for every native connection (and on every HTTP redirect). Fallback pods get a Kubernetes NetworkPolicy rendered from the same policy.
- `KubeSecretStore` (OR-21): Kubernetes Secrets in the instance's namespace; native connectors read them through `SecretResolver`, fallback pods get them mounted read-only as files.
- Log scrubbing: the SDK's logging never formats a `Secret`; the fallbacks' logging configs mask known secret option names; the supervisor's line filter drops and counts any fallback log line containing a resolved secret's bytes.
- The threat model: tenant isolation (native tasks per namespace in `flow` pods with per-namespace budgets; fallback pods per namespace group), SSRF and redirects, secret exposure, webhook forgery and replay, malicious profile or overlay content (`#bean`, `exec:`, `file:` refused), CDC privilege scope (replication roles, read-only roles), and the supply chain (crate audit, image digests, signed images).

Tests:
- `egress_guard_refuses_metadata_loopback_and_cluster`
- `egress_guard_rechecks_after_dns` (DNS rebinding)
- `redirect_rechecked`
- `network_policy_rendered_from_policy`
- `no_secret_in_fallback_logs`
- `no_secret_in_rendered_kube_objects`
- `overlay_refuses_bean_exec_file_schemes`
- `kube_secret_store_reads_secret`

Commit `flow: egress guard, Kubernetes secret store, log scrubbing and the connector threat model`.

### Task 26: The connector fleet on Kubernetes, and the Helm chart

**Files:** `fabric/crates/loams-flow/src/runtime/kube/{mod.rs,native.rs,connect.rs,debezium.rs,iggy.rs,lease.rs}`, `deploy/helm/loams-connectors/`, `.github/workflows/fabric.yml` (job `connectors-kind`). Tests go in `fabric/crates/loams-flow/tests/kube.rs` (golden) and `tests/it_kind.rs` (`#[ignore]`, CI).

**Interfaces:**
- Native connectors run as tasks inside `loams-fabric flow` pods (a Deployment, scaled by instance count and Task 27's budgets); split work spreads across pods through split leases (OR-16).
- `KubeRuntime` renders the fallbacks: `loams-connect` as one Deployment per namespace group with routes in a ConfigMap per instance; Debezium Server as one StatefulSet per CDC instance with a PVC (Q350); the Iggy connectors runtime as one Deployment per namespace group.
- Rendering is deterministic (golden tests), labelled `loams.dev/namespace`, `loams.dev/instance`, `loams.dev/connector`, `loams.dev/runtime` and `loams.dev/fallback`.
- `InstanceLease` and `SplitLease`: fenced leases on `_fabric.flow_objects` (CAS on `version`, holder, expiry), whose epoch every checkpoint write carries (Task 4). FL3's metastore leases replace them behind the same trait.
- Helm chart: image digests, resource budgets from Task 27, NetworkPolicies (Task 25), ServiceMonitors (Task 24), PodDisruptionBudgets.

Tests:
- `render_golden_per_runtime`
- `two_flow_processes_one_instance_runner`
- `stale_lease_holder_fenced`
- `splits_rebalance_on_pod_loss`
- `it_kind_mixed_fleet` (native Kafka, Postgres CDC and S3 sink, plus a Debezium Oracle fallback, all running; kill each pod; no loss)
- `helm_lint_and_template_golden`

Commit `deploy: the connector fleet on Kubernetes and the loams-connectors chart`.

### Task 27: Performance budgets and runtime upkeep

**Files:** `fabric/crates/loams-flow-conformance/src/bench.rs`, `docs/plans/cn2-dependency-spike.md` (measurements), `deploy/helm/loams-connectors/values.yaml` (budgets), `connect/README.md` (the Camel bump procedure, fallbacks only), `.github/workflows/fabric.yml` (jobs `connectors-bench`, scheduled, and `camel-bump`, manual dispatch).

**Semantics:**
- Per native connector: throughput (events and MiB per second) and p99 latency on its real system at three batch sizes, memory per instance, and CPU per 10k events; recorded, with a regression budget of 15 % that fails the scheduled job.
- Native task density: instances per `flow` pod at the default memory budget, written into the chart's defaults.
- Camel bump procedure (CN-R5, fallbacks only): regenerate the catalog snapshot, rerun `camel_direction_matches_catalog`, validate every fallback route, rerun every fallback suite. Q352's Quarkus measurement is dropped: Camel is fallback-only, and Camel Main stays.
- Crate upkeep: a scheduled `cargo outdated` and advisory check per family crate, opening one issue per family.

Tests:
- `bench_regression_budget_enforced`
- `budget_values_match_measurements`
- `camel_bump_dry_run_green`

Commit `connectors: performance budgets, crate upkeep and the fallback Camel bump procedure`.

### Task 28: Remote ingress for webhooks, OTLP and Prometheus (gated on Q30)

**Files:** the edge configuration that Q30's plan names (Envoy, D184), the listener binding in `loams-connector-http` (webhooks) and `loams-connector-protocols` (OTLP, Prometheus), and docs.

**Semantics:** when the unified auth plan has landed, `ingest`'s connector routes are exposed through the edge with TLS and per-route credentials (OR-17). Webhooks keep their provider signatures as a second factor. OTLP and Prometheus remote-write take a per-instance bearer key. SNS and Event Grid sources (Task 19) move from fixtures to nightly real pushes. Until then this task does not start, and the loopback rule holds (§33 §2.2).

Tests:
- `edge_route_requires_credential`
- `webhook_signature_still_checked_behind_edge`
- `otlp_remote_with_key`
- `loopback_only_before_gate` (always runs)

Commit `fabric: remote ingress for webhooks, OTLP and Prometheus through the edge`.

### Task 29: Upstream contributions (Q357) through an Iggy adapter

**Files:** `fabric/crates/loams-connector-iggy-adapter/` (wraps any `ConnectorFactory` as an Iggy connectors-runtime source or sink plugin), PRs on `dina-kar/iggy` posted upstream only with the owner's go-ahead (§23 §2.2), and `docs/plans/cn2-exit-report.md` (the list of upstream issues and PRs).

**Semantics:**
- The adapter maps the SDK's traits onto `iggy_connector_sdk`'s plugin interface, so one first-party connector set serves both Loams and Iggy (Q357).
- First plugins: Kafka, ADBC and Kinesis (Q357's list), then the native replacements of Iggy's own plugins where ours pass more suites (Elasticsearch, ClickHouse, MongoDB, Postgres CDC), proposed upstream with the suite results.
- The family crates stay the source of truth; the plugins are builds of them.
- Upstream issues for every gap CN2 recorded in a protocol crate, an emulator, `camel-iggy` or Debezium.

Tests: `adapter_runs_reference_connector_as_plugin`; the CN1 and CN2 suites, run against the plugin builds through `IggyRuntime`, are green.

Commit (fork): `connectors: loams connector plugins through the SDK adapter`. Commit (here): `sdk: the Iggy plugin adapter and upstream contributions`.

### Task 30: The CN2 gate, catalog page, runbooks and exit report

**Files:** `fabric/crates/loams-fabric/tests/e2e.rs` (`cn2_*`), `docs/guides/connectors/index.md` (regenerated), `docs/runbooks/connectors/{native-runtime.md,postgres-cdc.md,mysql-cdc.md,sqlserver-cdc.md,mongodb-cdc.md,fallbacks.md,secrets.md,egress.md}`, `docs/plans/cn2-exit-report.md`, `docs/plans/README.md` (CN2 status), `docs/design/33-connectors.md` (§8 rollout and §9 as built), and `CHANGELOG.md`.

**Semantics:** the gate is green when:
- every P1 and P2 manifest has a native default runtime, or a `fallback-only` note with a queue entry (Task 23);
- every P2 manifest is `preview` with its suites green against its real system, or `planned` with a `notes` entry and an owning task;
- every `exactly_once` manifest passes the crash suite;
- the 24-hour mixed soak on kind passes: 20 instances (at least 15 native, plus Debezium Oracle and one Camel fallback), with pod kills every 15 minutes, no loss, duplicates within the declared delivery, bounded memory, and no alert flapping;
- CDC convergence holds natively for Postgres, MySQL, MariaDB, SQL Server and MongoDB, and through Debezium for Oracle;
- the three import routes (Slack to Zulip, GitHub to Forgejo, Jira to ItsPlane) run end to end on fixtures.

The exit report lists, per connector: the real systems and versions tested, throughput and latency, duplicate counts in kill tests, the narrowings, the fallback (if any) and its queue rank, and the upstream issues opened.

Tests:
- `cn2_mixed_soak` (CI scheduled, 24 h)
- `cn2_cdc_sources_converge`
- `cn2_import_routes_end_to_end`
- `catalog_page_matches_registry`
- `p1_p2_native_or_queued`
- `p2_all_preview_or_explained`

Steps: each runbook step is executed once on kind and marked verified → commit `connectors: the CN2 gate, catalog page, runbooks and exit report`.

---

## Exit criteria for production (with the owning tasks)

- [ ] **Registry truth:** per-direction runtimes, fallbacks, profiles, effectively-once delivery, the native-first assignment and the Debezium split. Appendix A equals the CSV: Tasks 1–2.
- [ ] **Licences:** family crates pass D359; CI-only and user-supplied components are never in an image: Task 3.
- [ ] **SDK:** traits, splits, fenced checkpoints, delivery up to exactly-once, backpressure, retries, schema mapping and codecs: Tasks 4–5.
- [ ] **Test estate:** the suites, real systems for every connector, fixtures recorded from real systems, nightly real accounts: Task 6.
- [ ] **Profiles and ★ port:** targets and HTTP-API profiles; every ★ native module on the SDK: Tasks 7–8.
- [ ] **Every P1/P2 system native**, `preview` with green suites against its real system, or `planned` with an owned reason: Tasks 9–21.
- [ ] **Fallbacks declared and shrinking:** Camel, Debezium and Iggy only where OR-4 says, the queue ranked, Salesforce CDC native, the Oracle verdict recorded: Tasks 22–23.
- [ ] **Observability:** metrics, dashboards, every alert tested, the activity observer with no billing: Task 24.
- [ ] **Security:** the egress guard, Kubernetes secrets, log scrubbing, the threat model, and no secret anywhere: Task 25.
- [ ] **Fleet:** native tasks with split leases, fallback pods, the fenced leases, and the Helm chart: Task 26.
- [ ] **Budgets and upkeep:** per-connector performance budgets enforced, crate upkeep, the fallback Camel bump procedure: Task 27.
- [ ] **Remote ingress:** behind the edge once Q30 lands, loopback until then: Task 28.
- [ ] **Upstream:** the Iggy adapter and the Q357 plugins proposed, gaps filed: Task 29.
- [ ] **Gate:** the soak, CDC convergence, import routes, catalog page, runbooks and exit report: Task 30.

## Self-review

- **Spec coverage.**

  | §33 section (as revised 2026-10-10) | Task(s) |
  |---|---|
  | §1 D352 registry, instances | 1, 2 |
  | §1 D353 capabilities, refusal, tests per capability | 1, 6, 9–21 |
  | §1 D354 runtimes (native first, OR-1; fallbacks, OR-4) | 2, 4, 8, 22, 23 |
  | §1 D355 envelope, pass-through | 4, 9, 18, 19 |
  | §1 D356 bulk as Arrow | 4 (`EventBatch::Arrow`), 5, 10, 13, 17 |
  | §1 D357 CDC (native, OR-9) | 10, 11, 12, 14, 16, 22 |
  | §1 D358 rollout (CN2 = P2 native) | 2, 9–21, 30 |
  | §1 D359 licence gate | 3 |
  | §2.2 remote ingress | 28 |
  | §2.2 exactly-once only where the target dedupes (OR-7) | 1, 4, 6, 9–17 |
  | §8 rollout, §9 gate (extended to CN2) | 30 |
  | §10 open core, platform fleet | 24 (observer), 26 |
  | §11 CN-R1 drift | 6, 22, 27 |
  | §11 CN-R2 JVM footprint | 22, 26 (fallback-only) |
  | §11 CN-R3 Debezium pods | 10–12, 16 (native CDC), 22 |
  | §11 CN-R4 ADBC drivers | 8, 17 |
  | §11 CN-R5 `camel-iggy` preview | 0, 22, 27 |
  | §11 CN-R6 SaaS changes | 6, 20 |
  | §11 CN-R7 secrets | 4, 25 |
  | Q351 (superseded by OR-9) | 10 |
  | Q352 (dropped: Camel fallback-only) | 27 |
  | Q357 | 29 |
  | Q359 | 24 |
  | Appendix A P1 non-native ★ and every P2 row | 2, 8–23 |
  | Owner direction 2026-10-10 | Owner rulings; 2, 6, 22, 23 |

- **Types.** The manifest additions, `Profile`, the SDK traits and `PipelineConfig`, `TypeMapper`, the conformance kit's `Suite`, `ServiceKind` and `FaultPoint`, the P2 event names, `ConnectorActivityObserver` and `EgressPolicy` are defined once, in Shared contracts. CN1's `ConnectorRuntime`, `SecretStore` and `ConnectorHarness` are used; `Source` and `Sink` move into the SDK with their method names (OR-5).
- **Review Focus.** Items 1–13 each name an owning test (Tasks 1, 3, 4, 5, 6, 7, 9, 10, 11, 16, 19, 22, 24, 25, 26).
- **Placeholders.** Cells marked (verify) are Task 0's checks, and every one has a named fallback (OR-24).
- **Coverage of the 73 P2 rows.** Task 9: 2; Task 10: 3; Task 11: 1; Task 12: 3; Task 13: 6; Task 14: 5; Task 15: 2; Task 16: 2; Task 17: 5; Task 18: 7; Task 19: 5; Task 20: 20; Task 21: 8; Task 5: 2; Task 22: 2 (Oracle, `debezium-oracle`). MCP moved to P3 (OR-15).

## Open questions

None open. CN2-Q1 to CN2-Q15 of the first draft are answered in "Owner rulings 2026-10-10": Q1 → OR-10, Q2 → OR-2, Q3 → OR-11, Q4 → OR-12, Q5 → OR-13, Q6 and Q7 → OR-3, Q8 → OR-14, Q9 → OR-15, Q10 → OR-16, Q11 → OR-17, Q12 → OR-18, Q13 → OR-19, Q14 → OR-20, Q15 → OR-21. Items that need action outside this plan are under "Follow-ups outside this plan".

## Rulings made during execution

(Task 0 and later tasks append here.)
