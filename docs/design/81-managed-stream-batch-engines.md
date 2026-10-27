# 81 — Managed Stream and Batch Engines: RisingWave, Flink SQL, Spark

Status: **Approved** (owner, 2026-10-27: "approve all defaults") · proposed 2026-10-27. D1900–D1931 are approved, the lanes' D1950–D2019 are approved as this design amends them (§18), Q1400 is answered by the owner's request of 2026-10-27 and Q1401–Q1411 with their defaults (§20). The code lanes' decisions D1950–D2019 are declared here too (§18), as proposed and as this design amends them.

Source: the owner, 2026-10-27: "start rising wave managed flink sql and spark write as much code as possible" (the wave brief, `loams-wt/.he0-shared/wave-2026-10-27.md`). Seven code lanes (RW-a, RW-b, FL-a, FL-b, SP-a, SP-b, ENG-UI) started at the same time as this design and logged their assumptions in `loams-wt/.he0-shared/engines-81.md`. This document reconciles them. Where they disagreed, it decides, and the ruling was written back into that file (rulings R0–R30) so the lanes follow it while they write code.

Decisions **D1900–D1931** (D1932–D1949 kept free) and open questions **Q1400–Q1411** (Q1412–Q1429 kept free) are declared in the [decision log](13-decision-log.md). So are the lanes' ranges, **D1950–D2019**, so that the lanes add no decision-log rows of their own (ruling R0) and the checker never sees a duplicate.

Plans: [RW1](../plans/2026-10-27-rw1-managed-risingwave.md) (managed RisingWave, with the shared foundation), [FK1](../plans/2026-10-27-fk1-managed-flink-sql.md) (managed Flink SQL) and [SP1](../plans/2026-10-27-sp1-managed-spark.md) (managed Spark). **Track ids.** The Flink track is **FK**, not FL: FL1 and FL2 are the Fabric plans (`2026-10-01-fl1-fabric-foundation.md`, `2026-10-01-fl2-house-sql.md`, §32). The lane names FL-a and FL-b stay as they are; they are lane names, not track ids. RW and SP are free.

This document builds on:
- [§02](02-stream-engine.md) §7.3 and [§32](32-loams-flow-fabric-house.md): RisingWave as the companion stream processor (D22), reading the Fabric through Iggy's Kafka gateway (D341, D408); no Spark or Flink targets in Flow (D341).
- [§17](17-ai-data-ecosystem.md) §6.1 and [§26](26-jobs-api.md) §9–§10: PySpark on Sail through Spark Connect (D211); Flink SQL ported to RisingWave and DataStream on the Flink Kubernetes Operator (D212); the Jobs API (`submit_engine_job`, D204–D216).
- [§68](68-open-protocol-compute.md): the protocol ladder (D1251); Sail is Loams' Spark (D1262); the `spark` gateway module (D1263); one catalog (D1265); engines stay separate (D1266); companion deployment and `LoamsCompanion` (D1267); companion metering (D1268); licences (D1271).
- [§75](75-loams-house-engine.md): the House engine, whose tables are Iceberg v3 in the bucket, read by other engines through Lakekeeper (D1540, D1549).
- [§58](58-data-access-and-htap.md): Postgres wire first, and HTAP by moving data between engines through the bucket and the catalog, not through links.
- [§80](80-confluent-byoc.md): Confluent Cloud is an external Kafka edge, and Confluent's Flink is out of scope there (D1835).
- [§63](63-loams-meter.md) (the Meter: usage events, the gate), [§57](57-loams-gateway.md) (`loams-gateway`), [§56](56-loams-sandbox.md) (`loams-sandbox`), [§65](65-loams-operator.md) (the operator; exposure D1195), [§76](76-identity-and-authorization.md) (Zitadel and OpenFGA).
- [§70](70-byoc-clouds.md) (BYOC: the VM tier, never public, D1359).

Markers, as in §70 and §80:
- **(verify)**: not checked against a primary source; the task that depends on it checks it first.
- **(estimate)**: judged, not measured.

Vendor facts were read on **2026-10-27** from the page linked in §21. Pins follow D1274: a release at least 14 days old when the PR opens (on 2026-10-27 that means released on or before 2026-10-13).

---

## 1. Summary

### 1.1 The problem

Loams already decided where stream processing and Spark belong, piece by piece: RisingWave as a companion (D22, D341), Sail per namespace (D211, D1262), Flink DataStream on the operator and Flink SQL ported to RisingWave (D212), and a generic `LoamsCompanion` kind (D1267). None of it is built. There is no engine crate, no compose profile for any of the three, no Lakekeeper in the release stack, and no API, CLI or console surface. The owner now asks for three *managed* engines, and one of them, Flink SQL, is a product §26 deliberately did not manage: D212 sent Flink SQL users to RisingWave with a porting guide.

Seven code lanes are writing the code now. Without one design, they would pick seven tenancy units, three bucket layouts, two catalog names and two ways of storing credentials, and they had already started to (engines-81.md, first round).

### 1.2 The answer

