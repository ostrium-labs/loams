# 68 — Open-Protocol Compute: Airflow, Temporal, Streaming SQL, Spark and Job Queues on Loams

Status: **Approved** · 2026-10-21 (written 2026-10-20; the owner approved every decision and question with its default on 2026-10-21). Source: the owner's request of 2026-10-20, **"write the design for airflow, temporal, risingwave, rust spark like spice (which is mature and active) etc"**, with the owner's thesis that **open, existing protocols and APIs drive adoption**. Decisions **D1251–D1275** and questions **Q1030–Q1045** are in the [decision log](13-decision-log.md), all approved by the owner on 2026-10-21 with their defaults. The numbers follow §67 (D1228–D1250, Q1015–Q1029, PR #945, not yet merged). Plan stub: [OC1](../plans/2026-10-20-oc1-open-protocol-compute.md). **This document writes no code.**

> **Amendment (approved 2026-10-21; [§77](77-kv-gateways.md), D1640).** D1255 and D1272 change: a RESP gateway for key-value workloads is built, as a product on `loams-kv`, not as a Celery or BullMQ broker. D1255's Celery and BullMQ rulings, D1271 (no Redis 8 code or binaries) and Q1039's answer stand.

> **Amended by [§74](74-durable-execution-compat.md) (owner direction, approved 2026-10-21).** Loams adds a Temporal-protocol frontend in `loams-gateway` backed by Resonate and Loams Postgres (amends D1259, D1272, D1275); the upstream Temporal server on Loams Postgres runs from DX0 as the oracle and compat mode; the Airflow provider's first executor is `LoamsResonateExecutor` on Resonate, not `LoamsJobsExecutor` (amends D1257); DBOS is added. "Never on Loams SQL", real Airflow only, and `ostrium-labs/loams-airflow` stand. §74 §17 lists every change.

Builds on:
- [§00](00-pitch.md) §7 ("not a stream processor") and D42 (the protocol footprint): take over a product's role on Loams primitives, and speak its wire protocol only where the ecosystem protocol *is* the value.
- [§05](05-query-engine.md), D15: DataFusion is the single query engine, with `datafusion-distributed`, not Ballista.
- [§17](17-ai-data-ecosystem.md) §6–§7: Sail and Spice (D51, D54–D56).
- [§21](21-durable-execution.md) and [§14](14-durable-execution.md): Loams Durable, the embedded Resonate server (D138–D147), on TiKV (D261, PR #796).
- [§24](24-cpu-time-runtime.md) §16 and [§38](38-knative-authentik-gitops.md): the `Runner` trait (D375), Knative Serving and Eventing (D441–D446), the Rust Dapr server (D182, D183).
- [§26](26-jobs-api.md) and plan [JB1](../plans/2026-10-10-jb1-jobs-and-usage.md): Loams Jobs (D204–D216, D851): Celery through a kombu transport, BullMQ v6 through `IQueueBackend`, PySpark on Sail, Flink SQL on RisingWave.
- [§32](32-loams-flow-fabric-house.md), [§33](33-connectors.md), [§49](49-loams-house-production.md): the Event Fabric (Iggy and Fluss, D331–D336, D405, D408), Flow (D337–D341), the House on chDB (D342–D349, D761–D777), connectors (D352–D359, D852–D854).
- [§02](02-stream-engine.md) §7.4 and §34: CloudEvents on streams and the Loams profile (D270, D364, D402).
- [§57](57-loams-gateway.md), [§58](58-data-access-and-htap.md): `loams-gateway` and its `pg` module; PG wire first, the Data API, HTAP phases (D1030–D1061).
- [§63](63-loams-meter.md), [§64](64-loams-infra-stack.md), [§65](65-loams-operator.md), [§66](66-loams-mastra.md), [§67](67-gpu-scheduling-runai.md): the Meter, the infra stack's demand gates and Loams Run, the operator, the Mastra conformance pattern (D1206, D1207), GPU jobs.

Markers, as in §64–§67:
- **(verify)** means the claim has not been checked against a primary source. The task that depends on it checks it first.
- **(estimate)** means the number was judged, not measured.
- **(unverified)** means a search on 2026-10-20 did not settle it.

---

## 1. Summary

### 1.1 The request and how this document reads it

The owner named five families: an orchestrator (Airflow), a durable-workflow engine (Temporal), a streaming SQL engine (RisingWave), a Rust replacement for Spark ("like Spice"), and, by "etc", the job queues and event standards around them (Celery, BullMQ, Dapr, Knative, CloudEvents). The thesis is that **teams adopt a platform when the code they already have runs on it.** An Airflow DAG, a Temporal worker, a `CREATE MATERIALIZED VIEW`, a PySpark job and a Celery task are each written against an open protocol or API, and each of those protocols has thousands of users who will not rewrite.

Loams has already decided much of this, piece by piece: Celery, BullMQ, PySpark on Sail and Flink SQL on RisingWave are in §26 (D204–D216); RisingWave is the companion stream processor (D22, D341); Spice is a Flight SQL peer (D56); Resonate is the durable protocol (D19, D138). What is missing is the **rule** that says, for any product, how far Loams goes toward its protocol, and the three products no design covers yet: **Airflow, Temporal and Spark's "Rust replacement" question**, plus the gaps in Dapr, Knative and CloudEvents.

### 1.2 The rule: the protocol ladder (D1251)

For each product, Loams climbs only as far up this ladder as adoption needs, and every rung above the first has a demand gate (as §64 did for Nomad):

| Rung | Name | What Loams does | Example |
|---|---|---|---|
| **L0** | **Back it** | The unmodified product runs *on* Loams: its database is Loams Postgres (PG wire), its files and state are in the bucket, its catalog is Lakekeeper. Loams writes conformance tests and recipes, no engine code | Airflow's metadata DB on Loams Postgres; pg-boss on Loams Postgres; RisingWave's Hummock in the bucket |
| **L1** | **Plug in** | Loams ships a plugin through the product's own extension point (executor, secrets backend, kombu transport, BullMQ backend, provider, Dapr component). Users change a config line | `loams-celery` (D208); `@loams/bullmq` (D209); `airflow-provider-loams` (§4) |
| **L2** | **Host it** | Loams runs the unmodified upstream product for the user, per namespace, behind `loams-gateway`, with Loams auth, quotas and metering | Sail per namespace (D211); RisingWave via Jobs (D212); managed Airflow (OC2) |
| **L3** | **Speak it** | Loams serves the product's wire protocol from its own code, on its own engine | Qdrant API, Elasticsearch subset (D42); not proposed for anything in this document now |
| **L4** | **Replace it** | A Loams-native engine with the product's semantics | A continuous-query engine on DataFusion (§6.5): long-term direction only |

**The adoption test** that decides whether a rung above L1 is worth it: (1) the protocol has a large installed base of client code that users will not rewrite; (2) it is open (a permissive licence for the code Loams would run or read; no BSL, SSPL, ELv2, CCL or AGPL, D359) and stable enough to test against; (3) the lower rungs leave a gap a user has actually hit. L3 and L4 also need (4) a reason the upstream engine cannot be hosted (licence, tenancy, footprint or a Loams-specific capability). D42's "five-product trap" is L3 everywhere; this ladder keeps the trap closed while letting the cheap rungs carry adoption.

**One observation drives the near-term plan:** Postgres wire is itself an open protocol with a vast installed base, and Loams already speaks it (D1030). Every product whose state lives in Postgres (Airflow, Temporal, RisingWave's meta store, pg-boss, Graphile Worker, River, Oban, Hatchet, BullMQ's Postgres backend, Celery's `db+` result backend) is reachable at **L0**, with no engine code, once Loams Postgres passes that product's own persistence tests. The bucket plays the same role for products that keep state in S3 (RisingWave, Sail, Airflow logs and XComs), and Lakekeeper's Iceberg REST catalog for every table engine.

### 1.3 The recommendation in one paragraph

Start at **L0** for everything, now: a conformance matrix that runs unmodified Airflow, Temporal, RisingWave, Sail and the Postgres-backed job queues against Loams Postgres, the bucket and Lakekeeper, with published results, recipes and an opt-in Compose file (**OC0**, no engine code). Then **L1** where a plugin unlocks real value: an Airflow provider (hooks, operators, an asset trigger on Loams streams, a secrets backend and an executor on Loams Jobs), and the Spark Connect and RisingWave routes in `loams-gateway` (**OC1**). **L2** hosting of Airflow, Temporal, RisingWave and Sail follows per engine, each behind its own demand gate, on one operator kind (**OC2**). **L3** endpoints (Dapr components, CloudEvents SQL filters, a Celery RESP subset, a Spark Connect server on Loams' own engine) and the **L4** continuous-query engine are long-term directions behind gates, not commitments (**OC3**, **OC4**). Concretely: **Airflow** runs as real Airflow, never as a look-alike scheduler; **Temporal** is hosted, never imitated, and Durable stays Resonate; **streaming SQL** stays RisingWave, reached over Postgres wire; **"Rust Spark" is Sail, not Spice**, because Sail speaks Spark Connect and Spice does not; **Celery and BullMQ** keep §26's one-line plugins, plus a zero-change path through their Postgres backends.

### 1.4 The map

| Product (licence) | Role for a Loams user | Loams component | Protocol spoken, and by whom | Reuse | New | Phase |
|---|---|---|---|---|---|---|
| **Apache Airflow** (Apache-2.0) | Schedule and run DAGs of batch tasks | Real Airflow on Loams Postgres, the bucket and Loams Git; `airflow-provider-loams`; later managed | Airflow's REST API, UI and Task SDK, served by Airflow | Loams Postgres (D1030), the bucket, Loams Git Smart HTTP (D394), Jobs (D204), Loams Secrets (§64) | The provider (executor, trigger, secrets, operators); a `LoamsCompanion` engine | L0 OC0, L1 OC1, L2 OC2 (gated) |
| **Temporal** (MIT) | Durable workflows with an event history | Resonate stays Loams' engine; stock Temporal hosted on Loams Postgres if the spike passes | Temporal gRPC, served by Temporal's own server | Loams Postgres; Durable (D138) for the migration guide | A persistence spike, a recipe, a migration guide; later a companion | L0 OC0 (spike), L2 OC2 (gated) |
| **RisingWave** (Apache-2.0) | Streaming SQL and materialized views | RisingWave companion (D22, D212) | Postgres wire, served by RisingWave, routed by `loams-gateway` `pg` | The bucket (Hummock), Loams Postgres (meta), Lakekeeper, the House, Iggy's Kafka gateway (D408) | Conformance; the SNI route; tenancy rule | L0 OC0, L1 OC1, L2 J4/OC2 |
| **Flink SQL / DataStream** (Apache-2.0) | Streaming jobs teams already have | JB1 J4 unchanged (D212) | Flink's, on the user's opt-in Flink | — | — | J4 |
| **Spark (PySpark, Spark SQL)** (Apache-2.0) | Batch DataFrames and SQL | **Sail** per namespace (D211) | Spark Connect, served by Sail, routed by `loams-gateway` `spark` | Lakekeeper, the bucket, `format("loams")` (D54), engine runners (J3) | The `spark` route; a Spark conformance corpus | L0 OC0, L1 OC1, L2 J3 |
| **Comet / Gluten** (Apache-2.0) | Faster JVM Spark | Recipe only | Spark, on the user's cluster | Lakekeeper | A recipe | L0 OC1 |
| **Spice.ai OSS** (Apache-2.0) | App-tier federation and acceleration | Flight SQL peer (D56) | Flight SQL, served by Loams | The Flight SQL frontend | A recipe | L0 OC1 |
| **Ballista, Daft, DuckDB** (Apache-2.0, MIT) | Distributed DataFusion; Python dataframes | Not used (D15); readers (§17) | — | — | — | — |
| **Celery** (BSD-3) | Python task queue | `loams-celery` (D208); `db+` result backend on Loams Postgres | Kombu's transport API, by Loams' plugin | Jobs (D204) | Conformance of the `db+` path | L1 J1, L0 OC0 |
| **BullMQ** (MIT) | Node task queue | `@loams/bullmq` (D209); BullMQ's Postgres backend on Loams Postgres | BullMQ's `IQueueBackend`, by Loams' plugin; or Postgres wire | Jobs; Loams Postgres | Conformance of the Postgres backend | L1 J2, L0 OC0 |
| **pg-boss, Graphile Worker, River, Oban, Hatchet, Procrastinate** (MIT, MPL-2.0, Apache-2.0) | Postgres-backed job queues | Unmodified, on Loams Postgres | Postgres wire, served by Loams Postgres | Loams Postgres, the gateway | Conformance and recipes | L0 OC0 |
| **Dapr** (Apache-2.0) | Sidecar APIs: pub/sub, state, bindings | `loams-dapr` (D182), `daprd` long tail (D183); Loams as a pluggable component later | Dapr's component gRPC, by Loams | Streams, `loams-kv` | Components | L1 OC3 (gated) |
| **Knative** (Apache-2.0) | Serverless pods and event delivery | `KnativeRunner` (D441), `loams-knative-source` (D445) | CloudEvents over HTTP | Already planned (MT2) | — | — |
| **CloudEvents, CESQL** (CNCF specs) | Event envelope; filters | Every stream (D270, D364) | CloudEvents bindings, by Loams | `loams-cloudevents` | CESQL filters | L3 OC3 |
| **Serverless Workflow** (CNCF spec) | Declarative workflow DSL | A possible front end to Durable | — | Durable | — | Long-term |
| **Materialize, Estuary, ksqlDB, Redis 8** (BSL, BSL, CCL, RSAL/SSPL/AGPL) | — | **Excluded** (D1271) | — | — | — | — |

### 1.5 Phase order (D1252)

| Phase | Rung | Content | Horizon | Starts |
|---|---|---|---|---|
| **OC0** | L0 | Conformance of unmodified products on Loams Postgres, the bucket and Lakekeeper (`conformance/compute/`); recipes under `docs/guides/compute/`; the opt-in `deploy/release/compose.compute.yaml`; a compatibility matrix; the Temporal persistence spike | **Near-term** | After v0.1.1 (Q1030) |
| **OC1** | L1 | `airflow-provider-loams` (hooks, operators, `LoamsStreamTrigger`, secrets backend, `LoamsJobsExecutor`); the `spark` route in `loams-gateway`; the `pg` route to a namespace's RisingWave; the Temporal → Resonate migration guide; Comet and Spice recipes | **Near-term** (after its dependencies) | OC0 green; the executor after JB1's J1 core; the routes after GW1's `pg` module |
| **OC2** | L2 | `LoamsCompanion` operator kind; managed Airflow, Temporal and RisingWave per namespace; Sail and RisingWave lifecycle as already planned in JB1 J3/J4; companion metering | **Medium-term, demand-gated** | Per engine, its own gate (§16, §18) |
| **OC3** | L3 | Dapr pluggable components (state, pub/sub); CloudEvents SQL as the filter language; a Celery RESP subset; a Spark Connect endpoint on Loams' own engine | **Long-term direction, demand-gated** | Each behind its own question (Q1036–Q1041) |
| **OC4** | L4 | A native continuous-query engine on DataFusion over Loams streams | **Long-term direction, not committed** | Q1036; would amend §00 §7 |

### 1.6 What changes

| Before | After |
|---|---|
| No rule for how far to go toward a product's protocol beyond D42's narrowing | The ladder and its adoption test (D1251); every new "support X" request is placed on a rung |
| Airflow appears only as "no need for Airflow" (§21 §6.1, §26 §9.2) and a P3 connector row (§33) | Real Airflow is supported on Loams (L0), with a Loams provider (L1) and managed Airflow behind a gate (L2) (D1256–D1258) |
| Temporal is "replaced by Resonate" (§01) with no position on Temporal users | Durable stays Resonate; Temporal users get a hosted Temporal on Loams Postgres if the spike passes, and a migration guide; no Temporal-compatible frontend (D1259) |
| §26 builds the hosting of Sail and RisingWave (J3, J4) | Unchanged; OC0's conformance becomes J3's and J4's Task 0 input; the Spark Connect proxy moves into `loams-gateway` (D1263) |
| "Rust Spark like Spice" | Sail is the Spark Connect engine; Spice stays a Flight SQL peer and a recipe, not a Spark substitute (D1262) |
| Celery and BullMQ need a one-line change (D208, D209) | Same, plus a zero-change path for apps that already use Postgres-backed queues, verified on Loams Postgres (D1254) |
| Dapr: `loams-dapr` planned, Dapr Workflow denied (D182, D183) | Unchanged; Loams as a Dapr pluggable component is an OC3 option (D1269) |

## 2. Goals and non-goals

### 2.1 Goals

1. **Existing code runs.** For each family, name the smallest change a user makes (none, a config line, an import), and prove it with that product's own tests, not ours.
2. **One set of primitives underneath.** Every companion stores state in Loams Postgres, TiKV, the bucket or Lakekeeper, authenticates through Loams principals, and is reached through `loams-gateway`.
3. **Cheapest rung first.** OC0 ships no engine code; OC1 ships plugins; anything heavier waits for demand.
4. **Licence safety.** Nothing under BSL, SSPL, ELv2, CCL or AGPL is run, linked or copied; vendor enterprise features are never defaults (D1271).
5. **Honest positioning.** Docs say "runs Airflow", "runs Sail, a Spark Connect server", "hosts Temporal on Loams Postgres": never "Airflow-compatible", "a Spark replacement" or "Temporal-compatible" for things Loams did not build.

### 2.2 Non-goals

- **No look-alike engines** at L3 or L4 in OC0–OC2: no Airflow-API scheduler, no Temporal frontend, no Spark Connect server of Loams' own, no Redis or AMQP server, no stream processor.
- **No change to §26's semantics** (D207) or JB1's tasks. This design adds conformance, plugins and gates around them.
- **No second catalog** (Unity Catalog, Polaris) and no second durable engine (Dapr Workflow, Temporal as Loams' own engine).
- **No JVM in Loams' default deployment.** JVM engines (Spark with Comet or Gluten, Flink) stay recipes or JB1's opt-in fallbacks (D211, D212).

## 3. Where Loams stands today

| Family | Decided | Built on `dev` | Gap this document fills |
|---|---|---|---|
| Durable workflows | Resonate embedded, protocol `2026-04-01`, TiKV store (D138–D147, D261) | `loams-durable`, the TiKV store (#796) | Temporal users (§5) |
| Job queues | Celery kombu transport, BullMQ `IQueueBackend`, schedules on Resonate (D204–D210, D851) | Nothing yet (`loams-jobs` is JB1) | The zero-change Postgres path; Airflow's executor (§4, §8) |
| Orchestration | "Scheduled imports need no Airflow" (§21 §6.1); `plugin-airflow` a P3 connector row (§33) | — | Airflow (§4) |
| Streaming SQL | RisingWave companion (D22, D341); Flink SQL ported to it (D212); Arroyo documented only (D851) | — | The access path, conformance and the native-engine question (§6) |
| Batch / Spark | Sail per namespace, Spark Connect proxy, Spark fallback (D211, D851); Sail a named Iceberg reader (D55) | — | The "Rust Spark" question, Spice, Comet, the proxy's home (§7) |
| Analytics | House on chDB over Fluss and Iceberg (D342–D349) | `fabric/` House crates | Its place beside the companions (§9) |
| Events | CloudEvents on streams (D270, D364, D402); Knative runner and source (D441, D445); no broker (Q443); `loams-dapr` (D182) | `loams-cloudevents` | Dapr components, CloudEvents SQL, Serverless Workflow (§8.4) |
| Catalog | Lakekeeper Iceberg REST (D6, D47, D413) | Lakekeeper in compose (House); a REST client in `loams-house` | One catalog for every engine (§9.1) |

## 4. Airflow

### 4.1 What Airflow is in October 2026 (checked 2026-10-20)

- **Apache Airflow 3.3.2** (2026-09-17), Apache-2.0, about 47k stars and 9,300 commits in the last year; a new 3.x minor roughly every three months; Airflow 2 reached end of life on 2026-04-22 ([release notes](https://airflow.apache.org/docs/apache-airflow/stable/release_notes.html), [supported versions](https://airflow.apache.org/docs/apache-airflow/stable/installation/supported-versions.html)).
- **Components:** API server (UI, `/api/v2`, the Execution API), scheduler (with the executor in-process), DAG processor, triggerer. **Workers no longer touch the metadata DB**: since AIP-72 they reach the API server through the Execution API with short-lived per-task JWTs, and author against `airflow.sdk`. The Execution API is **not** a declared public interface ([public interface](https://airflow.apache.org/docs/apache-airflow/stable/public-airflow-interface.html), [security model](https://airflow.apache.org/docs/apache-airflow/stable/security/security_model.html)).
- **Metadata DB:** PostgreSQL 14–18 or MySQL 8.0/8.4 tested; SQLite for development; MariaDB unsupported ([prerequisites](https://airflow.apache.org/docs/apache-airflow/stable/installation/prerequisites.html)). The scheduler takes row locks with `SKIP LOCKED` in several places (HA schedulers depend on them) and migrations take `pg_advisory_lock` ([`scheduler_job_runner.py`](https://raw.githubusercontent.com/apache/airflow/main/airflow-core/src/airflow/jobs/scheduler_job_runner.py), [`utils/db.py`](https://raw.githubusercontent.com/apache/airflow/main/airflow-core/src/airflow/utils/db.py)).
- **Extension points:** executors (`BaseExecutor`; several at once since 2.10, chosen per task); secrets backends (`BaseSecretsBackend`); remote logging (`[logging] remote_base_log_folder`); the object-storage XCom backend (`XComObjectStorageBackend`); DAG bundles (`GitDagBundle`, `S3DagBundle`, custom `BaseDagBundle`); auth managers (`BaseAuthManager`); event-driven scheduling with `AssetWatcher` and `BaseEventTrigger`; provider packages that carry hooks and operators.
- **Managed Airflow** means: Amazon MWAA (Airflow 3.3.1 since 2026-09-01; per environment-hour), Google Cloud Composer 3 (Airflow 3 environments, being renamed "Managed Service for Apache Airflow"), Astronomer Astro (its own executor and remote-execution agents in the customer's cluster). Users buy upgrades, autoscaled workers, persistent logs, secrets, DB backups, DAG deploy from Git, private networking and SSO ([MWAA](https://aws.amazon.com/about-aws/whats-new/2026/09/amazon-mwaa-apache-airflow-3-3-1/), [Composer](https://docs.cloud.google.com/composer/docs/composer-versioning-overview), [Astro](https://www.astronomer.io/docs/astro/remote-execution-overview)).
- **Look-alikes:** the only Airflow-API-compatible scheduler on another engine found is **Leoflow** (Go, Apache-2.0, v0.4.8 of 2026-09-21, months old), which compiles DAGs into images ([pkg.go.dev](https://pkg.go.dev/github.com/neochaotic/leoflow)). Dagster's `dagster-airlift` observes Airflow and moves tasks one by one; it does not run DAG files.

### 4.2 Options

| | (a) Run real Airflow on Loams (L0 → L1 → L2) | (b) An Airflow-API-compatible scheduler on Durable (L3/L4) |
|---|---|---|
| What runs user DAGs | Airflow itself | Loams code that parses DAG files and imitates the scheduler, the REST API, the UI contract and the Execution API |
| Compatibility | Total by construction: providers, operators, deferrables, mapped tasks, assets, backfills | DAG files import `airflow.*`; every operator and provider pulls more of Airflow; semantics drift (mapped tasks, deferrables, assets, run state machine) |
| Moving target | The public interface (stable), plus a pinned image per minor | The Execution API is not public and changes between releases; a minor every three months |
| What Durable adds | Durable steps inside tasks (§26 §9.3's pattern), Loams Jobs as the executor | A durable scheduler, which Airflow users did not ask for |
| Cost (estimate) | OC0 2 weeks; provider 5–7 weeks; managed 6–8 weeks | Years for credible parity; Leoflow shows the pattern is still immature |

**Recommendation (D1256):** (a). Loams runs real Airflow and never ships an Airflow look-alike. Resonate stays the Loams-native authoring path for teams that want durable code rather than DAGs, and the docs show both.

### 4.3 L0: Airflow on Loams primitives (OC0)

Unmodified Airflow 3.3 with only configuration:

| Need | Airflow setting | Loams primitive | Gate test |
|---|---|---|---|
| Metadata DB | `[database] sql_alchemy_conn = postgresql+psycopg2://…` | Loams Postgres, through `loams-gateway` `pg`, **session pooling or a direct endpoint** (not PgDog transaction mode, verify) | `airflow db migrate` from empty; two schedulers in HA with `use_row_level_locking = True` run 500 task instances with no double-run; advisory lock taken and released on migrate |
| Logs | `[logging] remote_logging = True`, `remote_base_log_folder = s3://…`, `remote_log_conn_id` | The bucket (RustFS in compose) | Task logs readable in the UI after the worker pod is gone |
| XComs | `[core] xcom_backend = airflow.providers.common.io.xcom.backend.XComObjectStorageBackend`, `[common.io] xcom_objectstorage_path` | The bucket | An XCom above the threshold round-trips |
| DAG source | `[dag_processor] dag_bundle_config_list` with `GitDagBundle` | Loams Git over Smart HTTP v2 (D394) | A DAG pushed to Loams Git runs at the recorded commit |
| Secrets | `[secrets] backend` | Kubernetes or env secrets in OC0; Loams Secrets (§64) in OC1 | — |
| Auth | `[core] auth_manager` | Simple or Keycloak-style OIDC against Zitadel (§76 D1600, superseding D447) in OC0 (verify) | — |

The recipe is `docs/guides/compute/airflow.md`; the Compose profile `airflow` in `compose.compute.yaml` (§10.1) is the test bed.

### 4.4 L1: `airflow-provider-loams` (OC1; D1257)

A third-party Airflow provider (PyPI `airflow-provider-loams`, the third-party naming convention; repository location Q1031), Apache-2.0, built only on Airflow's public interface:

- **`LoamsHook`** (connection type `loams`: endpoint, `lk_` key or OIDC client, namespace) over the Python SDK.
- **Operators:** `LoamsSqlOperator` (Loams Postgres or the Data API, D1042), `LoamsHouseQueryOperator` (the House HTTP surface), `LoamsSparkSubmitOperator` (a `SparkBatch` engine job, D211), `LoamsStreamingSqlOperator` (a `StreamingSql` engine job, D212), `LoamsDurableOperator` (start a Resonate function and defer until its promise resolves), `LoamsJobEnqueueOperator`.
- **`LoamsStreamTrigger`** (a `BaseEventTrigger`) for `AssetWatcher`: long-polls a named consumer on a Loams stream, so a CloudEvent (a table commit, a Fabric event) creates an asset event. Asset URIs use the scheme `loams://<namespace>/<object>` (registered by the provider).
- **`LoamsSecretsBackend`** over Loams Secrets (§64, D1156).
- **`LoamsJobsExecutor`** (a `BaseExecutor`): each workload is enqueued on a Loams Jobs queue (`airflow.<executor-queue>`); `loams-airflow worker` processes lease jobs, run the workload with the Task SDK's supervisor and complete with the outcome; the executor's `sync` maps `watch` events to task states. Loams brings leases with fencing, quotas, the Meter gate and one place to see all work; a pod-per-task variant through an engine job is OC2. The exact 3.3 workload payload and supervisor entry point are **(verify)** at OC1 Task 0; if they move between minors, the executor pins a minor range like `@loams/bullmq` does (D209).

The provider is the adoption hook: an Airflow team uses Loams from its DAGs before it moves anything.

### 4.5 L2: managed Airflow (OC2, gated; D1258)

`LoamsCompanion` with `engine: airflow` (§10.2) renders the official Apache Airflow Helm chart (Apache-2.0) with §4.3's settings per namespace: API server and UI behind `loams-gateway` `http` with Loams SSO, scheduler pair, DAG processor, triggerer, workers on `LoamsJobsExecutor`, upgrades as durable workflows with a DB snapshot (a Loams Postgres branch) before `db migrate`. **Gate:** three deployments run the provider's executor in production, or three users ask for managed Airflow (Q1034).

### 4.6 Not built

An Airflow-compatible scheduler, REST API or UI of Loams' own; a DAG translator into Resonate; an Airflow fork; Astronomer's proprietary images or agents.

## 5. Temporal

### 5.1 What Temporal is (checked 2026-10-20)

- **Licence:** server MIT; SDKs MIT (Go, TypeScript, Python, .NET, Ruby, PHP, Rust core) and Apache-2.0 (Java); the API protos MIT ([temporalio/temporal](https://github.com/temporalio/temporal), [temporalio/api](https://github.com/temporalio/api)). Server contributions need a CLA. Latest v1.32.1 (2026-10-08), three minor lines patched in parallel; the Rust SDK went GA on 2026-09-23.
- **Protocol:** gRPC `WorkflowService` with **121 RPCs** (counted in `service.proto`, 2026-10-20) plus `OperatorService`; 61 history event types and 18 command types. The model is **event-sourced deterministic replay**: workers poll task queues, return commands, and the server records events that replay must match; sticky queues, signals, queries, updates, timers, child workflows, continue-as-new, schedules, search attributes, worker versioning and Nexus all sit on that history. Protos are guarded by `buf breaking`, but the surface grows every release.
- **Persistence:** Cassandra, MySQL 8, PostgreSQL 12+ (SQLite for development); visibility on Elasticsearch/OpenSearch or SQL. The SQL plugins take `SELECT … FOR UPDATE` and `FOR SHARE` row locks and fence shards by `range_id`; `numHistoryShards` is fixed at cluster creation. Temporal tests only its listed databases ("We do not run tests with vendors like Vitess and CockroachDB") ([persistence](https://raw.githubusercontent.com/temporalio/documentation/main/docs/encyclopedia/temporal-service/persistence.mdx)). Transaction-mode PgBouncer is reported to break prepared statements (a 2024 forum thread, **unverified** by Temporal).
- **Third-party servers:** none found that implement Temporal's API.
- **Temporal Cloud** bills $25–50 per million actions plus storage.

### 5.2 Resonate and Temporal compared

| Concept | Temporal | Resonate (Loams Durable) | Fit |
|---|---|---|---|
| Unit | Workflow execution with an event history | A durable promise per call; functions with `ctx.run`, `ctx.rpc`, `ctx.sleep` | Different models |
| Recovery | Replay the history; commands must match events (determinism checked) | Re-execute the function; completed promises are memoised (generator-based SDKs) | Both need deterministic orchestration code |
| Activity | Activity with four timeout kinds, heartbeats | `ctx.run` / `ctx.rpc` with one promise deadline and retries | Clean, but lossy on timeouts |
| Child workflow | Child workflow | `ctx.rpc` by name | Clean |
| Signal | Signal | A latent promise resolved externally (`ctx.promise`) | Clean |
| Timer | Timer | `ctx.sleep` | Clean |
| Query, Update | First-class | None | No equivalent |
| Schedules | Schedules | Resonate schedules (§21 §6.1, D210) | Close |
| Versioning | Patching, worker versioning | None in the protocol | Gap |
| Search, visibility | Search attributes, SQL-like list filters | Tags and promise search | Partial |
| Nexus, continue-as-new | First-class | Loops, detached calls | Rough or none |
| Storage | History DB + visibility store | One document per origin on TiKV (D261) | — |

### 5.3 Options

| | (a) Resonate stays; Loams adds a Temporal gRPC frontend (L3) | (b) Loams hosts the stock Temporal server on Loams Postgres (L0 → L2) | (c) Skip Temporal |
|---|---|---|---|
| What it takes | Reimplement Temporal's history and matching services: history storage, workflow-task dispatch, sticky queues, command → event semantics that `sdk-core` replays exactly, visibility | A persistence spike, a recipe, then a `LoamsCompanion` | Nothing |
| Compatibility | A moving 121-RPC target with no conformance suite; replay failures are subtle | Total by construction (Temporal's own server) | None |
| Cost (estimate) | A minimal subset 13–26 engineer-weeks; parity multi-year | Spike 1 week; recipe 1 week; managed 4–6 weeks | 0 |
| Risk | Very high | Persistence behaviour on the fork; four Go services to operate | Lost Temporal users |

**Recommendation (D1259):** **(c) now, (b) behind the spike, never (a).** Durable stays Resonate (D19, D138). OC0 runs Temporal's SQL persistence tests and a soak against Loams Postgres (session pooling or direct; `FOR SHARE`, 4 MB event batches, `numHistoryShards` chosen up front). If they pass, `docs/guides/compute/temporal.md` publishes "Temporal on Loams Postgres" as a self-hosted recipe (L0); managed Temporal (L2) waits for its gate (Q1033). Loams SQL (TiDB-derived) is **not** a Temporal backend: TiDB parses `LOCK IN SHARE MODE` but ignores it ([TiDB docs](https://docs.pingcap.com/tidb/stable/sql-statement-select)), so Temporal's read locks would silently lose exclusion. A migration guide (Temporal → Resonate) uses §5.2's table; it promises nothing for queries, updates or Nexus.

## 6. Streaming SQL

### 6.1 The landscape (checked 2026-10-20)

| Engine | Licence (LICENSE file) | Status | Fit |
|---|---|---|---|
| **RisingWave** | Apache-2.0; some "Premium" features behind a licence key (§26 §10.1) | v3.1.0 (2026-09-21), v3.0.5 (2026-10-12); very active | Postgres wire, `CREATE MATERIALIZED VIEW`; state in S3 (Hummock via OpenDAL, custom endpoint supported); meta store on Postgres since etcd's removal; Kafka, NATS JetStream, CDC sources; Iceberg, ClickHouse, Elasticsearch, Postgres sinks; an all-in-one dev mode; an 8 GiB-class memory floor ([architecture](https://docs.risingwave.com/get-started/architecture), [SQL meta backend](https://docs.risingwave.com/deploy/migrate-to-sql-backend)) |
| **Arroyo** | MIT OR Apache-2.0 | v0.15.0 (2025-12-01); Cloudflare-owned since 2025-04; commits continue, releases slow; on a DataFusion 48 fork | Documented alternative only (D851) |
| **Feldera** | MIT, except Feldera Enterprise code under commercial terms | Very frequent releases (v0.367.0, 2026-10-18); DBSP; Calcite (Java) frontend; in-memory state | Watch; enterprise boundary must be checked first |
| **Materialize** | BSL 1.1 (Apache-2.0 on 2030-10-20); the grant forbids a "Database Service" | — | **Excluded** (D1271) |
| **Estuary Flow** | BSL 1.1 | — | Excluded |
| **ksqlDB** | Confluent Community License | — | Excluded |
| **Flink SQL** | Apache-2.0 (Java) | — | JB1 J4's opt-in path (D212) |
| **Timeplus Proton** | Apache-2.0 (C++, ClickHouse-based) | Active | Not needed beside the House |
| **Denormalized** | Apache-2.0 (DataFusion streaming) | Last push 2024-12 | Dormant |
| DataFusion itself | Apache-2.0 | Unbounded tables run, but no durable operator state, checkpoints or event-time windows in core | The base an L4 engine would build on |

### 6.2 What Loams already does incrementally

Loams is not a stream processor (§00 §7), but three layers already compute continuously, and they cover most "materialized view" asks before a stream processor is needed:

1. **Links** (§09): stateless transforms and mergeable aggregates from streams into tables, collections and graphs, with consistency tokens.
2. **House materialized views** (D338): ClickHouse-style per-insert-block transforms over Fluss and Iceberg.
3. **Live queries** (D1048): an initial result plus keyed diffs, re-run on change.

Windowed joins with watermarks and checkpointed state are what these do not do. That is RisingWave's job (D22, D341).

### 6.3 Options

| | (a) Integrate RisingWave (L0 → L2) | (b) Source/sink only (L0) | (c) Native continuous-query engine on DataFusion (L4) |
|---|---|---|---|
| What Loams does | Runs RisingWave with Hummock in the bucket, its meta store on Loams Postgres, routed through `loams-gateway` | Documents and tests a user-run RisingWave against Loams | Builds incremental operators, windows, watermarks, a state store on S3, barrier checkpoints, exactly-once sinks, a planner |
| Protocol users see | Postgres wire with RisingWave's SQL | Same | A Loams dialect |
| Cost (estimate) | 12–20 engineer-weeks for packaging, tenancy, conformance, observability (much already in J4) | 4–6 engineer-weeks | 18–30 engineer-months for a usable stateful subset; 40+ for RisingWave-class breadth |
| Risk | A heavy dependency; tenancy is Loams' to build | Users operate it | The largest new engine in Loams; reverses §00 §7 |

**Recommendation (D1260, D1261):** (b) in OC0, (a) through JB1 J4 and OC2 behind its gate, (c) only as a long-term direction.

### 6.4 RisingWave on Loams (OC0, OC1)

- **State and meta:** Hummock on the bucket (`hummock+s3://<bucket>` with `RW_S3_ENDPOINT`); the SQL meta backend on Loams Postgres (**verify** in OC0: RisingWave tests only vanilla Postgres, MySQL and SQLite).
- **Sources:** Loams streams through Iggy's Kafka gateway (D408) once its consumer-group support lands; until then the Fabric's NATS or CDC paths, or Loams Postgres CDC (RisingWave's Postgres CDC source against Loams Postgres, **verify** logical replication through the fork).
- **Sinks:** Iceberg through Lakekeeper (RisingWave's REST catalog support), the House through RisingWave's ClickHouse sink (against `chsurface-2.0`'s native or HTTP interface, **verify**), Loams collections through its Elasticsearch sink (the ES subset, D48).
- **Access (OC1):** users connect with any Postgres client to `<ns>.stream.<domain>`; `loams-gateway`'s `pg` module resolves the SNI name to the namespace's RisingWave frontend and applies Loams auth before splicing (§57 §7). The protocol is Postgres wire, served by RisingWave, routed by Loams.
- **Licence boundary:** Loams never sets a RisingWave licence key and never documents a Premium feature as a Loams feature; the Iceberg REST catalog path is not Premium (§26 §10.1), the Glue catalog is.
- **Tenancy:** RisingWave runs user UDFs (Python, JavaScript, Rust/Wasm) in some modes; a namespace that enables UDFs gets its own RisingWave; a shared cluster with a database per namespace is allowed only with UDFs off (Q1035, refining D851's answer to Q96).

### 6.5 L4: a native continuous-query engine (long-term direction; D1261)

Not committed. If built, it would sit on DataFusion over Loams streams and Fluss, keep operator state in the bucket with Loams' manifest-pointer CAS (D47) for checkpoints, emit changelog streams (D21), and start from Arroyo's MIT/Apache-2.0 code rather than from scratch. **Gate (Q1036):** RisingWave per namespace proves too heavy for small namespaces (its memory floor times the namespace count), **and** at least three users need windowed joins over Loams streams with consistency tokens that a companion cannot honour. Building it would amend §00 §7 and D341; this document amends neither.

> **Superseded (approved 2026-10-29): D2240, D2243** ([§82](82-loams-stream-flink-compatible-engine.md)). On the owner's request the L4 engine is built now as **Loams Stream**, a Rust engine that speaks Apache Flink's SQL and REST APIs, on a hard fork of StreamFusion with runtime pieces ported from Arroyo, as this section anticipated for Arroyo; Q1036's gate no longer applies.

## 7. Spark-compatible batch ("Rust Spark")

### 7.1 Spice is not Spark (checked 2026-10-20)

**Spice.ai OSS** is Apache-2.0, very active (v2.3.2 of 2026-09-22, v2.4.0-rc.2 of 2026-10-15, about one tag a week, about 3,900 commits in the last year) and mature as a product, which supports the owner's description. But it is an **application-tier federation and acceleration engine**: one Rust binary on DataFusion, Arrow and Vortex, serving SQL over HTTP, Flight, Flight SQL, ODBC, JDBC and ADBC, with 30+ connectors, accelerators (its own Cayenne, Arrow, DuckDB, SQLite), CDC, LLM and search features, and an MCP server ([README](https://raw.githubusercontent.com/spiceai/spiceai/trunk/README.md)). It **does not serve Spark Connect**: its `spark` connector is a Spark Connect *client* that queries a Spark cluster ([docs](https://spiceai.org/docs/components/data-connectors/spark)). Its distributed mode is a Ballista fork; the docs place distributed query under Spice.ai Enterprise (**unverified** whether the OSS binary has it). Spice overlaps Loams' own engine (DataFusion, Tantivy, Iceberg) more than it complements it, which is why §17 kept it a Flight SQL peer (D56) and refused to embed it (Spice patches crates.io with forks of DataFusion and arrow-rs).

### 7.2 What speaks Spark

| Engine | Licence | Spark protocol | Status (2026-10-20) | Role for Loams |
|---|---|---|---|---|
| **Sail** (LakeSail) | Apache-2.0 | **Spark Connect server** (PySpark 3.5, 4.x clients) | v0.7.2 (2026-09-29); ~1,060 commits in the last year; DataFusion 55.1; Kubernetes cluster mode with object-store shuffle (0.7) and Celeborn (0.7.1); Iceberg REST, Unity, Glue, HMS catalogs; Delta and Iceberg MERGE/UPDATE/DELETE in its own crates; Python UDFs, `pandas_udf`, `mapInPandas`, `applyInPandas`; no RDD, JVM UDFs, MLlib, pandas-on-Spark; Structured Streaming in progress; crates not on crates.io; Adyen in production (vendor post); a seed-funded company | **The Spark engine** (D211, reconfirmed) |
| **Apache DataFusion Comet** | Apache-2.0 | A plugin inside JVM Spark | 1.1.0 (2026-10-06); Spark 3.5–4.1 | A recipe for teams that keep JVM Spark |
| **Apache Gluten** (Velox) | Apache-2.0 | A plugin inside JVM Spark | Apache top-level project (2026-03) | Same as Comet |
| **Apache DataFusion Ballista** | Apache-2.0 | None | 55.0.0 (2026-10-06); revived; Python client pre-alpha | Not used (D15: `datafusion-distributed`) |
| **Daft** | Apache-2.0 | A `daft.pyspark` Spark Connect layer (coverage **unverified**) | v0.7.26 (2026-10-06); Python-first, multimodal | A reader of Loams data (§17) |
| **DuckDB** | MIT | An experimental PySpark-like API, not Spark Connect | — | A reader |
| `spark-connect-rs` | Apache-2.0 | Rust client | 0.0.2 (2025-02); stale | A test harness at most |
| `datafusion-spark` | Apache-2.0 | Spark-semantics functions for DataFusion | 55.2.0 (2026-10-07), lockstep with DataFusion | Optional for Loams SQL (Q1043) |

**Spark Connect** (gRPC with protobuf plans, Arrow results; Spark 3.4+) is the protocol that matters: PySpark clients change only `.remote("sc://…")`. It has **no authentication of its own**, so a gateway in front is required anyway, and it does not carry RDDs or `SparkContext` ([overview](https://spark.apache.org/docs/latest/spark-connect-overview.html)).

### 7.3 Options

| | (a) Host Sail (L2; D211) | (b) A Spark Connect endpoint on Loams' own DataFusion (L3) | (c) Spice as the "Spark" component | (d) Comet or Gluten recipes (L0) |
|---|---|---|---|---|
| Spark code runs | PySpark and Spark SQL over Connect, within Sail's coverage | Whatever Loams reimplements | **No**: Spice does not accept Spark clients | Everything, on the user's JVM Spark |
| Cost (estimate) | Already J3; OC0 adds conformance (2 weeks) | 26–52 engineer-weeks to approach Sail; Sail's crates are not published, so reuse means a git pin | Integration only; it does not meet the goal | 2–3 days per recipe |
| Risk | Pre-1.0, one small company; coverage gaps | DataFusion lockstep against Lance (D51); a second Spark engine to maintain | Wrong tool | None for Loams |

**Recommendation (D1262):** (a) Sail is Loams' Spark, unchanged from D211, and the answer to "Rust Spark". (d) as recipes. (c) Spice stays what D56 made it, a Flight SQL peer, plus a recipe "Spice in front of Loams" for application-tier acceleration (Q1038). (b) is not built unless Sail stops shipping (no release for six months) or a Loams-only capability needs it (Q1037).

### 7.4 What OC0 and OC1 add to D211

- **Conformance (OC0):** `conformance/compute/spark/` runs a pinned Sail against Lakekeeper and the bucket with our own PySpark corpus: Spark SQL TPC-H-derived queries, DataFrame jobs, a `pandas_udf`, an Iceberg `MERGE` on a table Loams does not maintain (one writer class, §17 §6.1), a Delta read, and `format("loams")` (D54). Failures become an expected-failure list that fails on surprises in either direction (D1206's pattern).
- **The route moves into `loams-gateway` (OC1; D1263):** D211 put a Spark Connect proxy in Loams' gateway role. Since §57 there is one Rust gateway, so the proxy becomes its `spark` module: HTTP/2 gRPC on `<ns>.spark.<domain>:443`, `authorization: Bearer` checked against Loams principals, the namespace from the credential, sticky routing by Spark Connect's `session_id`, start-on-first-connect through the engine runner, and `traceparent` at the first hop. It is a router; Loams still parses no Spark plans.
- **Recipes (OC1):** Comet with Loams' Iceberg tables (Spark → Lakekeeper → bucket); Spice reading Loams over Flight SQL.

## 8. Job queues and event standards

### 8.1 Celery and BullMQ (D1255)

§26 decided both against wire emulation: Celery through a kombu transport and result backend (`broker_url = "loams://…"`, D208), BullMQ v6 through its `IQueueBackend` (D209). The research of 2026-10-20 confirms the reasons and adds one:

- **Redis protocol for BullMQ** means a Redis 6.2-class server with Lua (49 scripts that build keys at runtime; Dragonfly needs special flags), streams and blocking pops: 3–6 engineer-months and a permanent semantic tail (estimate).
- **Redis protocol for Celery** looked small (lists, hashes, zsets, pub/sub, MULTI/WATCH), but kombu's Redis transport on `main` now calls two Lua scripts through `EVALSHA` ([`redis.py`](https://raw.githubusercontent.com/celery/kombu/main/kombu/transport/redis.py)), so even Celery needs Lua.
- **AMQP 0-9-1:** no maintained Rust broker exists (`lapin` and `amqprs` are clients); a useful subset is 2–3 engineer-months.
- **Redis itself** is RSALv2/SSPLv1/AGPLv3 since Redis 8 ([README](https://raw.githubusercontent.com/redis/redis/unstable/README.md)), so Loams never runs it. Valkey (BSD-3) is fine to document as user-run, but a managed Redis-compatible service invites every Redis workload (§26 §7.2.4), so Loams does not host one.

So §26's non-goals stand: no RESP, no AMQP in OC0–OC2. A **Celery-only RESP subset** is an OC3 option behind Q1039.

### 8.2 The zero-change path: Postgres-backed queues (D1254)

Many teams already chose a queue whose broker is Postgres. On Loams Postgres these run with **no code change at all**, which is the owner's "existing workers run unchanged", at L0:

| Framework | Language, licence | Needs (verify each in OC0) |
|---|---|---|
| BullMQ v6 Postgres backend | TypeScript, MIT | `LISTEN`/`NOTIFY`, `SKIP LOCKED` (§26 §7.2) |
| Celery `db+postgresql://` result backend | Python, BSD-3 | SQLAlchemy only |
| pg-boss | TypeScript, MIT | Postgres 13+, `SKIP LOCKED`, `LISTEN`/`NOTIFY` |
| Graphile Worker | TypeScript, MIT | `LISTEN`/`NOTIFY`, `SKIP LOCKED`, PL/pgSQL and triggers |
| River | Go (and Ruby, Python clients), MPL-2.0 (run unmodified) | `LISTEN`/`NOTIFY`, `SKIP LOCKED`, leader election |
| Oban | Elixir, Apache-2.0 | Postgres 14+, `SKIP LOCKED`, its notifier |
| Hatchet | Go, MIT | Postgres; a full orchestrator |
| Procrastinate | Python, MIT | `LISTEN`/`NOTIFY`, `SKIP LOCKED` (verify) |

`LISTEN`/`NOTIFY` and session advisory locks do not survive PgDog's transaction pooling, so the recipes use the gateway's session-pooled or direct endpoint for workers (verify against D1207, which found advisory locks and `SKIP LOCKED` working in transaction mode for Mastra's usage). Each framework's own test suite (or its documented smoke tests) runs against Loams Postgres in OC0, with results in the compatibility matrix. Where a team wants Loams' leases, fencing, quotas and durable mode, it moves to the §26 plugins later.

### 8.3 Dapr (D1269)

Dapr 1.18.7 (2026-10-09; CNCF graduated 2024-11-12) runs **pluggable components** out of process over gRPC on a Unix socket: state stores (basic, transactional, ETags), pub/sub and bindings ([docs](https://docs.dapr.io/developing-applications/develop-components/pluggable-components/pluggable-components-overview/)). Dapr Workflow runs on `durabletask-go` over the actor state store; no pluggable workflow backend is documented. Loams' position (D182, D183) stands: `loams-dapr` checks tenant identity, Go `daprd` serves the long tail, **Dapr Workflow stays denied** because Resonate owns durable execution. **OC3 option (Q1040):** Loams as a Dapr pub/sub component over `loams.stream.v1` and a state store over `loams-kv`, about 6–8 engineer-weeks (estimate), after F1's `loams-dapr` exists and a user asks.

### 8.4 Knative, CloudEvents and Serverless Workflow (D1270)

- **Knative** graduated in CNCF on 2025-10-08. Loams already runs functions on Knative Serving (D441), delivers stream records to Knative sinks with `loams-knative-source` (D445), documents its produce endpoint as a sink, and ships no broker (Q443). No change: a Loams Broker class stays behind Q443's revisit.
- **CloudEvents** (graduated 2024-01-25; spec 1.0.2; HTTP, Kafka, AMQP, MQTT, NATS, WebSocket bindings) is already the envelope of every Loams stream (D270, D364). The gap is **CloudEvents SQL** (CESQL 1.0, 2024-06-13), the filter language Knative Triggers use. **OC3 option (Q1041):** CESQL becomes the filter syntax for stream subscriptions and Flow route filters when those filters are next changed; no separate project.
- **CNCF Serverless Workflow** (spec 1.0.x; SonataFlow still implements 0.8) is an open workflow DSL. It could become a declarative front end that compiles to Resonate functions; runtime maturity against 1.0 is **unverified**, so it is a long-term direction only (Q1042).

## 9. Relationship to the House, the engine and the catalog

### 9.1 One catalog, many engines (D1265)

Every table engine in this document points at **Lakekeeper's Iceberg REST catalog** (D6, D47, D413), with credential vending and the shared OpenFGA store (D67): Sail, RisingWave, JVM Spark with Comet or Gluten, Spice, Trino and DuckDB as readers, and the House's `lake` tables. Loams-maintained keyed tables keep one writer class (§17 §6.1): companions write their own tables, never the link targets. Collections (Lance) are reached through `format("loams")` (D54) and Flight SQL, not through the catalog. Delta tables are readable by Sail and the Delta connector (CN2), but Loams' own tables are Iceberg. Loams hosts no Unity Catalog or Polaris (Q1044 asks whether to answer Unity's REST API from Lakekeeper later).

### 9.2 Engines stay separate (D1266)

| Engine | Where it runs | Why not linked |
|---|---|---|
| Loams' engine (DataFusion 54, Lance, Tantivy) | The `loams` binary | — |
| House (chDB) | `loams-house-worker` processes (D761) | Process-global libchdb, Arrow 59 in `fabric/` (D343) |
| Sail (DataFusion 55, embedded CPython) | Its own process per namespace (D211) | DataFusion lockstep; tenant Python |
| RisingWave | Its own processes | A distributed system, not a library |
| Airflow, Temporal | Their own processes (Python, Go) | Other languages |

D51 (no embedded engines in the engine binary) and D343 (a Loams-owned service binary may link an engine library) hold unchanged. The **division of labour**: the engine serves retrieval and interactive SQL over collections and tables with consistency tokens; the House serves ClickHouse-dialect analytics; RisingWave keeps windowed state; Sail runs Spark programs; Airflow and Resonate orchestrate. They share data through the bucket and the catalog, not through code.

### 9.3 Metering (D1268)

Companions run as pods or processes labelled with §27's pod labels, so node CPU attributes to the namespace. OC2 adds one event type, `io.loams.dev.usage.companion.runtime.v1` (attributes `engine`, `cpu_ms`, `mem_gib_s`, `storage_gib_s`), emitted by the node agent per companion and namespace; the Meter gate (§63) reserves on start and stops a companion at a hard limit like any other workload. Jobs-backed work (the Airflow executor, `SparkBatch`, `StreamingSql`) keeps `…usage.jobs.run.v1`.

## 10. Deployment

### 10.1 Compose (OC0; D1267)

An opt-in `deploy/release/compose.compute.yaml`, used with `docker compose -f compose.yaml -f compose.compute.yaml --profile <name> up -d`, with one profile per companion: `airflow` (API server, scheduler, DAG processor, triggerer, one worker; Loams Postgres; logs and XComs in RustFS; DAGs from Loams Git), `temporal` (the four services on Loams Postgres; only if the spike passes), `risingwave` (all-in-one dev mode; Hummock in RustFS; meta on Loams Postgres), `sail` (a local Sail server against Lakekeeper). Every image is pinned by digest at a release at least 14 days old; nothing publishes a host port beyond the existing stack's rules; nothing starts without a profile. It is the CI test bed and a self-hosted recipe, not a supported production deployment.

### 10.2 Kubernetes (OC2; D1267)

One generic kind in `loams.dev/v1alpha1`, **`LoamsCompanion`**, owned by `LoamsNamespace` (§65), with `engine` (`airflow`, `temporal`, `risingwave`, `sail`), a pinned `version`, resources, and engine-specific blocks, rendered from each project's upstream chart or operator where one exists (Airflow's chart, Temporal's chart, RisingWave's operator, Sail's Kubernetes mode). One kind, not four, so the operator's conformance (LOC) covers them with one class (`stateless` plus an external store). The rule for sharing: **an engine that runs tenant code runs per namespace** (Sail, Airflow workers, RisingWave with UDFs); Temporal's server and RisingWave without UDFs may be shared with a database per namespace. Exposure follows D1195: `tailnet` or `internal` only, reached through `loams-gateway`.

## 11. Licences (D1271)

| Allowed to run unmodified, link, or copy with provenance | Run unmodified only | Never run, link or copy |
|---|---|---|
| Apache-2.0: Airflow, Sail, RisingWave, Comet, Gluten, Ballista, Daft, Spice, `datafusion-spark`, Lakekeeper, Dapr, Knative, Oban; MIT: Temporal server and most SDKs, Arroyo (also Apache-2.0), pg-boss, Graphile Worker, Hatchet, BullMQ; BSD-3: Celery, kombu, Valkey | MPL-2.0: River (unmodified, D-rule of §64) | BSL: Materialize, Estuary Flow; CCL: ksqlDB; RSALv2/SSPL/AGPL: Redis 8; ELv2: Airbyte's platform (D359) |

Vendor enterprise features are never defaults or documented as Loams features: RisingWave Premium, Feldera Enterprise, Spice.ai Enterprise, BullMQ Pro, Astronomer's runtime and agents, Temporal Cloud features. Temporal's CLA matters only if Loams upstreams server patches; Loams does not fork Temporal.

## 12. Testing

- **Where:** `conformance/compute/<product>/`, one directory per product with a `run.sh`, pinned versions in `conformance/compute/pins.env`, an expected-failure list (`xfail.toml`, strict: an unexpected pass fails), and a JUnit report.
- **When:** GitHub-hosted runners only. Per PR only for the files themselves (lint, shellcheck, Compose config validation); the suites run nightly and on demand (`.github/workflows/compute-compat.yml`), because Airflow, Temporal and RisingWave together exceed a per-PR budget.
- **What "passes" means:** the product's own tests or documented smoke tests, not Loams-written imitations; Loams adds only the HA and fault cases its primitives must survive (two Airflow schedulers, a killed Temporal history pod, a restarted RisingWave compute node reading Hummock back from the bucket).
- **Output:** `docs/guides/compute/compatibility.md`, generated from the last nightly, and later a page on loams.dev (Q1045).

## 13. What not to build (D1272)

- An Airflow-compatible scheduler, REST API, UI or DAG translator.
- A Temporal-compatible frontend, a Temporal fork, or Temporal on Loams SQL.
- A Spark Connect server of Loams' own in OC0–OC2; a Spark fork; Spice embedded in the engine (§17 §7 stands).
- A Redis or AMQP server in OC0–OC2; a managed Redis-compatible service.
- A stream processor in Loams before Q1036's gate (§00 §7 stands).
- A Unity Catalog or Polaris service; a second catalog.
- Dapr Workflow on Loams, or a durabletask backend.
- Any Materialize, Estuary, ksqlDB or Redis 8 code or binary.

## 14. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Focus: v0.1.x, preview.3, JB1, OP1 and GP0 compete | High | Medium | OC0 is tests and docs only; everything after is gated |
| R2 | Loams Postgres differs from vanilla Postgres under Airflow's or Temporal's locking | Medium | High | OC0 runs their own suites; recipes name the endpoint mode; failures go to the fork's backlog (§51) |
| R3 | Sail is pre-1.0 and one small company | Medium | High | Pinned versions, our own corpus, the JVM Spark fallback (D211), Comet recipe; Q1037 |
| R4 | RisingWave's footprint makes per-namespace hosting uneconomic | Medium | Medium | Shared clusters without UDFs (Q1035); Q1036's gate |
| R5 | Users read "runs Airflow" as "Loams is Airflow-compatible" | Medium | Low | §2.1's wording rule; the compatibility matrix is the claim |
| R6 | The Airflow Execution API moves between minors and breaks `LoamsJobsExecutor` | High | Medium | Pin a minor range; nightly against Airflow `latest`; the stock Celery or Kubernetes executors remain a fallback |
| R7 | Temporal persistence on the fork fails subtly under load | Medium | High | Spike plus soak before any recipe; no managed Temporal without the gate |
| R8 | A vendor moves a feature behind an enterprise licence | Medium | Medium | §11's rule; the licence gate (D359) checks every pinned version |
| R9 | Nightly suites are slow or flaky on GitHub-hosted runners | High | Low | Profiles run in parallel jobs; strict xfail lists; manual reruns |

## 15. Decisions (D1251–D1275, approved by the owner on 2026-10-21)

| ID | Decision |
|---|---|
| D1251 | **The protocol ladder:** L0 back it, L1 plug in, L2 host it, L3 speak it, L4 replace it; each product climbs only as far as the adoption test (installed base, openness, a gap users hit; for L3/L4 a reason the upstream cannot be hosted) justifies; refines D42 |
| D1252 | **Phases OC0–OC4** of §1.5: OC0 and OC1 near-term; OC2 per-engine demand gates; OC3 and OC4 long-term directions |
| D1253 | **OC0 ships no engine code:** conformance of unmodified Airflow, Temporal, RisingWave, Sail and Postgres-backed queues on Loams Postgres, the bucket and Lakekeeper (`conformance/compute/`), recipes (`docs/guides/compute/`), `compose.compute.yaml`, a generated compatibility matrix |
| D1254 | **The zero-change queue path:** Postgres-backed queues (BullMQ's Postgres backend, Celery's `db+` result backend, pg-boss, Graphile Worker, River, Oban, Hatchet, Procrastinate) are supported on Loams Postgres once their suites pass, using the gateway's session or direct endpoint where they need `LISTEN`/`NOTIFY` or session advisory locks |
| D1255 | **Celery and BullMQ keep §26's plugins** (D208, D209); no RESP or AMQP server and no managed Redis-compatible service in OC0–OC2; a Celery-only RESP subset is an OC3 option (Q1039) |
| D1256 | **Airflow runs as real Airflow** on Loams primitives (metadata DB on Loams Postgres with an HA scheduler gate, logs and XComs in the bucket, DAGs from Loams Git); never an Airflow look-alike |
| D1257 | **`airflow-provider-loams`** (Apache-2.0, third-party provider): `LoamsHook`, operators for SQL, House, Spark, streaming SQL, Durable and Jobs, `LoamsStreamTrigger` for `AssetWatcher`, `LoamsSecretsBackend`, and `LoamsJobsExecutor` on a Loams Jobs queue, pinned to tested Airflow minors |
| D1258 | **Managed Airflow** (OC2) through `LoamsCompanion` with the official chart, SSO through the gateway and upgrades with a Postgres branch snapshot; gated (Q1034) |
| D1259 | **Temporal:** Durable stays Resonate; no Temporal-compatible frontend; Temporal hosted on Loams Postgres only after OC0's persistence spike and soak pass (recipe first, managed behind Q1033); never on Loams SQL; a Temporal → Resonate migration guide |
| D1260 | **Streaming SQL stays RisingWave** (D22, D212, D341 reconfirmed): Hummock in the bucket, meta on Loams Postgres (verified in OC0), Iceberg and House sinks, reached over Postgres wire through `loams-gateway`'s `pg` SNI route; no Premium feature as a default |
| D1261 | **No native continuous-query engine now;** an L4 engine on DataFusion is a long-term direction behind Q1036, would start from Arroyo's MIT/Apache-2.0 code, and would amend §00 §7 and D341 only when approved |
| D1262 | **Sail is Loams' Spark** (D211 reconfirmed); Spice stays a Flight SQL peer (D56) with a recipe, not a Spark substitute; Comet and Gluten are recipes for JVM Spark; Ballista is not used (D15); no Spark Connect server of Loams' own before Q1037 |
| D1263 | **The Spark Connect proxy is `loams-gateway`'s `spark` module** (amends D211's "proxy in the gateway role"): bearer auth against Loams principals, namespace from the credential, sticky by `session_id`, start-on-connect, no plan parsing |
| D1264 | **Spark conformance:** `conformance/compute/spark/` runs our PySpark corpus (SQL, DataFrame, pandas UDF, Iceberg MERGE, Delta read, `format("loams")`) on pinned Sail with a strict expected-failure list; it is J3's acceptance input |
| D1265 | **One catalog:** every engine uses Lakekeeper's Iceberg REST catalog with credential vending; companions never write Loams-maintained keyed tables; no Unity Catalog or Polaris service |
| D1266 | **Engines stay separate processes** (D51, D343 hold): Sail, RisingWave, Airflow and Temporal are never linked into Loams binaries |
| D1267 | **Deployment:** an opt-in `deploy/release/compose.compute.yaml` with one profile per companion (OC0); one operator kind `LoamsCompanion` with `engine` (OC2); engines that run tenant code run per namespace; exposure `tailnet` or `internal` only (D1195) |
| D1268 | **Companion metering:** `io.loams.dev.usage.companion.runtime.v1` (engine, CPU, memory, storage time) from the node agent, gated by the Meter like other workloads; Jobs-backed work keeps `…usage.jobs.run.v1`; amends §63 §6.3 (OC2) |
| D1269 | **Dapr:** D182 and D183 stand; Dapr Workflow stays denied; Loams as a Dapr pub/sub and state-store pluggable component is an OC3 option (Q1040) |
| D1270 | **Knative and CloudEvents:** D441, D445 and Q443's answer stand; CloudEvents SQL becomes the filter syntax for subscriptions and Flow routes when they next change (Q1041); Serverless Workflow on Durable is a long-term direction (Q1042) |
| D1271 | **Licences:** §11's table; BSL, SSPL, ELv2, CCL, AGPL and RSAL products are never run, linked or copied; vendor enterprise features are never defaults; checked by D359's gate for every pin |
| D1272 | **Not built:** the list in §13 |
| D1273 | **Plan [OC1](../plans/2026-10-20-oc1-open-protocol-compute.md)** (stub) specifies OC0 task by task; OC1–OC4 get full specifications when their gates are met |
| D1274 | **Pins and CI:** every companion image and chart pinned by version and digest at a release at least 14 days old; suites nightly and on demand on GitHub-hosted runners; per-PR checks only for the files themselves |
| D1275 | **Wording:** docs say "runs Airflow / Temporal / RisingWave / Sail on Loams" and cite the matrix; never "Airflow-compatible", "Temporal-compatible" or "a Spark replacement" for code Loams did not write |

## 16. Open questions (Q1030–Q1045)

| ID | Question | Default (all answered by the owner on 2026-10-21: the default) |
|---|---|---|
| Q1030 | When does OC0 start? | After v0.1.1 ships, one engineer, beside GP0; it is tests and docs |
| Q1031 | Where does `airflow-provider-loams` live? | A public repository `ostrium-labs/loams-airflow` (the `loams-mastra` pattern), PyPI `airflow-provider-loams`, listed on Airflow's ecosystem page |
| Q1032 | Propose it as an Apache Airflow community provider? | No; third-party until it has users; revisit after OC2 |
| Q1033 | Managed Temporal: build it? | Only after the spike passes and three users ask; until then a recipe |
| Q1034 | Managed Airflow's gate? | Three deployments run `LoamsJobsExecutor` in production, or three users ask |
| Q1035 | RisingWave tenancy (refines D851's answer to Q96)? | Per namespace when UDFs are enabled; otherwise a shared cluster with a database per namespace and UDFs off |
| Q1036 | Build a native continuous-query engine (L4)? | No; revisit only if RisingWave per namespace is uneconomic **and** three users need windowed joins with consistency tokens |
| Q1037 | Build a Spark Connect endpoint on Loams' own engine (L3)? | No; revisit if Sail has no release for six months or a Loams-only capability needs it |
| Q1038 | Spice: anything beyond D56 and a recipe? | No; no hosting, no embedding; reconsider if Spice ships a Spark Connect server or users ask for it as a managed cache |
| Q1039 | A Celery RESP or AMQP 0-9-1 subset (L3)? | No; revisit if three users cannot change their broker URL |
| Q1040 | Loams as Dapr pluggable components? | After F1's `loams-dapr` exists and a user asks; pub/sub first |
| Q1041 | Adopt CloudEvents SQL for filters? | Yes, when subscription or Flow filters are next changed; no separate project |
| Q1042 | Serverless Workflow 1.0 as a front end to Durable? | Not now; revisit when a 1.0 runtime matures and users ask |
| Q1043 | Use `datafusion-spark` in Loams SQL for Spark function names? | Not until a user asks; it is in lockstep with DataFusion, so it can be added at any bump |
| Q1044 | Answer Unity Catalog's REST API (from Lakekeeper or a facade)? | No; Lakekeeper's Iceberg REST only |
| Q1045 | Publish the compatibility matrix on loams.dev? | Yes, generated from the nightly, after OC0 exits |

## 17. Amendments to earlier decisions

| Decision | Change |
|---|---|
| D42 | Refined by the ladder (D1251): D42's narrowing applies to L3; L0–L2 are open to any product that passes the adoption test |
| D211 | The Spark Connect proxy is `loams-gateway`'s `spark` module (D1263); J3's acceptance includes D1264's suite |
| D212, D22, D341 | Reconfirmed; RisingWave is reached through the gateway's `pg` route (D1260); tenancy per Q1035 |
| §17 §6.2, D56 | Reconfirmed; Spice is not a Spark path (D1262) |
| §21 §6.1, §26 §9.2 ("no Airflow needed") | Still true for Loams' own scheduling; Airflow users are now supported on Loams (D1256) |
| §33 (`plugin-airflow`, P3) | Superseded by `airflow-provider-loams` (D1257) |
| §63 §6.3 (event types) | Adds `io.loams.dev.usage.companion.runtime.v1` (D1268, OC2) |
| §65 §5.2 (kinds) | Adds `LoamsCompanion` (D1267, OC2) |

## 18. Effort (estimate, engineer-weeks)

| Phase | Content | Effort | Gate to start |
|---|---|---|---|
| OC0 | Harness, eight queue suites, Airflow, Temporal spike, RisingWave, Sail corpus, Compose file, recipes, matrix | 4–6 | v0.1.1 shipped (Q1030) |
| OC1 | Airflow provider (5–7, executor last); gateway `spark` route (2–3) and `pg` RisingWave route (1); Temporal migration guide (1); Comet and Spice recipes (1) | 10–13 | OC0 green; JB1's J1 core for the executor; GW1's `pg` module for the routes |
| OC2 | `LoamsCompanion` (3–4); managed Airflow (6–8); managed Temporal (4–6); RisingWave and Sail lifecycle (in JB1 J3/J4, not counted); metering (1–2) | 14–20 | OP1's `LoamsNamespace`; per engine Q1033, Q1034 |
| OC3 | Dapr components (6–8); CESQL (2–3); Celery RESP subset (4–6); Spark Connect on Loams' engine (26–52) | Each separately | Q1037, Q1039–Q1042 |
| OC4 | Continuous-query engine | 80–120 | Q1036 |
| **Near-term total (OC0–OC1)** | | **14–19** | |

## 19. Sources

Read on 2026-10-20 through the GitHub API (repository metadata, releases, LICENSE files), crates.io, PyPI and the projects' documentation:

- **Airflow:** [release notes](https://airflow.apache.org/docs/apache-airflow/stable/release_notes.html), [supported versions](https://airflow.apache.org/docs/apache-airflow/stable/installation/supported-versions.html), [prerequisites](https://airflow.apache.org/docs/apache-airflow/stable/installation/prerequisites.html), [executors](https://airflow.apache.org/docs/apache-airflow/stable/core-concepts/executor/index.html), [edge3 provider](https://airflow.apache.org/docs/apache-airflow-providers-edge3/stable/index.html), [public interface](https://airflow.apache.org/docs/apache-airflow/stable/public-airflow-interface.html), [security model](https://airflow.apache.org/docs/apache-airflow/stable/security/security_model.html), [DAG bundles](https://airflow.apache.org/docs/apache-airflow/stable/administration-and-deployment/dag-bundles.html), [event scheduling](https://airflow.apache.org/docs/apache-airflow/stable/authoring-and-scheduling/event-scheduling.html), [XCom object storage](https://airflow.apache.org/docs/apache-airflow-providers-common-io/stable/xcom_backend.html), [scheduler source](https://raw.githubusercontent.com/apache/airflow/main/airflow-core/src/airflow/jobs/scheduler_job_runner.py); [MWAA 3.3.1](https://aws.amazon.com/about-aws/whats-new/2026/09/amazon-mwaa-apache-airflow-3-3-1/), [MWAA pricing](https://aws.amazon.com/managed-workflows-for-apache-airflow/pricing/), [Composer versions](https://docs.cloud.google.com/composer/docs/composer-versioning-overview), [Astro remote execution](https://www.astronomer.io/docs/astro/remote-execution-overview); [Leoflow](https://pkg.go.dev/github.com/neochaotic/leoflow), [dagster-airlift](https://docs.dagster.io/migration/airflow-to-dagster/task-level-migration).
- **Temporal:** [server](https://github.com/temporalio/temporal), [API protos](https://raw.githubusercontent.com/temporalio/api/master/temporal/api/workflowservice/v1/service.proto), [persistence](https://raw.githubusercontent.com/temporalio/documentation/main/docs/encyclopedia/temporal-service/persistence.mdx), [visibility](https://docs.temporal.io/self-hosted-guide/visibility), [defaults](https://docs.temporal.io/self-hosted-guide/defaults), [pricing](https://docs.temporal.io/cloud/pricing), [CONTRIBUTING (CLA)](https://raw.githubusercontent.com/temporalio/temporal/main/CONTRIBUTING.md), [Rust SDK GA](https://temporal.io/blog/build-durable-applications-rust-temporal-rust-sdk-now-generally-available), [PgBouncer thread](https://community.temporal.io/t/is-it-safe-to-use-pool-mode-session-for-pgbouncer-for-postgres-with-temporal/12951); [Resonate](https://github.com/resonatehq/resonate), [coming from Temporal](https://docs.resonatehq.io/evaluate/coming-from/temporal); [TiDB SELECT](https://docs.pingcap.com/tidb/stable/sql-statement-select).
- **Streaming SQL:** [RisingWave](https://github.com/risingwavelabs/risingwave) and [LICENSE](https://raw.githubusercontent.com/risingwavelabs/risingwave/main/LICENSE), [v3.1.0](https://github.com/risingwavelabs/risingwave/releases/tag/v3.1.0), [architecture](https://docs.risingwave.com/get-started/architecture), [compose and S3](https://docs.risingwave.com/deploy/risingwave-docker-compose), [SQL meta backend](https://docs.risingwave.com/deploy/migrate-to-sql-backend), [Iceberg](https://docs.risingwave.com/iceberg/overview), [sources](https://docs.risingwave.com/ingestion/overview), [sinks](https://docs.risingwave.com/delivery/overview); [Arroyo](https://github.com/ArroyoSystems/arroyo), [Cloudflare acquisition](https://blog.cloudflare.com/cloudflare-acquires-arroyo-pipelines-streaming-ingestion-beta/); [Materialize LICENSE](https://raw.githubusercontent.com/MaterializeInc/materialize/main/LICENSE); [Feldera LICENSE](https://raw.githubusercontent.com/feldera/feldera/main/LICENSE); [Estuary LICENSE-BSL](https://raw.githubusercontent.com/estuary/flow/master/LICENSE-BSL); [ksqlDB LICENSE](https://raw.githubusercontent.com/confluentinc/ksql/master/LICENSE); [DataFusion DDL](https://datafusion.apache.org/user-guide/sql/ddl.html).
- **Spark:** [Spice](https://github.com/spiceai/spiceai), [README](https://raw.githubusercontent.com/spiceai/spiceai/trunk/README.md), [Spark connector](https://spiceai.org/docs/components/data-connectors/spark), [Ballista at Spice](https://spice.ai/blog/apache-ballista-at-spice-ai), [distributed query](https://spice.ai/docs/enterprise/features/distributed-query); [Sail](https://github.com/lakehq/sail), [LICENSE](https://raw.githubusercontent.com/lakehq/sail/main/LICENSE), [docs](https://docs.lakesail.com/sail/latest/), [catalogs](https://docs.lakesail.com/sail/latest/guide/catalog/), [v0.7.2](https://github.com/lakehq/sail/releases/tag/v0.7.2); [Comet 1.0.0](https://datafusion.apache.org/blog/2026/08/07/datafusion-comet-1.0.0); [Ballista 54.0.0](https://datafusion.apache.org/blog/2026/07/12/datafusion-ballista-54.0.0); [Daft Spark Connect](https://docs.daft.ai/en/stable/api/spark_connect/); [Gluten LICENSE](https://raw.githubusercontent.com/apache/gluten/main/LICENSE); [Spark Connect overview](https://spark.apache.org/docs/latest/spark-connect-overview.html); [`datafusion-spark`](https://crates.io/api/v1/crates/datafusion-spark); [iceberg-rust status](https://iceberg.apache.org/status/); [Lakekeeper](https://github.com/lakekeeper/lakekeeper).
- **Queues and events:** [Celery protocol](https://docs.celeryq.dev/en/stable/internals/protocol.html), [kombu Redis transport](https://raw.githubusercontent.com/celery/kombu/main/kombu/transport/redis.py), [kombu SQLAlchemy](https://docs.celeryq.dev/projects/kombu/en/stable/reference/kombu.transport.sqlalchemy.html); [BullMQ compatibility](https://docs.bullmq.io/guide/redis-tm-compatibility/), [BullMQ scripts](https://github.com/taskforcesh/bullmq/tree/master/src/commands), [Dragonfly and BullMQ](https://dragonflydb.io/docs/integrations/bullmq); [Redis README](https://raw.githubusercontent.com/redis/redis/unstable/README.md), [Valkey COPYING](https://raw.githubusercontent.com/valkey-io/valkey/unstable/COPYING); [pg-boss](https://raw.githubusercontent.com/timgit/pg-boss/master/README.md), [Graphile Worker](https://worker.graphile.org/docs), [River](https://riverqueue.com/docs), [Oban](https://oban.hexdocs.pm/Oban.html), [Hatchet](https://docs.hatchet.run/home); [Dapr releases](https://github.com/dapr/dapr/releases), [pluggable components](https://docs.dapr.io/developing-applications/develop-components/pluggable-components/pluggable-components-overview/), [Dapr workflow](https://docs.dapr.io/developing-applications/building-blocks/workflow/workflow-architecture/); [Knative graduation](https://www.cncf.io/announcements/2025/10/08/cloud-native-computing-foundation-announces-knatives-graduation/), [Knative brokers](https://knative.dev/docs/eventing/brokers/); [CloudEvents graduation](https://www.cncf.io/announcements/2024/01/25/cloud-native-computing-foundation-announces-the-graduation-of-cloudevents/), [spec](https://github.com/cloudevents/spec/releases), [sdk-rust](https://github.com/cloudevents/sdk-rust); [Serverless Workflow](https://github.com/serverlessworkflow/specification/releases).
- **Loams** at `dev` (`342179be3`): designs §00, §05, §14, §17, §21, §24, §26, §32, §33, §38, §49, §57, §58, §63–§67; plans JB1, D1, MA1; `deploy/release/compose.yaml`.