- **Three engines, one control plane** (D1900, D1902). `crates/loams-engines` owns instances, the phase machine, validation, authorisation, the HTTP facade and metering. `loams-risingwave`, `loams-flink` and `loams-spark` are stateless adapters behind one `ManagedEngine` trait (ENG-UI's shape, D2010–D2015).
- **Why each.** RisingWave is the streaming database: Postgres wire, materialised views, state in the bucket. Flink SQL serves teams who already have Flink SQL and should not have to port it (this amends D212). Spark serves batch DataFrames and SQL: Sail by default, Apache Spark 4.1 as the opt-in backend for coverage (D1262 kept).
- **Apache-2.0 only, and no vendor premium** (D1901). Loams sets no RisingWave licence key and uses no Premium feature, including `CREATE SECRET`. It uses no Ververica or Confluent Flink pieces and no Databricks pieces.
- **The unit is the namespace, one instance per set of processes** (D1903, D1904). An instance belongs to one Loams namespace (`environment` in OpenFGA). No managed cluster is shared between namespaces: RisingWave's database isolation and resource groups are Premium, Flink session clusters run user code, and Sail runs tenant Python in-process.
- **Three deployment modes, one contract** (D1906). Compose: one override file per engine plus a shared `compose.lake.yaml`, single-tenant, preview. Kubernetes: per-instance objects rendered by the adapters (the Flink Kubernetes Operator for Flink), under `LoamsCompanion` in OC2. BYOC: either of the two on the user's VM or cluster.
- **State in the bucket, metadata in Loams Postgres** (D1907–D1912):
  - RisingWave keeps its meta store on Loams Postgres and its Hummock state on S3.
  - Flink keeps checkpoints and savepoints on S3, with Kubernetes HA in k8s and a recovery record on S3 in compose.
  - Spark writes event logs (JVM Spark) under the instance's prefix, and every table goes through the catalog.
- **One catalog, three data paths** (D1913–D1916):
  - Lakekeeper's Iceberg REST catalog, called `loams` in every engine.
  - The House reads what the engines write as Iceberg tables. No engine joins the House's private link.
  - The Iggy Kafka gateway is the stream source and sink. It is at-least-once because it has no transactions.
  - Loams Postgres CDC uses logical replication, with publications the controller creates.
- **Never public; Loams principals only** (D1917–D1919):
  - Every way in goes through `loams-gateway`: pg wire for RisingWave, the Flink SQL Gateway REST protocol and Loams' API for Flink, and Spark Connect for Spark.
  - Each instance has its own service account and scoped credentials. A user's API key is never written into engine SQL or configuration.
  - OpenFGA gets an `engine` type.
- **Isolation by sandbox profile** (D1905). Engine pods run the `linux` sandbox spec. User code runs only under `gvisor` in multi-tenant clusters: RisingWave UDFs, Flink UDF jars and Spark Python UDFs on shared nodes.
- **Metered like other workloads** (D1920). One runtime event type, `io.loams.dev.usage.engine.runtime.v1`, and an hourly storage gauge. The Meter gate is called on create, resume and scale up, and a hard limit suspends an instance; it never deletes one.
- **Surfaces** (D1921–D1923). A REST facade under `/v1/namespaces/{ns}/engines`, `loams engines` plus `loams risingwave|flink|spark`, and console pages with the shared SQL editor, all labelled **preview**.
- **Tested against upstream's own suites** (D1927): RisingWave's sqllogictest e2e files, Flink's SQL tests and Nexmark, the PySpark Spark Connect parity tests, and cross-engine Iceberg round trips. Each has a strict expected-failure list.

### 1.3 Decisions

| # | Decision | § | Status |
|---|---|---|---|
| D1900 | **Managed engines:** RisingWave, Flink SQL and Spark are hosted per namespace as preview products. Flink SQL is now managed on Apache Flink (**amends D212**, which ported it to RisingWave; the porting guide stays as an option). D341 holds: Flow has no Spark or Flink targets. D1262 holds: Sail is Loams' Spark, and Apache Spark is a second backend. §68's "no JVM in the default deployment" holds, because every engine is opt-in | 4 | Approved (owner, 2026-10-27) |
| D1901 | **Licences:** Apache-2.0 engines and connectors only (the Postgres JDBC driver is BSD-2-Clause, allowed by D1271's table). Pins at least 14 days old. No RisingWave licence key and no RisingWave Premium feature on any default or documented path. No Ververica (VVR, VVP) or Confluent Flink pieces, and no Databricks pieces. Images are upstream's or Loams-built from upstream binaries with a jar manifest | 5 | Approved (owner, 2026-10-27) |
| D1902 | **One control plane:** `crates/loams-engines` holds the instance store, ids, phase machine, validation, authorisation, HTTP facade and metering. Engine crates are stateless adapters implementing ENG-UI's `ManagedEngine` (adopts D2010, D2014) | 6.1 | Approved (owner, 2026-10-27) |
| D1903 | **Tenancy unit:** the Loams namespace. An instance has an id `eng_<26 base32>` and a name unique per namespace and kind, and lives under `/v1/namespaces/{ns}/engines/{id}`. Runtime resources are named from the id, and the bucket prefix is `engines/<ns>/<id>/` | 6.2 | Approved (owner, 2026-10-27) |
| D1904 | **No shared engine clusters in managed mode:** each instance runs its own processes (amends Q1035's answer and D1267's sharing rule for RisingWave). Compose is single-tenant | 10.4 | Approved (owner, 2026-10-27) |
| D1905 | **Sandbox profile:** engine pods get a `loams-sandbox` spec rendered to the pod: non-root, all capabilities dropped, `no_new_privs`, RuntimeDefault seccomp, egress limited to Loams services. User code (RisingWave UDFs, Flink UDF jars, Spark Python UDFs) on shared nodes needs the `gvisor` runtime class, and is refused where the node reports none | 10.5 | Approved (owner, 2026-10-27) |
| D1906 | **Deployment modes:** compose overrides `compose.{risingwave,flink,spark}.yaml` and the shared `compose.lake.yaml`, with profiles `risingwave`, `flink`, `spark` and `spark-jvm`, all needing `rustfs`, single-tenant, and no host port but `engine-gateway` on loopback. Kubernetes: per-instance objects in namespace `eng-<26>` rendered by the adapters, under `LoamsCompanion` (D1267) once OC2 lands. BYOC: either, on the user's VM or cluster | 7 | Approved (owner, 2026-10-27) |
| D1907 | **Bucket layout and credentials:** bucket `loams-engines`, prefix `engines/<ns>/<id>/`, holding `hummock/`, `backup/`, `checkpoints/`, `savepoints/`, `ha/`, `recovery/`, `spark-events/`, `scratch/` and `jobs/`. Iceberg data lives in the warehouse bucket `loams-lake`. Every instance gets S3 keys scoped to its prefix, and no engine sets bucket-wide lifecycle rules | 8.1 | Approved (owner, 2026-10-27) |
| D1908 | **RisingWave storage:** the meta store on Loams Postgres (direct endpoint, one database and role per instance) and Hummock on S3. Meta backups go to `backup/`. Distributed mode with meta, frontend, compute and compactor at every size (adopts D1950–D1952) | 8.2 | Approved (owner, 2026-10-27) |
| D1909 | **Flink SQL shape:** each instance has one session cluster (JobManager and TaskManagers) and one Flink SQL Gateway. Statements go through the gateway's REST API. A pipeline is stored as SQL text with secret refs plus its last savepoint. Catalogs are session-scoped and recreated by a bootstrap script. No compiled plans or catalog store in FK1, and application mode per pipeline is FK2 | 8.3 | Approved (owner, 2026-10-27) |
| D1910 | **Flink state and HA:** RocksDB with incremental checkpoints every 60 s, exactly-once, retained on cancel. Checkpoints and savepoints go on S3, with CANONICAL savepoints for upgrades. k8s uses Kubernetes HA with `storageDir` on S3. Compose has no Flink HA; the controller instead keeps a recovery record on S3 and replays it (adopts D1974, D1975) | 8.3 | Approved (owner, 2026-10-27) |
| D1911 | **Spark shape:** Spark Connect is the session protocol, with backend `sail` (default, 0.7.2) or `spark` (Apache Spark 4.1.3 Connect server, opt-in). Batch jobs run as driver containers against the instance's endpoint, or as `spark-submit` client-mode driver pods for jars. There is no Kubeflow Spark Operator (**amends D211**'s operator clause) | 8.4 | Approved (owner, 2026-10-27) |
| D1912 | **Spark logs and warehouse:** on JVM Spark, event logs go to `spark-events/`, with an optional read-only History Server behind the gateway. Sail writes OTLP telemetry and no event log. There is no Hive metastore, `spark.sql.warehouse.dir` is scratch only, and every table is in catalog `loams` | 8.4 | Approved (owner, 2026-10-27) |
| D1913 | **One catalog:** Lakekeeper's Iceberg REST catalog with vended credentials (D1265), called `loams` in every engine. The Iceberg namespace is `ns_<id>[.<db>]`, which is where the House looks. In compose, Lakekeeper runs in `compose.lake.yaml` with its database on Loams Postgres and authentication off on an internal network (single-tenant). In k8s it uses its own Postgres (D413), Zitadel OIDC and OpenFGA | 9.1 | Approved (owner, 2026-10-27) |
| D1914 | **The House reads engine tables as Iceberg:** no engine joins `house-link`, and there is no ClickHouse-sink into the House by default | 9.2 | Approved (owner, 2026-10-27) |
| D1915 | **Kafka:** the Iggy fork's Kafka gateway (`kafka` profile) is the source and sink for all three engines, authenticated with the instance's own Iggy user. With no Kafka transactions, Kafka sinks are at-least-once, and exactly-once end to end holds only into Iceberg | 9.3 | Approved (owner, 2026-10-27) |
| D1916 | **Loams Postgres CDC:** RisingWave's `postgres-cdc` and Flink CDC's `postgres-cdc` read from the direct endpoint over pgoutput. The controller creates the publication under a CDC role per source, slots are named `loams_<rw\|fk>_<id26>_<n>`, and a slot is dropped when its source or instance is deleted. A slot-lag alarm and the scale-to-zero interaction come first (verify) | 9.4 | Approved (owner, 2026-10-27) |
| D1917 | **Engine service credentials:** each instance is `service_account:eng_<26>` with scoped credentials: an Iggy user, a Lakekeeper client, a CDC role and S3 keys. A user's key is never in engine SQL, configuration or catalogs. Credentials reach processes through env or mounted files, never argv. RisingWave connectors carry the scoped credentials inline, because `CREATE SECRET` is Premium | 10.1 | Approved (owner, 2026-10-27) |
| D1918 | **Access only through `loams-gateway`:** RisingWave on pg wire (terminate TLS, Loams key as the password inside TLS, upstream as the service user; adopts D1957). Flink through its raw SQL Gateway REST, routed by hostname, with the web UI read-only, plus Loams' API on the facade. Spark through the `spark` module (D1263). Hostnames are `<instance>.<ns>.<region>.{rw,flink,spark}.<base-domain>`, and compose uses loopback ports 4566, 8083 and 15002 | 10.2 | Approved (owner, 2026-10-27) |
| D1919 | **Authorisation:** an OpenFGA type `engine` under `environment` with `can_view`, `can_operate`, `can_admin` and `can_connect`. Until its tuples exist, the same checks run on `environment:<ns>`. Reads need `can_view`, lifecycle, SQL, sessions and jobs need `can_operate`, create, delete and upgrade need `can_admin`, and the data path needs `can_connect` (amends D2016) | 10.3 | Approved (owner, 2026-10-27) |
| D1920 | **Metering:** `io.loams.dev.usage.engine.runtime.v1` (`cpu_ms`, `memory_mib_ms`, `estimated`) every 60 s per running instance, and the hourly gauge `io.loams.dev.usage.engine.storage.v1` (`state_bytes`). Both are emitted by the facade, with ids `<instance>/<window_end_unix>`. The Meter gate is called on create, resume and scale up, and a hard limit suspends an instance. This replaces D1268's `companion.runtime.v1` name and amends D2017 | 11 | Approved (owner, 2026-10-27) |
| D1921 | **API:** the facade of D2015 under `/v1/namespaces/{ns}/engines`, with `/sql` (RisingWave, Flink), `/flink/...` (FL-b's sessions, statements and pipelines), `/sessions` and `/jobs` (Spark), errors `{error, message}`, and the contract `api/engines/openapi.json` | 12.1 | Approved (owner, 2026-10-27) |
| D1922 | **CLI:** `loams engines {list,create,get,scale,upgrade,suspend,resume,delete,usage}`, plus `loams risingwave`, `loams flink` and `loams spark` (RW-b A11, D1989, SP-b) as clients of the facade, each behind a cargo feature | 12.2 | Approved (owner, 2026-10-27) |
| D1923 | **Console:** Engines pages in the console shell, with the shared SQL editor (Postgres dialect for RisingWave, Flink SQL mode, Spark SQL), pipelines and savepoints for Flink, sessions and jobs for Spark, usage, and a Preview badge everywhere (adopts D2018) | 12.3 | Approved (owner, 2026-10-27) |
| D1924 | **Lifecycle:** one phase machine (D2012) with one operation at a time per instance. Suspend means RisingWave scales every role to zero keeping meta and Hummock, Flink stops with a savepoint, and Spark has no backend running. Resume reverses it, and the first Spark connection triggers resume | 13.1 | Approved (owner, 2026-10-27) |
| D1925 | **Upgrades:** forward only, within the versions each adapter declares. RisingWave takes a meta backup first, then rolls meta, compactor, compute and frontend. Flink stops with CANONICAL savepoints, redeploys and restores, and rollback is the old version from the same savepoint. Spark serves new sessions on the new version and lets running jobs finish | 13.2 | Approved (owner, 2026-10-27) |
| D1926 | **Failure handling:** failures go to `Failed{op}` with retry-same-op or delete, the per-engine table of §13.3 applies, data is never deleted without `purge`, and every alarm is visible in `observe()` checks | 13.3 | Approved (owner, 2026-10-27) |
| D1927 | **Tests:** upstream suites as oracles (RisingWave sqllogictest e2e subset, Flink SQL tests and Nexmark, the PySpark Connect parity tests, Sail's own), cross-engine Iceberg round trips including the House, strict `xfail.toml` lists, and nightly runs on GitHub-hosted runners. Per-PR jobs only lint and render goldens | 14 | Approved (owner, 2026-10-27) |
| D1928 | **Status labels:** every surface says **preview**, and docs say "not yet run end to end" until a lane has run it. `GET /v1/engines` carries `stability` (adopts D2019) | 15 | Approved (owner, 2026-10-27) |
| D1929 | **Phases and tracks:** RW1 (with the shared foundation), FK1 and SP1 now, then RW2, FK2 and SP2 once preview exits. Track FK, not FL | 16 | Approved (owner, 2026-10-27) |
| D1930 | **Not built:** no Loams fork of any engine, no Flink DataStream or JAR jobs in FK1 (D212's operator path stays), no Kafka Connect, no shared multi-tenant clusters, no engine on the public internet, no premium or vendor pieces, no Loams Cloud (§78) hosting in this phase | 16.2 | Approved (owner, 2026-10-27) |
| D1931 | **Default pins:** RisingWave v3.1.0, Flink 2.2.1 with Iceberg 1.12.0, Kafka connector 5.0.0-2.2, JDBC 4.1.0-2.2 and Flink CDC 3.7.0, Apache Spark 4.1.3 with iceberg-spark-runtime-4.1 1.12.0, Sail 0.7.2, Lakekeeper v0.13.6, Flink Kubernetes Operator 1.16.1. Each is pinned by digest or checksum in its lane's files | 5.2 | Approved (owner, 2026-10-27) |

### 1.4 Phases

| Phase | What | Effort (estimate) |
|---|---|---|
| **RW1** | The shared foundation: `loams-engines`, the facade, authorisation, metering, `compose.lake.yaml`, `engine-gateway` and the console. Then managed RisingWave: adapter, compose, Helm chart, gateway route, connectors, SQL API, CLI and conformance | 4–5 engineer-weeks (foundation 2, RisingWave 2–3) |
| **FK1** | Managed Flink SQL: adapter, SQL Gateway client, compose, k8s via the operator, catalogs, connectors, image, API, CLI, Nexmark and SQL tests | 3–4 engineer-weeks |
| **SP1** | Managed Spark: the Connect proxy as a gateway module, the Sail and Spark backends, compose, k8s, jobs, the Python helpers, CLI and parity tests | 3–4 engineer-weeks |
| RW2, FK2, SP2 | After preview exits: UDFs under gVisor, Flink application mode per pipeline and compiled plans, Spark History Server, `LoamsCompanion` (OC2), Loams Cloud | Not estimated |

The code for RW1, FK1 and SP1 is being written now (2026-10-27) without local builds (owner rule for this wave). CI is the compile check, and nothing has run end to end.

---

## 2. Scope

**In:** hosting the three engines for a namespace, end to end: lifecycle, storage layout, catalog wiring, stream and CDC wiring, access, authorisation, isolation, metering, the API, CLI and console, upgrades, failures and tests.

**Out:**
- Writing a stream processor or a Spark engine (D1261's L4 stays a long-term direction).
- Flink DataStream and JAR jobs. D212's operator path is unchanged and becomes FK2 or a §26 J4 task.
- Kafka Connect.
- Confluent's Flink (§80 D1835).
- Hosting the engines in Loams Cloud on TiDB X (§78), which waits for preview to exit (Q1408).

---

## 3. Where things are today (checked on `dev` at `b444af39a`, 2026-10-27)

| Piece | State |
|---|---|
| Engine crates | None on `dev`. `crates/loams-risingwave`, `loams-flink`, `loams-spark` and `loams-engines` are being written in lane worktrees (rw-a-controller, rw-b-connectors, fl-a-core, fl-b-catalog, sp-a-connect, sp-b-jobs, eng-ui) |
| Compose | `deploy/release/compose.yaml` has the `kafka` profile (Iggy fork, `kafka-gateway` on 127.0.0.1:9093, SASL off by default, no transactions), `rustfs` (in `deploy/loams-postgres-dev/compose.yaml`), `tikv`, `authz` and `postgres-control`. `compose.compute.yaml` has `temporal`, `dbos` and `airflow` on Loams Postgres's direct endpoint `compute1:55433` (DX1 R0.7). **No Lakekeeper** in any compose file |
| Gateway | `crates/loams-gateway`: the `pg` module (splice or terminate to PgDog) and the `temporal` module. No `spark`, `risingwave` or `engines` HTTP routes yet |
| House | Network `house` plus `house-link`, a network only the engine and the House join. On a single node the House uses its local SQLite catalog, and Lakekeeper only in clusters (§75 §3) |
| OpenFGA | `deploy/openfga/core.fga`: org → project → environment (= namespace), plus `deployment` and `database`. No `engine` type |
| Meter | §63's event types. D1268's `companion.runtime.v1` is not emitted by anything |
| Sandbox | `crates/loams-sandbox` (D1008) with the `linux` adapter; `gvisor` and `microvm` per §56 |

---

## 4. Why each engine (D1900)

| | RisingWave | Flink SQL | Spark |
|---|---|---|---|
| What users get | A streaming database: Postgres wire, `CREATE MATERIALIZED VIEW`, sources and sinks, incremental joins and windows with watermarks | Their existing Flink SQL jobs, unchanged: Flink's dialect, its connectors, savepoints | Batch DataFrames and Spark SQL from PySpark, with no cluster to run |
| Why Loams needs it | Loams does not do stateful stream processing (§00 §7, D341). RisingWave is the companion it chose (D22) | Teams with Flink SQL in production will not port to Postgres-dialect SQL to try Loams. D212's port-only answer loses them | Spark is the batch API that teams know. Sail runs PySpark on DataFusion with no JVM (D211, D1262) |
| Engine | RisingWave v3.1.0 (Rust, Apache-2.0) | Apache Flink 2.2.1 (Java, Apache-2.0) | Sail 0.7.2 (Rust, Apache-2.0) by default; Apache Spark 4.1.3 (Apache-2.0) opt-in |
| Protocol users see | Postgres wire | Flink SQL Gateway REST (Flink's SQL Client) and Loams' API | Spark Connect |
| Overlap | RisingWave and Flink SQL overlap: both do streaming SQL. Loams recommends RisingWave for new work (Postgres wire, no JVM, scale to zero) and Flink SQL for existing Flink SQL. The porting guide of §26 §10.1 stays | | Spark does not overlap the House: the House serves ClickHouse-dialect queries and Spark runs programs. Both read the same Iceberg tables |

**What changes from earlier designs.** D212 put Flink SQL on RisingWave only. D1900 adds managed Flink SQL on Apache Flink. Running a JVM engine is opt-in, as D211 already allowed for Spark, so "no JVM in the default deployment" (§68 §2.2) holds. D211's Kubeflow Spark Operator clause is dropped (D1911): a driver pod in client mode needs no operator.

---

## 5. Licences and pins (D1901, D1931)

### 5.1 The licence check

| Component | Licence | Use | Out of scope |
|---|---|---|---|
| RisingWave | Apache-2.0 | Unmodified image | **Premium features** behind a licence key ([premium features](https://docs.risingwave.com/get-started/premium-features)): secret management (`CREATE SECRET`), time travel, database isolation, resource groups, auto schema change for Postgres and MySQL CDC, the Glue catalog for Iceberg, the RisingWave Console, elastic disk cache and others. Loams never sets `RW_LICENSE_KEY`. The free trial that turns these on for clusters of at most 4 RWU is a trap: a pipeline that uses them stops working when the instance grows. So Loams' generators never emit them, and the conformance suite asserts that `SELECT rw_test_paid_tier()` errors |
| Apache Flink, Flink Kubernetes Operator | Apache-2.0 | Official image plus Loams' jar layer | Ververica Runtime and Platform, Confluent Cloud for Apache Flink, any vendor connector |
| Flink connectors: Kafka 5.0.0, JDBC 4.1.0, Flink CDC 3.7.0 `postgres-cdc`, `iceberg-flink-runtime-2.2` 1.12.0, `iceberg-aws-bundle`, Hadoop client 3.4.2 | Apache-2.0 | Jars by checksum (`deploy/flink/connector-jars.toml`) | |
| PostgreSQL JDBC driver 42.7.14 | BSD-2-Clause | Jar | |
| Apache Spark 4.1.3, `iceberg-spark-runtime-4.1_2.13` 1.12.0, `hadoop-aws` 3.4.2 | Apache-2.0 | Official image plus jars | Databricks Runtime, Photon, Unity Catalog service |
| Sail 0.7.2 (`pysail`), `pyspark-client` 4.1.3 | Apache-2.0 | Loams-built image (Sail publishes none) | |
| Lakekeeper | Apache-2.0 | Unmodified image | |
| Spark Connect protos (vendored from Spark v4.1.3) | Apache-2.0 | Generated code in `loams-spark` with `VENDORED.md` and the licence file | |

No AGPL, BSL, FSL, SSPL or ELv2 code is read or copied (wave rule). The lanes read upstream docs and released sources and do not clone repositories.

### 5.2 Pins (D1931)

| Pin | Version | Released | Recorded by |
|---|---|---|---|
| RisingWave image | v3.1.0 (v3.0.5 of 2026-10-12 also qualifies) | 2026-09-21 | RW-a (D1958) |
| Flink image | `flink:2.2.1-scala_2.12-java21` (2.3.0 is not chosen: the Iceberg and connector lines target 2.2) | 2026-05-15 | FL-a (D1970) |
| Flink connector bundle | as §5.1 | ≥14 days | FL-b (D1986) |
| Flink Kubernetes Operator | 1.16.1 | 2026-09-17 | FL-a |
| Apache Spark | 4.1.3 (not 4.2.0: Iceberg 1.12.0 ships Spark runtimes for 4.0 and 4.1 only) | 2026-07-15 | SP-a |
| Sail | 0.7.2 | 2026-09-29 | SP-a |
| Lakekeeper | v0.13.6 | 2026-09-22 | SP-a (moves to `compose.lake.yaml`) |

---

## 6. Architecture

### 6.1 One control plane, stateless adapters (D1902)

```
 client (psql, Flink SQL Client, PySpark, loams CLI, console)
        │  TLS, Loams API key or token
        ▼
 loams-gateway ── pg (risingwave routes) ── engines http (Flink SQL Gateway, web UI) ── spark (Spark Connect)
        │                                  │                                          │
        ▼                                  ▼                                          ▼
 RisingWave frontend               Flink SQL Gateway → JobManager             Sail / Spark Connect server
 meta·compute·compactor            TaskManagers                               (driver, workers)
        │                                  │                                          │
        ├── meta store: Loams Postgres     ├── checkpoints, savepoints, HA: S3        ├── spark-events, scratch: S3
        ├── Hummock: S3                    │                                          │
        └──────────────┬───────────────────┴───────────────┬──────────────────────────┘
                       ▼                                   ▼
           Lakekeeper (Iceberg REST, `loams`)      Iggy Kafka gateway · Loams Postgres (CDC)
                       ▼
                 House (reads Iceberg)

 loams-engines (facade, store, phase machine, authz, metering) ──► adapters: loams-risingwave · loams-flink · loams-spark
                                                                     └► compose services or k8s objects
```

- `loams-engines` (ENG-UI) owns:
  - the instance record and its store (`MemoryStore` now, a Postgres store next);
  - ids and validation;
  - the phase machine (D2012);
  - authorisation (§10.3);
  - the HTTP facade (§12.1);
  - the 60-second metering ticker (§11).
- An adapter implements `ManagedEngine`: `validate`, `create`, `scale`, `upgrade`, `suspend`, `resume`, `delete`, `observe` and `usage`, plus the optional `sql()` and `spark()` surfaces. It reads the `Instance` and talks to the runtime: compose services in single-node mode, Kubernetes objects in a cluster. It reports what it observed and keeps no state of its own. The two exceptions are FK1's pipeline records (D1988), which the facade's store holds through the adapter's `JobStore` trait, and SP1's session table (D1995).
- The registry (D2014) holds one adapter per kind. A kind whose profile is off answers 501 `engine_not_enabled`.

### 6.2 Tenancy and naming (D1903)

- An instance belongs to **one namespace**: the OpenFGA `environment`, the unit §63's `tenantid` (`<org>/<namespace>`) and the stream API (`/v1/namespaces/{ns}/…`) already use. The code lanes first keyed instances by project. That is overruled (R2) because metering, authorisation and the catalog's `ns_<id>` are all per namespace.
- `id = eng_<26 lowercase base32>`. `name` is a DNS label of 1–40 characters, unique per namespace and kind. A namespace may have several instances, for example a `default` and a `backfill` RisingWave.
- Runtime names come from the id: the k8s namespace `eng-<26>`, the RisingWave meta database and role `rw_<26>`, slot names `loams_<rw|fk>_<26>_<n>`, the Iggy user `eng_<26>`, and the service account `service_account:eng_<26>`.
- The prefix is `state_prefix = engines/<ns>/<id>/`.
- Sizes are ENG-UI's tiers (D2011): `xs`, `s`, `m`, `l` and `xl` per replica. Each adapter maps a tier to its own layout in one table (R19): RisingWave roles, Flink TaskManagers and slots, Sail workers or Spark executors. `config` overrides individual values the adapter validates.

---

## 7. Deployment modes (D1906)

### 7.1 Compose (preview, single-tenant)

```
docker compose -f compose.yaml -f compose.lake.yaml -f compose.risingwave.yaml \
  --profile rustfs --profile kafka --profile risingwave up -d
```

| File | Owner | Services |
|---|---|---|
| `deploy/release/compose.lake.yaml` | RW-a writes it; FL-a and SP-a append under their own comment blocks | `lakekeeper`, `lakekeeper-migrate`, `engines-init` (buckets `loams-engines` and `loams-lake`, Postgres databases and roles, the Lakekeeper warehouse `loams`), `engine-gateway` (`loams-gateway` with the engine routes) |
| `compose.risingwave.yaml` | RW-a | meta, frontend, compute, compactor (distributed mode, D1950), profile `risingwave` |
| `compose.flink.yaml` | FL-a | `jobmanager`, `taskmanager`, `sql-gateway`, `flink-init`, profile `flink` |
| `compose.spark.yaml` | SP-a | the proxy (or `engine-gateway`'s `spark` module) and Sail, profile `spark`; the Apache Spark Connect server with profile `spark-jvm` (`LOAMS_SPARK_BACKEND=sail\|spark`) |

Rules:
- **Every engine profile needs `rustfs`.** The light stack's local-folder bucket is not S3, and the engines' state contract is S3 everywhere, so `engines-init` refuses to start without RustFS. Streams need `kafka`; CDC needs the Postgres stack, which is on by default.
- **No host ports** except `engine-gateway` on `127.0.0.1`: 4566 (RisingWave pg wire), 8083 (the first Flink instance's SQL Gateway REST) and 15002 (Spark Connect). This follows the release compose's rule that published ports are loopback unless `.env` says otherwise. Engines, Lakekeeper and the gateway share an internal `lake` network; engines reach Loams Postgres and Iggy on the default network.
- Meta databases are on Loams Postgres's direct endpoint `compute1:55433`, as DX1's ruling R0.7 does for Temporal and Airflow: RisingWave's meta node needs a session, not a transaction pooler.
- Images are pinned by digest in each lane's file. Telemetry is off (`ENABLE_TELEMETRY=false` for RisingWave).
- Compose is **one trust domain**: one user's install. Lakekeeper runs with authentication off on the internal network. The engine-gateway still checks a Loams key on every way in.

### 7.2 Kubernetes

- Each instance gets a namespace `eng-<26>` holding:
  - the workloads;
  - a ServiceAccount;
  - a NetworkPolicy that admits ingress only from pods labelled `app.kubernetes.io/name: loams-gateway` and the engine controller, and allows egress only to Loams Postgres, the bucket endpoint, Lakekeeper, the Kafka gateway, OTLP and DNS (never `169.254.169.254`);
  - a ResourceQuota from the size;
  - the sandbox profile of §10.5.
- RisingWave uses the Helm chart `deploy/helm/loams-risingwave`, one release per instance with values rendered by the adapter (D1958). Loams uses its own chart rather than the RisingWave operator: one chart is simpler to pin and review, and the operator adds a CRD that Loams would have to track **(estimate)**.
- Flink uses the Flink Kubernetes Operator 1.16.1: a `FlinkDeployment` in session mode, plus a Deployment for the SQL Gateway pointed at the JobManager's REST endpoint (D1972 as amended). Kubernetes HA is on (D1974).
- Spark uses a Sail server Deployment in `kubernetes-cluster` mode, or an Apache Spark Connect server with `spark.master=k8s://…` in client mode, plus driver pods for jobs (D1990, D2001). The proxy is `loams-gateway --module spark`.
- **`LoamsCompanion`** (D1267) is the operator kind these become in OC2. The adapters' render functions are written so the operator can call them. Until OC2, the facade applies the objects through a `ClusterApi` seam.
- Exposure is `internal` or `tailnet` only (D1195). No Service of type `LoadBalancer` or `NodePort`, and no Ingress.

### 7.3 BYOC

- **VM tier** (§70): the compose files of §7.1 on the user's VM, reached over the tailnet. The bucket is the BYOC bucket, and the S3 keys are fetched by the BY1 secrets fetcher.
- **Cluster tier:** the k8s objects of §7.2 in the user's cluster.
- Neither mode makes anything public (D1359).
- Confluent Cloud (§80) can be the Kafka endpoint in place of the Iggy gateway for RisingWave and Flink sources (mode c of D1820) **(verify per engine)**. Confluent's own Flink is not used.

---

## 8. Storage

### 8.1 The bucket layout (D1907)

```
s3://loams-engines/engines/<ns>/<id>/
  hummock/          RisingWave state store (Hummock)
  backup/           RisingWave meta backups
  checkpoints/      Flink checkpoints (retained, 3 kept)
  savepoints/       Flink savepoints (CANONICAL on stop and upgrade)
  ha/               Flink Kubernetes HA storageDir
  recovery/         Flink compose recovery record (jobs.json)
  spark-events/     JVM Spark event logs (rolling)
  scratch/          spark.sql.warehouse.dir, Sail shuffle and checkpoints
  jobs/             uploaded job files (PySpark scripts, SQL files, jars)
s3://loams-lake/                Lakekeeper warehouse `loams` (Iceberg data and metadata)
```

- Every instance gets S3 credentials scoped to `engines/<ns>/<id>/*`, minted through the `ObjectStoreProvisioner` seam (D1952). In compose the RustFS admin keys stand in (one trust domain); in k8s the bucket's IAM or RustFS policies are scoped per prefix. Engines read and write Iceberg data only with the credentials Lakekeeper vends.
- No engine sets bucket-wide lifecycle rules (`RW_DO_NOT_CONFIG_BUCKET_LIFECYCLE=true`). Old checkpoints are removed by the engines' own retention.
- `delete` removes the prefix only with `purge=true`. The default delete keeps state for 7 days (Q1404), so that a mistaken delete can be undone by creating an instance from the same prefix.

### 8.2 RisingWave (D1908)

- **Meta store:** `--backend postgres` on a database and role per instance, `rw_<26>`, on Loams Postgres's direct endpoint, created through the `MetaDbProvisioner` seam: pg-control's role and database RPCs in production, the compose admin role locally (D1951). RisingWave splices the password into a URL without encoding it, so generated passwords use `[A-Za-z0-9]` only. That RisingWave's meta node runs on the Loams Postgres fork (a Neon fork) is **(verify)**: RisingWave tests vanilla Postgres (§68 §6.4).
- **State:** `hummock+s3://loams-engines` with `RW_DATA_DIRECTORY=engines/<ns>/<id>/hummock`, and `RW_S3_ENDPOINT` with path style for RustFS (D1952).
- **Backups:** `RW_BACKUP_STORAGE_URL` and `…_DIRECTORY` under `backup/`. A meta backup is taken before every upgrade and daily.
- **Mode:** distributed, with meta, frontend, compute and compactor at every size (D1950), so that scaling and upgrades run the same code at every size. Standalone and single-node modes are not used.

### 8.3 Flink (D1909, D1910)

- **Session cluster per instance:** one JobManager (two with k8s HA), N TaskManagers with slots equal to vCPUs, and a SQL Gateway. Every statement goes through the SQL Gateway REST API. The client speaks v3 and negotiates up to v4 for `scripts` (D1971).
- **Pipelines:** a pipeline is a long-running `INSERT` or `STATEMENT SET`. The store keeps its SQL text with secret refs (D1985), its job id and its last savepoint. Resume and upgrade re-run the session bootstrap, then the SQL with `execution.state-recovery.path`.
  - **No `COMPILE PLAN` in FK1.** A compiled plan serialises temporary tables with their options, credentials included, and cross-minor upgrades of stateful SQL are not promised in FK1 (Q1405).
  - **No catalog store.** Catalogs are session-scoped and recreated by the bootstrap (D1981), so Flink never persists a credential.
- **State backend:** RocksDB with incremental checkpoints every 60 s, `EXACTLY_ONCE`, `RETAIN_ON_CANCELLATION`, 3 retained, and exponential-delay restarts. Savepoints are CANONICAL for stop and upgrade (portable across versions) and NATIVE on request (D1975).
- **S3:** `flink-s3-fs-presto` for checkpoints, with `s3.endpoint` and path style for RustFS. Credentials go only in env, never in the rendered `config.yaml`. Both the 2.x keys (`execution.checkpointing.dir`, `…savepoint-dir`) and the legacy keys are rendered (D1973).
- **HA:**
  - In k8s, `high-availability.type: kubernetes`, with `storageDir` under `ha/` and a Role for ConfigMaps and leases.
  - In compose, Flink has no filesystem HA service, so there is one JobManager with HA off. The controller writes a recovery record (`recovery/jobs.json`: the statement plus the last checkpoint or savepoint per job) and replays it through the SQL Gateway after a JobManager restart (D1974). This is Loams' recovery, not Flink's HA, and the docs say so.
- **Protected keys:** `SET` and `RESET` of the checkpoint and savepoint directories, the state backend, HA and `execution.state-recovery.path` are refused at the gateway (D1977), so a user cannot point state outside the instance's prefix.

### 8.4 Spark (D1911, D1912)

- **Sessions:** Spark Connect, with backend `sail` (default) or `spark`. Each instance has its own backend (Sail runs tenant Python in-process). After 15 minutes with no session the instance suspends, and the first call resumes it, waiting up to 120 s (D1995).
- **Jobs (D2000–D2004):**
  - A PySpark or SQL job runs in a short-lived driver container against the instance's Spark Connect endpoint (§26 §9.2).
  - A jar job runs as `spark-submit --deploy-mode client` in a driver pod that launches its own executors, so there is no Kubeflow Spark Operator.
  - Runs are retried by policy and scheduled through the embedded Resonate server's `schedule.create`.
  - Job files are uploaded under `jobs/`.
- **Event logs:** JVM Spark has `spark.eventLog.enabled=true` and `spark.eventLog.dir=s3a://loams-engines/engines/<ns>/<id>/spark-events/`, with rolling on. A History Server per instance is optional (SP2) and read-only behind the gateway. Sail writes no Spark event log; its logs and traces go to OTLP and the run's events.
- **Warehouse:** there is no Hive metastore. `spark.sql.warehouse.dir` points at `scratch/` and holds no tables. `spark.sql.defaultCatalog=loams`, so every table is an Iceberg table in Lakekeeper (D1996, D2005).

---

## 9. Catalogs and data paths

### 9.1 Lakekeeper, catalog `loams` (D1913)

| | RisingWave | Flink | Spark |
|---|---|---|---|
| Catalog | Iceberg sink and source with `catalog.type = 'rest'` (RW-b A6) | `CREATE CATALOG loams WITH ('type'='iceberg','catalog-type'='rest', …)` in the session bootstrap (D1981, D1982) | `spark.sql.catalog.loams=org.apache.iceberg.spark.SparkCatalog`, `type=rest` (JVM); `SAIL_CATALOG__LIST` with iceberg-rest (Sail) (D1996) |
| URI | `http://lakekeeper:8181/catalog` in compose; the cluster's Lakekeeper Service in k8s | same | same |
| Warehouse | `loams` (bucket `loams-lake`) | same | same |
| Credentials | Vended (`X-Iceberg-Access-Delegation: vended-credentials`). OAuth2 client credentials of the instance's Lakekeeper client in k8s; none in compose (auth off on the internal network) | same | same |
| Namespace | `ns_<id>[.<db>]` | `base-namespace = ns_<id>`, so Flink database `d` is `ns_<id>.d` | `ns_<id>.<db>` |

- The name `loams` is the same in all three engines (R26). FL-b's first draft used `loams_lake`. Flink's JDBC catalog for Loams Postgres is `loams_pg`.
- **One writer class** (§17 §6.1): engines write their own tables and never tables Loams maintains (links, collections).
- In k8s, Lakekeeper keeps its own Postgres (D413), uses Zitadel OIDC (§76) and authorises with OpenFGA, which it supports natively **(verify the version)**. In compose its database is on Loams Postgres (`lakekeeper` database). That is a compose-only simplification, and it does not amend D413.

### 9.2 The House (D1914)

The House reads engine tables as Iceberg: an engine writes `ns_<id>.<db>.<table>` through Lakekeeper, and the House sees it as a lake table (SP-b D2005, §75 §3). Rules:
- **No engine joins `house-link`.** That network is the engine's and the House's alone, and the engine trusts it.
- No ClickHouse sink into the House by default. RisingWave's ClickHouse sink, if a user wants it later, goes through `loams-gateway`'s `http` route with the instance's credentials (RW2).
- **Single node:** the House uses its local SQLite catalog, so in compose the House does not see engine tables until it can use Lakekeeper as a catalog **(verify against HE1/HS1)**. The compose guide says so.
- **Iceberg versions:** the House writes Iceberg v3 (§75). The engines' Iceberg 1.12.0 reads v3 metadata **(verify deletion vectors per engine)**. Engines write v2 unless a table is created as v3, and the conformance round trip (§14) checks both directions.

### 9.3 Streams through the Iggy Kafka gateway (D1915)

- Sources and sinks use `connector='kafka'` (RisingWave, Flink, and Structured Streaming on JVM Spark only) at `kafka-gateway:9093`, with SASL/PLAIN. That is `SASL_PLAINTEXT` on the compose network and `SASL_SSL` in k8s.
- **Authentication:** the instance's own Iggy user `eng_<26>`, with permissions on its namespace's streams only (D1917), not the user's key. Iggy's gateway checks Iggy users today (`KAFKA_SASL_ENABLED`). When it maps Loams keys (§32 §5.10), the user becomes the instance's service-account key, and nothing else changes.
- **Delivery:** the gateway has no transactions (D1797), so Flink's Kafka sink is pinned to `sink.delivery-guarantee = at-least-once` (the builder refuses exactly-once, D1983), and RisingWave's Kafka sink is at-least-once. Exactly-once end to end holds only into Iceberg: Flink commits on checkpoint, and RisingWave's Iceberg sink commits every `commit_checkpoint_interval`.
- **Schema registry:** none by default. JSON with explicit columns; Avro or Protobuf with a registry only when the user names one (RW-b A4).
- **Confluent:** a Confluent cluster (§80 mode c) is a valid Kafka endpoint for sources and sinks, with its own credentials. Confluent's Flink is not used.

### 9.4 Loams Postgres CDC (D1916)

- **RisingWave:** `postgres-cdc`, one shared source per database, then `CREATE TABLE … FROM src TABLE 'schema.table'` (RW-b A5).
- **Flink:** Flink CDC's `postgres-cdc` with incremental snapshot and pgoutput (D1984).
- **Spark:** no CDC path. It reads Iceberg tables, or Postgres over JDBC for batch.
- **The endpoint** is the direct compute endpoint, not PgDog: replication needs a session and the replication protocol. TLS is `require`.
- **The controller creates the publication** (RisingWave's `publication.create.enable = 'false'`) under a CDC role per source with `REPLICATION` and `SELECT` on the published tables only. The engine never holds an owner or superuser role.
- **Slots** are named `loams_<rw|fk>_<26>_<n>`. They are dropped on source drop and on instance delete. An orphan slot holds WAL and costs storage, so the facade lists the slots it owns and `observe()` reports lag.
- **Not checked yet:**
  - **(verify)** that logical replication works on the Loams Postgres fork: `wal_level = logical`, slots that survive a compute restart, and the behaviour at scale to zero.
  - **(verify)** whether an active slot keeps a compute from suspending, and what a compute restart does to an engine's replication connection. The fork's behaviour is not yet checked.
  - The plans run these checks first (RW1 Task 8, FK1 Task 7). Until they pass, CDC is labelled **experimental** within preview.
- Auto schema change for CDC is RisingWave Premium. Loams does not use it: schema changes need a new table in RisingWave, and the guide says so.

---

## 10. Security

### 10.1 Credentials (D1917)

- **A service principal per instance:** `service_account:eng_<26>`, created on instance create and deleted with the instance. The facade mints these scoped credentials for it:

| For | Credential | Scope |
|---|---|---|
| Kafka | Iggy user `eng_<26>` | the namespace's streams |
| Lakekeeper | OAuth2 client (k8s); none (compose) | the namespace's Iceberg namespaces, through OpenFGA |
| Postgres CDC | role `cdc_<26>_<n>` | `REPLICATION`, `SELECT` on published tables |
| Postgres meta | role `rw_<26>` | owner of its meta database only |
| S3 | keys | `engines/<ns>/<id>/*` |

- **A user's API key is never written** into engine SQL, configuration, catalogs or savepoints. A user who creates a RisingWave source writes no password: the generators (RW-b, FL-b) fill in the instance's credentials.
- **RisingWave:** `CREATE SECRET` is Premium, so connectors carry the scoped credentials inline (`Credential::Inline`, the default). RisingWave's catalog stores DDL text, so anyone with the instance's SQL access can read them. They grant only what the namespace's own principals already hold, and one instance serves one namespace (D1904). `Credential::Secret` stays in RW-b's code as a non-default variant documented as Premium (R5).
- **Flink:** builders take secret refs (D1985) that resolve at session open, and every rendered script has a redacted form for logs, API echoes and the CLI. Saved pipelines persist refs only.
- **Spark:** catalog and S3 credentials go in the server's environment, never in a conf file. The proxy strips the client's credential and sends the backend's own pre-shared token (`SPARK_CONNECT_AUTHENTICATE_TOKEN` on Apache Spark; Sail has none and is reachable only from the proxy) (D1993).
- **Delivery:** credentials reach processes through env or mounted files from the secret store, never argv.

### 10.2 Access through `loams-gateway` (D1918)

| Engine | Way in | Gateway behaviour |
|---|---|---|
| RisingWave | pg wire, `<instance>.<ns>.<region>.rw.<base-domain>:5432` (compose `127.0.0.1:4566`) | A `risingwave` route section in the routes file. The gateway terminates TLS (RisingWave sits behind it, not PgDog), asks for a cleartext password inside TLS, and checks it as a Loams API key or token with `can_connect` on the instance. It then logs in upstream as the instance's service user (MD5) and relays (D1957). This is a new path in the `pg` module: §57's splice keeps PgDog's key with PgDog, and here there is no PgDog |
| Flink | Raw Flink SQL Gateway REST, `https://<instance>.<ns>.<region>.flink.<base-domain>/v{2,3,4}/…` (compose `127.0.0.1:8083`). The web UI is read-only on the same host | An `engines` HTTP route with bearer auth. Session handles are bound to the principal and instance that opened them, and a foreign handle is a 404. `scripts` outside the instance's prefix get 403, and protected `SET` keys get 403 (D1977). Routing is by hostname, not by a path prefix, because the stock SQL Client takes an endpoint with no prefix (R25). The web UI runs with `web.submit.enable=false` and `web.cancel.enable=false` |
| Flink (Loams' API) | `/v1/namespaces/{ns}/engines/{id}/flink/…` on the facade | As every facade route |
| Spark | Spark Connect, `sc://<instance>.<ns>.<region>.spark.<base-domain>:443/;use_ssl=true` (compose `sc://127.0.0.1:15002`) | The `spark` module (D1263; SP-a's library). Bearer auth or `x-loams-api-key`, with the credential's instance checked against `:authority`. The proxy reads only `session_id` and `user_context` and sets `user_context.user_id` to the Loams subject (D1994). Limits (8 sessions, 16 operations per instance) and idle release (D1995) |

- Nothing else is reachable: no RisingWave meta dashboard, no JobManager REST and no Sail port.
- The keys files in FL-a's and SP-a's first drafts (D1978, D1993) are test seams. Production routes verify Loams keys and tokens through the same verifier as the rest of `loams-gateway` and check §10.3's relations.
- Certificates: a wildcard per namespace and engine (`*.<ns>.<region>.rw.<base-domain>`); one certificate in compose (one trust domain).

### 10.3 Authorisation (D1919)

Added to `deploy/openfga/core.fga`, alongside `database`:

```
type engine
  relations
    define environment: [environment]
    define can_view: viewer from environment
    define can_operate: editor from environment
    define can_admin: deployer from environment
    define can_connect: viewer from environment
```

| Action | Relation |
|---|---|
| list, get, usage, observe | `can_view` |
| scale, suspend, resume, SQL, Flink sessions and pipelines, Spark sessions and jobs | `can_operate` |
| create, delete, upgrade, purge | `can_admin` |
| the data path through the gateway (pg wire, Flink REST, Spark Connect) | `can_connect` |

Until the facade writes `engine:<id>#environment@environment:<ns>` tuples, the same checks run on `environment:<ns>` as `can_read`, `can_write` and `can_admin`. ENG-UI first proposed new relations on `project` (D2016). Those are not added (R18), so the project type keeps D1603's shape. The built-in authorizer (the default without OpenFGA) gets the same mapping.

### 10.4 Multi-tenancy (D1904)

- One instance means its own processes, its own meta database, its own prefix, its own credentials and, in k8s, its own namespace and NetworkPolicy.
- **No shared clusters.** RisingWave's database isolation and resource groups are Premium. A Flink session cluster runs every job's user code in shared TaskManagers. Sail runs Python UDFs in-process. A shared cluster per engine would trade isolation for a smaller memory floor, and Q1035's shared-cluster option is withdrawn for managed mode. The cost of that choice is RisingWave's and Flink's memory floor per namespace (about 2–4 GiB for the smallest instance, **(estimate)**), which suspend reduces to storage only.
- Several instances in one namespace share nothing but the namespace's catalog namespace and streams.

### 10.5 The sandbox profile (D1905)

- Every engine pod gets a `loams-sandbox` `SandboxSpec` (§56) rendered to the pod:
  - a non-root uid and gid, all capabilities dropped and `no_new_privs`;
  - `RuntimeDefault` seccomp (the `linux` adapter's own profile where it runs the process directly, as in compose);
  - a read-only root filesystem where the engine allows it, with writable `emptyDir` for spill;
  - no service-account token unless the engine needs one (Flink's Kubernetes HA does);
  - the egress of §7.2.
- **User code** stays off in FK1 and SP1 except where the engine cannot run without it:
  - RisingWave's embedded UDFs (Python, JavaScript, Rust/Wasm): off by default, and external UDF servers refused. Allowed only on nodes with gVisor (RW2, Q1402).
  - Flink UDF jars: not accepted in FK1 (D1930).
  - Spark Python UDFs on Sail: allowed, because Sail runs them and they are core to PySpark. In multi-tenant clusters the Sail pods need the `gvisor` runtime class (D1005's pattern). A node without gVisor is reported, and the facade refuses to place Sail with UDFs there unless the cluster is single-tenant (a BYOC cluster of one customer).
- `SandboxReport` (D1007) for each pod is part of `observe()`'s checks.

---

## 11. Metering (D1920)

| Type | When | Data |
|---|---|---|
| `io.loams.dev.usage.engine.runtime.v1` | Every 60 s per instance not suspended or deleted, and at suspend | `namespace`, `instance`, `engine`, `size`, `cpu_ms`, `memory_mib_ms`, `window_start`, `window_end`, `estimated` |
| `io.loams.dev.usage.engine.storage.v1` | Hourly per instance, suspended ones included (a gauge, §63 §8.3) | `namespace`, `instance`, `engine`, `state_bytes` |

- **The producer is the facade**, from the adapter's `usage()`. It measures cgroups in compose and Kubernetes metrics in k8s, and falls back to the reserved size with `estimated: true` when nothing is measured. The `id` is `<instance>/<window_end_unix>`, `source` is `loams://<cluster>/engines/<node>`, `subject` is `org_<org>`, and `tenantid` is `<org>/<ns>` (§63 §6.1). Values are integers.
- **Adapters never emit events** (D2017).
- **The node agent:** when it covers engine pods (D1268's plan), it emits the same type with the same `id` rule for the same window, and dedupe keeps one. D1268's name `companion.runtime.v1` is replaced: managed engines are §68's companions, and one name is enough.
- **The gate** (§63 §10) is called with the size's reservation on create, resume and scale up. Over a hard limit it refuses, and a running instance that crosses one is **suspended** with a savepoint for Flink. It is never deleted.
- **Other usage keeps its own types:** gateway connections (`gateway.connection.v1`), Kafka bytes through Iggy, Lakekeeper calls and Postgres compute for the meta database.
- **Prices** are not set in this design (Q1403). Preview meters and shows usage, and charges nothing.

---

## 12. Surfaces

### 12.1 API (D1921)

ENG-UI's facade (D2015), mounted at `/v1` with the namespace path of D1903:

| Route | What |
|---|---|
| `GET /v1/engines` | Kinds, versions, sizes, capabilities, `stability: "preview"` |
| `GET\|POST /v1/namespaces/{ns}/engines` | List (`?kind=`) and create (202 with the `Instance`) |
| `GET\|PATCH\|DELETE /v1/namespaces/{ns}/engines/{id}` | Get; PATCH `{size}` or `{version}`; delete (`?purge=true`) |
| `POST …/{id}/suspend`, `POST …/{id}/resume` | Lifecycle |
| `GET …/{id}/usage?from=&to=` | The samples the facade emitted |
| `POST …/{id}/sql` | One statement batch (RisingWave, Flink), with `max_rows` and `timeout_ms` |
| `…/{id}/flink/sessions`, `…/sessions/{s}/statements[/{st}/results]`, `…/{id}/flink/jobs[/{j}/start\|stop\|resume]` | Flink (FL-b D1988). Results stream as NDJSON with changelog kinds |
| `GET\|POST …/{id}/sessions`, `DELETE …/sessions/{s}` | Spark sessions (returns `connect_url`) |
| `GET\|POST …/{id}/jobs`, `POST …/jobs/{j}/cancel`, `GET …/jobs/{j}/logs?attempt=` | Spark jobs (SP-b D2000–D2003) |

- Errors are `{ "error": "<code>", "message": "…" }`, as in the console contract.
- The contract file is `api/engines/openapi.json` (OpenAPI 3.1).
- The SDKs gain `engines` clients after the contract settles. The Python `loams.spark.session()` (D2007) builds a Spark Connect session with the token in gRPC metadata, never in the URI.
- `submit_engine_job` (§26) gains `EngineJob::{StreamingSql, SparkBatch}` targets that call this facade. Airflow's `LoamsStreamingSqlOperator` and `LoamsSparkSubmitOperator` (§68 §4.4) do the same.

### 12.2 CLI (D1922)

- `loams engines list|create|get|scale|upgrade|suspend|resume|delete|usage` works for every kind.
- Per engine:
  - `loams risingwave sql|list|progress|cancel|render|apply` (RW-b A11);
  - `loams flink sql|jobs …|bootstrap|jars` (D1989);
  - `loams spark submit|status|logs|session|schedule` (SP-b).
- All are clients of the facade, and each sits behind a cargo feature (the `loams cloud` pattern). Each prints redacted SQL by default.
- `--url` direct mode (talking to the engine on the compose network) exists for development only and prints a warning.

### 12.3 Console (D1923)

- The Engines pages are a plugin in the console shell (§59):
  - a list per namespace, with create, size, version, phase and health;
  - a SQL workspace on the shared CodeMirror editor in `@loams/ui`, with the Postgres dialect for RisingWave, a Flink SQL mode and Spark SQL;
  - for RisingWave, materialised views, sources, sinks and DDL progress;
  - for Flink, pipelines with start, stop-with-savepoint and resume, plus savepoints;
  - for Spark, sessions and jobs with logs;
  - usage charts.
- Every page carries the **Preview** badge.
- Mock routes in `loams-console-mock` let the pages ship before the facade runs.

---

## 13. Lifecycle, upgrades and failures

### 13.1 Lifecycle (D1924)

```
Pending → Provisioning → Running ⇄ Scaling | Upgrading
Running → Suspending → Suspended → Resuming → Running
any → Deleting → Deleted ;  a failed op → Failed (retry the same op, or delete)
```

- **One operation at a time** per instance. A second one gets `409 conflict`.

| Operation | RisingWave | Flink | Spark |
|---|---|---|---|
| create | Meta database, prefix and credentials; start meta, compactor, compute, frontend | Prefix, credentials, session cluster, SQL Gateway | Backend (Sail or Spark), proxy route |
| scale | Change role replicas and resources (rolling) | TaskManager count and slots (rolling TaskManager restart). Running jobs rescale only on stop and resume, because the adaptive scheduler is off in session mode | Sail worker cap or executors. Running jobs keep their size |
| suspend | Every role to 0, meta included. The meta database and Hummock stay (D1954) | Stop every pipeline with a savepoint, then every replica to 0 | Release sessions and scale the backend to 0 |
| resume | Meta first, then compactor, compute, frontend. RisingWave recovers from the meta store and the object store | Scale up, re-run the bootstrap, restore each pipeline from its savepoint | On the first connection (wait at most 120 s) |
| delete | Workloads, then (with `purge`) the prefix, credentials, meta database and CDC slots | Cancel jobs, workloads, then (with `purge`) the prefix | Backend and routes, then (with `purge`) the prefix |

### 13.2 Upgrades (D1925)

- **Forward only**, within the version list the adapter declares. No downgrade and no skipped major: RisingWave 3.x → 4.x is allowed, 3.x → 5.x is refused (D1955).
- **RisingWave:**
  1. Take a meta backup.
  2. Roll meta, then compactor, compute and frontend, each ready before the next.
  3. On failure, stop and report `Failed{upgrade}`. The meta backup and the old image are the restore path, which is manual in RW1 (Q1406).
- **Flink:**
  1. Stop every managed job with a CANONICAL savepoint (`POST /jobs/:id/stop`) and wait for each to complete. A failed savepoint aborts the upgrade before anything is redeployed.
  2. Redeploy with the new image.
  3. Wait for the expected TaskManagers.
  4. Resubmit each pipeline from its savepoint.
  - A savepoint that does not restore leaves that pipeline `Failed` on the new version with the savepoint kept. The documented rollback is to `upgrade` back to the previous version from the same savepoint (D1976, R23).
  - Minor versions of Flink SQL can change operator state layouts, so the adapter's version list holds only patch upgrades within 2.2 for FK1 (Q1405).
- **Spark:** stateless. New sessions get the new version, and running jobs finish on the old one. Iceberg tables are unaffected.
- **Connectors and Lakekeeper:** a jar bundle or Lakekeeper pin change is a Loams release, with the same conformance gate.

### 13.3 Failure modes (D1926)

| Failure | Effect | Handling |
|---|---|---|
| RisingWave compute node lost | Streaming jobs pause | RisingWave recovers from the last barrier checkpoint in Hummock. `observe()` is `Degraded` until compute is back |
| RisingWave meta lost or restarted | DDL and recovery stall | Meta restarts from Loams Postgres (sessions only, never PgDog). A lost meta database restores from `backup/` |
| Loams Postgres endpoint suspended or moved | RisingWave meta loses its session; CDC disconnects | The meta database's endpoint has scale to zero off (`suspend_timeout = never`) for as long as a running instance uses it. CDC reconnects and resumes from the slot |
| Flink TaskManager lost | Jobs restart | Flink restarts from the last checkpoint (exponential delay) |
| Flink JobManager lost | k8s: HA failover. Compose: jobs gone | k8s: Kubernetes HA. Compose: the controller replays `recovery/jobs.json` from the last retained checkpoint |
| SQL Gateway lost | Sessions lost; jobs keep running | Clients reopen sessions. Pipelines are tracked by job id, not session |
| Savepoint fails on stop or upgrade | The operation aborts | The job keeps running, and the error and the checkpoint path are reported |
| Iggy Kafka gateway down | Sources stall, sinks back off | Engines retry. Offsets are committed by the engines (RisingWave in its state, Flink in checkpoints), so nothing is lost |
| Lakekeeper down | Iceberg commits fail | RisingWave's and Flink's Iceberg sinks retry on the next checkpoint. Spark statements fail |
| Bucket unavailable | Every engine stalls | Engines' own retries. `observe()` is `Unhealthy` |
| CDC slot lag grows | WAL retained, storage cost | Alarm at 1 GiB and 6 h of lag **(estimate)**. At 10 GiB, `observe()` is `Degraded`, and the facade pauses the source if the namespace's storage gate is near its limit |
| Meter hard limit | Admission refused | Suspend, with a savepoint for Flink. Never delete |
| Sail session crash | That session's operations fail | The client retries on a new session. Sail restarts |

Data is never deleted without `purge=true`. Every alarm is a named check in `observe()`, shown in the console and the CLI.

---

## 14. Testing (D1927)

**Upstream suites as oracles.** Engines run unmodified, so the question is whether Loams' wiring (storage, catalog, Kafka, CDC, gateway) breaks anything an engine's own tests pass upstream.

| Engine | Oracle | Where |
|---|---|---|
| RisingWave | A pinned subset of RisingWave's `e2e_test/` sqllogictest files (streaming, batch, sinks, Iceberg, Postgres CDC), run with `sqllogictest-rs` against a managed instance through the gateway. Files are fetched at the pinned tag by the script, not vendored | `conformance/risingwave/` (RW-b A12) |
| Flink | Flink's own SQL end-to-end cases: the SQL Client and SQL Gateway tests at the pinned tag, run against the instance through the gateway with Flink's SQL Client. Plus **Nexmark** (Apache-2.0) queries q0–q22 on Kafka from the Iggy gateway, checkpointing to the bucket. Nexmark is also run on RisingWave as a cross-check | `conformance/flink/` |
| Spark | The PySpark Spark Connect parity tests (`pyspark.sql.tests.connect`) at 4.1.3, against Sail and against Apache Spark, with an expected-failure list per backend (Sail's coverage, §68 §7.2). Plus §68 §7.4's corpus (TPC-H-derived SQL, DataFrames, a `pandas_udf`, Iceberg `MERGE`) | `conformance/compute/spark/` (SP-b D2009) |
| Cross-engine | Iceberg round trips: RisingWave writes, then Spark, the House (cluster mode) and Flink read, and each direction in turn, on v2 and v3 tables. A Kafka round trip: RisingWave produces through the Iggy gateway and Flink consumes. CDC: a Postgres table into RisingWave and Flink, compared with the source | `conformance/engines/` |
| Loams' own | Lifecycle (create, scale, suspend, resume, upgrade, delete with and without purge), state survives suspend and resume, credentials never in argv or logs (log grep), `rw_test_paid_tier()` errors, gateway refusals (wrong namespace, protected `SET`, foreign session handle), the metering event shape | Unit tests in each crate; `conformance/engines/` |

Rules, as in §68 §12 and DX0:
- **Strict `xfail.toml`:** every expected failure has a reason and an issue, and an unexpected pass fails the run.
- **Real checks:** the Loams cases check effects (rows read back, the savepoint restored with the same counts), not exit codes.
- **Runners:** GitHub-hosted runners only, nightly and on `workflow_dispatch`. Per-PR jobs lint, render goldens (compose, k8s objects, configuration, generated SQL) and run unit tests.
- **Builds:** none ran locally in the 2026-10-27 wave (owner rule). CI compiles. The first end-to-end run is each plan's Task "first run".

---

## 15. Status labels (D1928)

| Surface | Label |
|---|---|
| All three engines | **Preview**. `GET /v1/engines` returns `stability: "preview"`, and the console carries a badge |
| Docs (`docs/guides/risingwave*.md`, `flink-sql.md`, `spark.md`) | "Preview. Not yet run end to end" until the plan's first-run task passes, then "Preview" |
| Postgres CDC | "Experimental" within preview until §9.4's checks pass |
| Compose | "A single-user preview stack, not a production deployment" |
| Wording (§68 D1251, D1275) | "Runs RisingWave", "runs Apache Flink", "runs Sail, a Spark Connect server", "runs Apache Spark". Never "RisingWave-compatible", "a Flink replacement" or "Loams Spark" |

---

## 16. Phases and what not to build

### 16.1 Phases (D1929)

- **RW1** ([plan](../plans/2026-10-27-rw1-managed-risingwave.md)): the shared foundation, plus RisingWave.
  - The foundation is `loams-engines`, the facade, OpenFGA `engine`, metering, `compose.lake.yaml`, `engine-gateway`, the console pages and the engines conformance harness.
  - RisingWave is the adapter, compose, Helm, the gateway route, connectors, the SQL API, the CLI and conformance.
- **FK1** ([plan](../plans/2026-10-27-fk1-managed-flink-sql.md)): Flink SQL. The adapter, SQL Gateway and REST clients, compose, k8s, the gateway route, catalogs and connectors, the image, the API and pipelines, the CLI and conformance.
- **SP1** ([plan](../plans/2026-10-27-sp1-managed-spark.md)): Spark. The proxy and gateway module, the Sail and Spark backends, compose, k8s, jobs and schedules, the Python helpers, the CLI and conformance.
- **Exit for each:**
  - the plan's first-run task passes on compose;
  - the conformance suite runs nightly with a committed expected-failure list;
  - the guide is published.
- **Then RW2, FK2, SP2:** UDFs under gVisor; Flink application mode per pipeline and compiled plans for cross-version upgrades; the Spark History Server; `LoamsCompanion` (OC2); the Postgres instance store; Loams Cloud (§78) after its own design.

### 16.2 Not built (D1930)

- No fork of RisingWave, Flink, Spark or Sail. Gaps go to upstream issues with the owner's consent, as with every upstream post.
- No Flink DataStream or JAR jobs in FK1, and no Flink UDF jars. D212's operator path stays for FK2 or §26 J4.
- No Kafka Connect worker. §80 D1835 already excludes it.
- No shared multi-tenant engine cluster (D1904).
- No public endpoint for any engine, its UI or its admin API.
- No RisingWave Premium, Ververica, Confluent or Databricks piece.
- No ClickHouse sink into the House by default (D1914).
- No hosting in Loams Cloud on TiDB X in this phase (Q1408).
- No Loams-written stream or Spark engine (D1261 and D1262 hold).

---

## 17. Relation to earlier decisions

| Earlier | What this design does |
|---|---|
| D22, D341 (RisingWave companion; no Spark or Flink in Flow) | Kept. RisingWave becomes managed, and Flow is unchanged |
| D212 (Flink SQL ported to RisingWave; DataStream on the operator) | **Amended by D1900:** Flink SQL is also managed on Apache Flink. The porting guide stays. DataStream is unchanged and not in FK1 |
| D211 (Sail per namespace; Spark fallback through the Kubeflow Spark Operator) | Sail kept as the default. Apache Spark 4.1.3 Connect becomes a managed backend. **Amended by D1911:** jar jobs use client-mode driver pods, with no operator |
| D1262, D1263 (Sail is Loams' Spark; the `spark` gateway module) | Kept. SP-a's proxy is that module |
| D1265, D413 (one catalog; Lakekeeper on its own Postgres) | Kept. Compose runs Lakekeeper on Loams Postgres as a compose-only simplification |
| D1267 (compose profiles; `LoamsCompanion`; engines with tenant code per namespace, others may share) | Profiles kept, in one file per engine rather than in `compose.compute.yaml`. **Amended by D1904:** managed RisingWave is never shared either |
| D1268 (`companion.runtime.v1`) | **Replaced by D1920's** `engine.runtime.v1` and `engine.storage.v1` |
| Q1035 (RisingWave shared without UDFs) | **Narrowed by D1904:** not in managed mode |
| D408, D1796–D1798 (Iggy's Kafka gateway, no transactions) | Kept. D1915 states the at-least-once consequence |
| D1835 (§80: no ksqlDB or Flink integration with Confluent) | Kept: Confluent's Flink is not used. A Confluent cluster may be a Kafka endpoint |
| D1195, D1311, D1359 (never public) | Kept by D1918 |
| D1603, D1612 (OpenFGA model v1) | Extended with `engine` (D1919) |
| D1005, D1007 (sandbox adapters and reports) | Applied to engine pods (D1905) |
| D1274 (pins at least 14 days old) | Applied (D1931) |

---

## 18. The code lanes' decisions (D1950–D2019)

Declared here so that the decision log has one source. Each row is approved (owner, 2026-10-27) as the lane wrote it, with this design's amendment where one applies. The full text is in `engines-81.md` and the lanes' PRs.

| Range | Lane | Decisions | Amended by |
|---|---|---|---|
| D1950–D1959 | RW-a (controller, deploy) | Cluster model (distributed, four roles), meta backend on Loams Postgres, state store on S3, lifecycle, suspend, upgrade order, probes, gateway route (terminate, Loams key, service user), deploy (`compose.risingwave.yaml`, Helm chart), product defaults (no telemetry, no licence key) | D1903 (namespace + instance, not project), D1907 (prefix), R19 (tiers) |
| D1960–D1969 | RW-b (connectors, SQL API, CLI) | Typed DDL builders and quoting, credentials, Kafka through Iggy, schema registry optional, Postgres CDC, Iceberg sink on Lakekeeper, House sink, Postgres sink, Kafka sink, SQL API over pg wire, CLI, conformance | D1917 (no `CREATE SECRET`, scoped service credentials), D1914 (no House sink), D1916 (the controller creates publications) |
| D1970–D1979 | FL-a (Flink core) | Image 2.2.1, SQL Gateway API v3/v4, session cluster, object storage layout, HA (k8s; compose recovery record), RocksDB state backend, lifecycle and upgrade, gateway guards, auth seam, deploy | D1903, D1907 (prefix and k8s namespace), D1918 (hostname routing, not a path prefix) |
| D1980–D1989 | FL-b (catalog, connectors, API) | Typed Flink SQL builders, session bootstrap, Iceberg on Lakekeeper, Kafka through Iggy, JDBC and `postgres-cdc`, secret refs, connector bundle, image, HTTP API and pipelines, CLI | D1913 (catalog `loams`, not `loams_lake`), D1917 (instance credentials for Kafka), D1921 (namespace paths) |
| D1990–D1999 | SP-a (Connect proxy, backends) | Two backends (Sail, Apache Spark 4.1.3), tenancy, proxy home, auth, transparent forwarding, limits and idle suspend, config rendering, Lakekeeper in compose, deploy (`spark`, `spark-jvm`), the adapter | D1903, D1918 (endpoint per instance), D1913 (Lakekeeper in `compose.lake.yaml`, bucket `loams-lake`) |
| D2000–D2009 | SP-b (jobs, catalog, Python) | Job spec, the two execution paths (no operator), local runner, run state machine, schedules on Resonate, catalog conf, HTTP paths, Python helpers, endpoint, conformance | D1931 (Iceberg runtime 4.1 1.12.0), D1921 (facade paths) |
| D2010–D2019 | ENG-UI (shared crate, console) | Adapter split, size tiers, phase machine, `ManagedEngine` and the surfaces, registry, HTTP facade, authorisation, metering, console, preview labels | D1903 (namespace paths), D1919 (OpenFGA `engine`, not `project` relations), D1920 (event names and integer fields) |

---

## 19. Risks

| Risk | Mitigation |
|---|---|
| Seven lanes landing at once write conflicting shapes | One ruling file (`engines-81.md`) that this design follows. Crate shells take the union of `Cargo.toml` entries, and the decision-log rows are written once (R0) |
| No local builds this wave: crates may not compile | CI is the check. Lanes fix from `gh run view --log-failed`. Nothing is labelled beyond preview |
| RisingWave's meta store on the Neon-based fork misbehaves | RW1 Task 2 runs the meta node's own test path first (verify). The fallback is a plain Postgres 16 container for the meta store in compose, labelled as such |
| Logical replication on the fork, and its interaction with scale to zero | §9.4's checks come first. CDC stays "experimental" until they pass |
| Memory floor per namespace (two JVM engines) | Suspend to storage only. Sizes start at `xs`. A shared cluster only if Q1407 says so |
| Premium creep through RisingWave's free trial | Generators never emit Premium SQL, and the conformance suite asserts `rw_test_paid_tier()` errors |
| Flink state not portable across versions | Patch upgrades only in FK1. CANONICAL savepoints. Rollback from the same savepoint |
| Iggy Kafka gateway gaps (transactions, compaction) | At-least-once, stated. Iceberg for exactly-once. The gateway's parity work continues in the fork (D1797) |
| Sail coverage gaps | The Apache Spark backend, and parity tests with per-backend expected failures |
| Credentials readable in RisingWave's catalog | Scoped to what the namespace already holds, one namespace per instance, rotation on demand (`loams engines rotate-credentials`, RW1 Task 10) |

---

## 20. Open questions (Q1400–Q1411), answered 2026-10-27

| # | Question | Default |
|---|---|---|
| Q1400 | Approve managed Flink SQL on Apache Flink, amending D212 (Flink SQL ported to RisingWave only)? | Yes; the porting guide stays. **Answered 2026-10-27 by the owner's request ("start rising wave managed flink sql and spark"): yes** |
| Q1401 | Should the Apache Spark backend be in SP1 or wait for SP2? | **In SP1, opt-in** (`spark-jvm` profile), because Sail's 4.x coverage is about 71 % (§26 §9.1). **Answered (owner, 2026-10-27): the default** |
| Q1402 | RisingWave embedded UDFs (Python, JavaScript, Wasm): when? | **RW2**, only on gVisor nodes or single-tenant clusters; off in RW1. **Answered (owner, 2026-10-27): the default** |
| Q1403 | Prices for `engine.runtime.v1` and `engine.storage.v1`? | **None in preview**: meter and show usage, charge nothing. **Answered (owner, 2026-10-27): the default** |
| Q1404 | How long does a non-purge delete keep an instance's prefix? | **7 days**, then a sweeper purges it. **Answered (owner, 2026-10-27): the default** |
| Q1405 | Cross-minor Flink upgrades of stateful SQL (compiled plans with `table.plan.compile.catalog-objects = IDENTIFIER`)? | **FK2**; FK1 allows patch upgrades within 2.2 only. **Answered (owner, 2026-10-27): the default** |
| Q1406 | RisingWave upgrade rollback: automated restore from the meta backup, or manual? | **Manual in RW1** (documented runbook), automated in RW2. **Answered (owner, 2026-10-27): the default** |
| Q1407 | A shared cluster for very small namespaces to cut the memory floor? | **No**; revisit if suspended-instance resume latency or cost complaints appear. **Answered (owner, 2026-10-27): the default** |
| Q1408 | Hosting managed engines in Loams Cloud on TiDB X (§78)? | **Not in this phase**; self-hosted compose and BYOC only. **Answered (owner, 2026-10-27): the default** |
| Q1409 | FLIP-480 script deployment (`/sessions/:h/scripts`, application mode) instead of the session cluster, when the pinned Flink supports it? | **Evaluate in FK2**; FK1's client negotiates v4 but does not use it by default. **Answered (owner, 2026-10-27): the default** |
| Q1410 | A Spark History Server per instance (JVM backend)? | **SP2**, optional, read-only behind the gateway. **Answered (owner, 2026-10-27): the default** |
| Q1411 | Should the Lakekeeper pin move to v0.14.x (v0.14.0 of 2026-10-07 is eligible)? | **Yes in RW1 Task 1** once its digest is recorded and the compose round trip passes; v0.13.6 until then. **Answered (owner, 2026-10-27): the default** |

---

## 21. Sources

Read on 2026-10-27.
- RisingWave: [releases](https://github.com/risingwavelabs/risingwave/releases) (v3.1.0 of 2026-09-21, v3.0.5 of 2026-10-12); [premium features](https://docs.risingwave.com/get-started/premium-features) (the Premium list and the 4 RWU trial); [Premium edition](https://docs.risingwave.com/get-started/rw-premium-edition-intro) (`RW_LICENSE_KEY`, `rw_test_paid_tier()`).
- Apache Flink: [downloads](https://flink.apache.org/downloads/) (2.3.0 of 2026-06-25, 2.2.1 of 2026-05-15, 1.20.5 LTS; Kafka connector 5.0.0; JDBC connector 4.1.0; Flink CDC 3.7.0; Kubernetes Operator 1.16.1); [FLIP-480](https://cwiki.apache.org/confluence/display/FLINK/FLIP-480%3A+Support+to+deploy+SQL+script+in+application+mode) (accepted; release status not confirmed).
- Apache Spark: [downloads](https://spark.apache.org/downloads.html) (4.2.0 of 2026-07-14, 4.1.3 and 4.0.4 of 2026-07-15); [Spark Connect overview](https://spark.apache.org/docs/latest/spark-connect-overview.html).
- Sail: [releases](https://github.com/lakehq/sail/releases) (0.7.2 of 2026-09-29).
- Lakekeeper: [releases](https://github.com/lakekeeper/lakekeeper/releases) (v0.14.0 of 2026-10-07, v0.13.6 of 2026-09-22).
- The lanes' log: `loams-wt/.he0-shared/engines-81.md` (outside the repository), rulings R0–R30 and the lanes' sections, 2026-10-27.
- In this repository: `deploy/release/compose.yaml`, `deploy/release/compose.compute.yaml`, `deploy/loams-postgres-dev/compose.yaml`, `deploy/openfga/core.fga`, `crates/loams-gateway/src/`, designs §02, §17, §26, §32, §56, §57, §58, §63, §65, §68, §75, §76, §80.
