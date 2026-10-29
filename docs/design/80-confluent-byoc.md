# 80 — Confluent Cloud with Loams BYOC

Status: **Approved** (owner, 2026-10-26, with all defaults; Q1333: $2,400 of credits) · proposed 2026-10-25. D1820–D1835 are approved and Q1333–Q1341 answered (§17).

Source: the owner, 2026-10-25: "I want Confluent Cloud integration with BYOC where users can host Loams alongside their [Confluent] region with a budget limit and use [it]. I have $2,400 credits and want to use them. Also I have HashiCorp free tier so we can use it."

Decisions **D1820–D1835** (D1836–D1839 kept free) and open questions **Q1333–Q1341** are declared in the [decision log](13-decision-log.md). **Numbering:** track HE reserved D1800–D1819 and Q1320–Q1329 (§75); open PR #1231 (MS1) uses D1806–D1814 and Q1330–Q1332; D1815 is taken (HE1 R14.4). This document starts after the highest taken number and takes the number 80.

Plan: [CC1](../plans/2026-10-25-cc1-confluent-byoc.md). The track id is **CC** (Confluent Cloud); **CF** is taken by the console foundation (§59, CF1 is its Part 1 of 4), so "CF2" would read as that track's Part 2.

This document builds on:
- [§70](70-byoc-clouds.md) and plan [BY1](../plans/2026-10-21-by1-byoc-clouds.md): BYOC on AWS, Google Cloud and Azure; the VM tier; the OpenTofu modules `deploy/tofu/modules/{aws,gcp,azure}-vm` (built, validated offline, never applied); the secrets fetcher `deploy/byoc/lib/loams-byoc-secrets` (D1358); the never-public rule (D1359); cost guards for customers (D1381) and for our test accounts (D1380).
- [§33](33-connectors.md) and plans CN1/CN2: the native Kafka family `fabric/crates/loams-connector-kafka` (Apache Kafka, with Redpanda and Azure Event Hubs as target profiles), its Confluent Schema Registry client, the SDK's `Pipeline`, rate limiter and secret resolver.
- [§32](32-loams-flow-fabric-house.md) §5.10 (D1796–D1798): Kafka wire in the Ostrium fork of Iggy, run as the optional `kafka` compose profile; `loams_sink` (#1184, the Iggy to Loams reader).
- [§64](64-loams-infra-stack.md) (D1150: OpenTofu is the IaC Loams supports; D1153: state backends) and [§78](78-loams-cloud-on-tidb-x.md) §16 (D1729, Q1273: no HCP product dependency; OpenTofu with R2 state for Ostrium's infrastructure; HCP Terraform Free optional as a state-only backend).
- [§69](69-edge-reach.md) (D1311: never public) and [§10](10-operations.md) (operations).
- The Confluent oracle notes of 2026-10-25 (`loams-wt/.he0-shared/confluent-oracle.md`, outside the repository, ids only) and the fork's `docs/loams/confluent-oracle.md` on `ostrium-labs/iggy`: one Basic cluster `lkc-120zwvj` in AWS `ap-south-1`, `max_ecku` 1, a service account scoped by prefixed ACLs, no Schema Registry, no connectors.

Markers, as in §70:
- **(verify)**: not checked against a primary source; the task that depends on it checks it first.
- **(estimate)**: judged, not measured.
- **(unverified)**: a search on 2026-10-25 did not settle it.

Vendor facts were read on **2026-10-25** from the page linked beside each one. A date after a link is the page's own "last published" date. Prices are list prices in USD, exclude tax and credits, and vary by region ("Prices vary by cloud region", [Confluent pricing](https://www.confluent.io/confluent-cloud/pricing/)).

---

## 1. Summary

### 1.1 The problem

Many teams that would run Loams already run Kafka on Confluent Cloud: Debezium CDC topics, event streams, and the apps that produce and consume them. Today Loams has the pieces to talk to them but no path a user can follow:
- `loams-connector-kafka` speaks SASL/TLS and the Confluent Schema Registry's wire format, but **nothing runs connector instances on a BYOC VM**: the release image's `loams-fabric` has only the `house` role, and `loams-flow`'s `NativeRuntime` is "not built yet" (`fabric/crates/loams-flow/src/runtime/mod.rs`).
- The BYOC modules create a VM, a bucket, a key and secrets, but nothing for Kafka, and nothing decides where a Confluent cluster sits relative to the VM.
- Confluent Cloud has **no budget or spend-cap API**: its notifications cover free-trial use and service quotas, not spend ([notifications](https://docs.confluent.io/cloud/current/monitoring/configure-notifications.html), 2026-09-25), and its billing page describes no budget feature ([billing](https://docs.confluent.io/cloud/current/billing/overview.html), 2026-09-25). The owner asked for a budget limit.
- Ostrium has Confluent for Startups credits for one year and a HashiCorp free tier, and no plan for using either.

### 1.2 The answer

- **Integration means three things, built in this order** (§4, D1820): **(a)** Loams reads from and writes to the user's own Confluent cluster through the existing Kafka connectors (CDC and events in, events out); **(c)** a documented choice between Confluent and the Iggy `kafka` profile as the Kafka endpoint for the user's apps; **(b)** the BYOC installer optionally provisions a Basic or Standard cluster in the same cloud and region as the Loams VM, with scoped service accounts, keys, topics and ACLs. Confluent is always an edge of Loams, never its internal log: the bucket stays the source of truth (D1), and a Loams install keeps working when Confluent is unreachable.
- **A small connector runtime on the VM** (D1821): a `loams-fabric connect` role that runs native connector instances with the SDK's `Pipeline`, reading and writing Loams streams through the engine's `ProduceCloudEvents` and `FetchCloudEvents` (dedup by `source` + `id`), secrets from `FileSecretStore`, and a loopback admin API with pause, resume and rate limits. CN2's `NativeRuntime` absorbs it later.
- **A `confluent-cloud` target profile and Debezium decoding** (D1822): `sasl_ssl` with PLAIN and full verification, Schema Registry with basic auth, and a `format: debezium` option on the Kafka source that turns Debezium change events into Loams change events.
- **An OpenTofu module on Confluent's own provider** (D1823): `confluentinc/confluent` is **Apache-2.0** and is served by the OpenTofu registry (v2.91.0 on 2026-10-25), so Loams depends on it as it depends on the cloud providers. It composes with `<cloud>-vm`, takes the VM's cloud and region, and creates Basic by default, Standard on request, and refuses Enterprise and Dedicated unless asked twice.
- **Co-location by region code** (D1824): Confluent uses the clouds' own region codes, and Basic and Standard run in every listed AWS, Azure and Google Cloud region ([regions](https://docs.confluent.io/cloud/current/clusters/regions.html), 2026-10-23), so the cluster's region is the VM's region, checked against a pinned table.
- **Networking stays outbound-only** (D1825): public endpoints with TLS and SASL on Basic and Standard; PrivateLink only on Enterprise or Dedicated, opt-in, with its price stated. Nothing in Confluent ever connects into Loams.
- **Secrets never land in an image, user data, a log or plain state** (D1826, D1827): API keys go into the user's cloud secret store, the BY1 fetcher puts them into the Fabric's file secret store at boot, the module path that creates a key requires OpenTofu's enforced state encryption, and workload identity (OAuth, no key) is the target.
- **Budget limits, split honestly into hard and best-effort** (§8, D1828–D1830): the cluster type, `max_ecku`, topic retention and partition counts, and Loams' own connector rate limits are hard; throughput on Basic and Standard is not hard-capped by Confluent (client quotas are Enterprise, Freight and Dedicated only). A **budget guard** estimates spend hourly from the Metrics API and reconciles daily with the Costs API, then alerts at 50% and 80%, throttles Loams' connectors at 90%, and, opt-in, pauses them at 100%. It never deletes anything. Spend shows in the CLI, `loams byoc status`, an API and a console card.
- **Our credits pay for testing and demos, not for users** (§9, D1831): a nightly BYOC + Confluent round trip on an ephemeral Basic cluster in `us-east-1` next to the AWS nightly VM, a weekly Standard row, a monthly Enterprise + PrivateLink row, the Kafka gateway oracle, design-partner demos and a quarterly benchmark, at about **$60 a month planned and $100 a month at most**, well inside $2,400 / 12 = $200. Users bring their own Confluent account; credits are tied to Ostrium's organisation.
- **HashiCorp** (§10, D1833): OpenTofu stays the engine (MPL-2.0); no Terraform CLI and no BSL binary is shipped or required. The only lasting free HCP product is HCP Terraform Free (500 managed resources, one concurrent run). R2 state stays the default for our infrastructure (D1729); HCP Terraform Free is allowed as a state-only backend for one small root if the owner wants its UI (Q1336). Users never need HCP.

### 1.3 Decisions

| # | Decision | § | Status |
|---|---|---|---|
| D1820 | **Confluent Cloud is an external Kafka edge of Loams BYOC, in three modes:** (a) connectors to the user's own cluster, (c) Confluent instead of (or beside) the Iggy `kafka` profile, (b) an installer-provisioned cluster next to the VM. Built a → c → b. Confluent is never Loams' internal log or a dependency of the default stack | 4 | Approved (owner, 2026-10-26) |
| D1821 | **`loams-fabric connect`:** a VM connector runtime (native instances only) on the SDK's `Pipeline`, with `FabricWriter`/`FabricReader` over the engine's stream gRPC (`ProduceCloudEvents`, `FetchCloudEvents`), `FileSecretStore`, instance and checkpoint state in the bucket, and a loopback admin API (status, pause, resume, rate); the `connectors` compose profile; CN2's `NativeRuntime` absorbs it | 4.4 | Approved (owner, 2026-10-26) |
| D1822 | **The `confluent-cloud` target profile and Debezium decoding:** a profile on `loams-connector-kafka` (`sasl_ssl`, PLAIN, `verify_full`, Schema Registry with basic auth, no narrows unless a test finds one); `format: debezium-json \| debezium-avro` on the Kafka source maps Debezium envelopes to Loams change events | 4.2 | Approved (owner, 2026-10-26) |
| D1823 | **The `confluent` OpenTofu module** in `deploy/tofu/modules/confluent/` on `confluentinc/confluent` (Apache-2.0, from the OpenTofu registry, pinned to a release at least 14 days old): an environment (new or existing), a Basic (default) or Standard cluster with `max_ecku`, topics with retention limits, a data service account with prefix ACLs, an optional metrics service account; Enterprise, Dedicated and Freight refused unless `allow_premium_cluster_types = true` | 5 | Approved (owner, 2026-10-26) |
| D1824 | **Co-location:** the cluster's cloud and region are the VM module's; the module refuses another cloud or region unless `allow_cross_region = true`, and prints the data-transfer warning; a pinned region table, refreshed by a weekly job, says which types each region offers | 6.1 | Approved (owner, 2026-10-26) |
| D1825 | **Networking:** public endpoints with TLS and SASL on Basic and Standard by default, reached outbound from the VM (consistent with D1359); PrivateLink only on Enterprise or Dedicated, opt-in (`confluent_networking = "privatelink"`), with its cost stated; no path from Confluent into Loams, so Confluent-managed connectors that would pull from a Loams database are not supported | 6.2 | Approved (owner, 2026-10-26) |
| D1826 | **Least-privilege identities:** per install, a data service account with Kafka ACLs only (no topic CREATE or DELETE, no ALTER_CONFIGS, no ACL operations, a fixed topic list or prefix); a metrics service account with MetricsViewer; an optional ops account (CloudClusterAdmin on that one cluster) only for the guard's `max_ecku` throttle; no billing key on the VM by default. Basic has no in-cluster RBAC, so ACLs scope everything | 7.1 | Approved (owner, 2026-10-26) |
| D1827 | **Secrets:** Confluent keys are never in an image, cloud-init, a log or plain state. `loams byoc confluent key create\|rotate` creates keys with the user's own Confluent credentials and writes them straight into the cloud secret store; the fetcher writes them into the Fabric's file secret store (mode 0600); the module's `create_api_keys = true` path is allowed only with OpenTofu state encryption `enforced = true` on the cloud's KMS; OAuth workload identity (no key) is the target once the Kafka connector speaks OAUTHBEARER | 7.2 | Approved (owner, 2026-10-26) |
| D1828 | **Hard limits are only what Confluent or Loams enforces:** cluster type, `max_ecku`, topic count, partitions and `retention.bytes` set by the module and not changeable by Loams' key, and Loams' own connector byte rates. The module outputs `confluent_ceiling_monthly_usd` for what is bounded and says what is not (throughput on Basic and Standard) | 8.1 | Approved (owner, 2026-10-26) |
| D1829 | **The budget guard:** hourly estimate from the Metrics API and a pinned price table, daily reconciliation with the Costs API (when a billing reader is configured), a monthly budget per integration; 50% and 80% alert, 90% throttle (Loams' connectors to a floor rate; optionally `max_ecku` to the type's minimum), 100% pause of Loams' Confluent instances (opt-in, off by default); resume at the month's start or by command; never deletes a cluster, topic or key | 8.2 | Approved (owner, 2026-10-26) |
| D1830 | **Spend is visible:** `loams confluent cost`, a line in `loams byoc status`, `GET /v1/integrations/confluent/spend` on the connect role's admin API, a console card; every number labelled estimate or reconciled, with its date | 8.3 | Approved (owner, 2026-10-26) |
| D1831 | **Ostrium's credits:** spent only on Ostrium's tests, oracles, demos and benchmarks under the plan of §9 (about $60 a month, at most $100), in three Confluent environments (`default` for the oracle, `loams-ci`, `loams-demo`), with names `loams-ci-<run>-*`, a 3-hour TTL, teardown in `always()`, an hourly reaper and Ostrium's own budget guard; users bring their own Confluent account and are billed by Confluent | 9 | Approved (owner, 2026-10-26) |
| D1832 | **CI access to Confluent:** a GitHub environment `byoc-confluent` restricted to `dev`; a CI service account with EnvironmentAdmin on `loams-ci` only and a Cloud API key in the environment's secrets until GitHub OIDC into a Confluent identity pool is shown to work (then keyless); the Costs API reader runs on the local CI host, not in GitHub | 9.3 | Approved (owner, 2026-10-26) |
| D1833 | **HashiCorp:** OpenTofu stays; no Terraform CLI in CI, docs or images, no BSL code or binary (D1150, D1729); R2 state stays the default for Ostrium's infrastructure; HCP Terraform Free is allowed only as a state-only backend (local execution) for Ostrium's persistent test root; HCP Vault, Boundary and Packer are not used; nothing a user installs needs HCP | 10 | Approved (owner, 2026-10-26) |
| D1834 | **Docs and site:** `docs/guides/byoc/confluent.md`, `docs/guides/connectors/confluent-cloud.md`, a Kafka-endpoint decision table in `docs/guides/kafka.md`, a Confluent section on loams.dev's AWS, Google Cloud and Azure deploy pages, and an optional Confluent add-on in the `/cost` calculator from a dated price table | 12 | Approved (owner, 2026-10-26) |
| D1835 | **Phases CC0–CC3 and what is not built** (§13, §14): no Confluent marketplace or partner listing (as D1580), no Confluent-hosted Loams, no Kafka Connect worker, no ksqlDB or Flink integration, no use of Ostrium's credits for users | 13, 14 | Approved (owner, 2026-10-26) |

### 1.4 Phases

| Phase | What | Effort (estimate) |
|---|---|---|
| **CC0** | Rulings and spikes on Ostrium's credits (EOS on Basic, the provider on OpenTofu, region API, OAuth support per type, Metrics API breakdowns, cloud-side data-transfer prices); this design approved | 3–4 days |
| **CC1** | `loams-fabric connect` and the `connectors` profile; the `confluent-cloud` profile; Debezium decoding; secrets; the `confluent` module; the nightly AWS + Basic round trip; the guides | 4–5 engineer-weeks |
| **CC2** | The budget guard, spend surfaces, the site's deploy sections and calculator; the weekly Standard row | 2 engineer-weeks |
| **CC3** | OAuth workload identity (no keys), the Enterprise + PrivateLink option and its monthly row, the Google Cloud and Azure rows once those accounts exist | 2–3 engineer-weeks |

## 2. Scope

**In scope:** a Loams BYOC install (VM tier first, §70 §5) using a Confluent Cloud Kafka cluster as a source and sink; optionally creating that cluster; region co-location and networking; secrets and identities; budget limits and spend visibility; how Ostrium spends its Confluent credits; how Ostrium uses its HashiCorp free tier.

**Not in scope:**
- Confluent Platform (self-managed). It is ordinary Kafka to the connectors and needs nothing here.
- Kafka Connect workers, ksqlDB, Flink and Tableflow on Confluent. Users may run them; Loams does not drive them.
- Confluent-managed connectors (for example the Debezium PostgreSQL CDC source) pointed at a Loams database: they connect inbound, which D1359 rules out (§6.2).
- The container and Kubernetes tiers (§70 BY1, BY2). The connect role and the module are written so those tiers can reuse them; their rows come with those tiers.
- A Kafka gateway inside the engine (D74, deferred by D408) and the Iggy fork's work (§32 §5.10).

## 3. Where things are today (checked on `dev` at `b518dd9dc`, 2026-10-25)

| Piece | State |
|---|---|
| `fabric/crates/loams-connector-kafka` | Built (CN2 Task 9): source with topic-partition splits and offsets committed after the Fabric acknowledges; sink with the idempotent producer or Kafka transactions (`__loams_checkpoints`); `security_protocol` `ssl`/`sasl_ssl`, SASL PLAIN and SCRAM, mTLS; OAUTHBEARER not supported; the Confluent Schema Registry client with optional basic auth (`registry.rs`). Tested against `apache/kafka:4.3.1`; Redpanda and Event Hubs as target profiles (`connectors/profiles/`) |
| SDK pieces | `Pipeline`, `RateLimit { requests, bytes }` per instance (`ratelimit.rs`), `SecretResolver` and the `Scrubber` |
| `loams-flow` | Manifests, validation, `FileSecretStore` (`<root>/<namespace>/<secret>/<key>`, owner-only files) and `KubeSecretStore`; the Kubernetes renderer. **No `NativeRuntime`** and no pause API |
| `loams-fabric` binary | One role, `house` (`fabric/crates/loams-fabric/src/main.rs`). The release compose runs it only as House |
| Engine streams | `StreamService` with `ProduceCloudEvents` and `FetchCloudEvents` (`proto/loams/stream/v1/stream.proto`), dedup by `source` + `id` within a window; `stream-grpc` is in the release features (`release-package.yml`) |
| `kafka` compose profile | Built (KW-L2): Iggy and the Kafka gateway from the fork, loopback, preview, no transactions; not wired to the engine (`docs/guides/kafka.md`) |
| BYOC | BY0 Tasks 3–7 merged; the VM modules built and validated offline only; the secrets fetcher (curl + jq, three clouds); the AWS nightly and reaper workflows exist but wait on the owner's AWS checklist; `budget_monthly_usd` not built (D1381) |
| Confluent at Ostrium | One Basic cluster (`lkc-120zwvj`, AWS `ap-south-1`, `max_ecku` 1) for the Kafka gateway oracle, report-only, run from the owner's machine; credits recorded as $2,000 in the oracle notes and as $2,400 by the owner; the owner confirmed $2,400 (Q1333, 2026-10-26) |
| Site (loams-site `main` at `24b9204`) | Deploy pages for AWS, Google Cloud, Azure, Cloudflare and Vercel; `/cost` calculator (`components/cost/calculator.tsx`, `lib/cloud-prices`) with no Kafka line |

## 4. What integration means for users (D1820)

### 4.1 The options

| | (a) Connect to the user's cluster | (b) Provision a cluster next to Loams | (c) Confluent instead of the Iggy `kafka` profile |
|---|---|---|---|
| **Who it is for** | Teams that already run Confluent: Debezium CDC topics, event streams, existing producers | Teams that want Kafka and do not have it yet, or want a cluster for Loams only | Teams choosing a Kafka endpoint for their own apps next to Loams |
| **What Loams does** | Kafka source instances read topics (CDC or events) into Loams streams, from which links fill collections and tables; Kafka sink instances write Loams streams to topics | The `confluent` module creates the environment (or uses one), the cluster, the topics, the service accounts, the ACLs and the keys' secret slots, then (a) | Nothing new in code: the guide's decision table; `install.sh --kafka confluent\|iggy\|none` sets the profile and writes connector instances |
| **Cost to the user** | Their existing Confluent bill plus Loams' traffic (§8) | A new Confluent bill: Basic from $0 (first eCKU free), Standard from $0.75 per eCKU-hour | Confluent's bill instead of Iggy's VM resources |
| **Works without (b)** | Yes, any cluster type, any cloud | — | Yes |
| **Risk** | Low: connectors exist; the runtime does not (D1821) | Medium: a vendor API in the installer, a new bill the user did not expect | Low |

**Recommendation:** all three, in the order a → c → b. (a) is what existing Confluent users need and reuses built code; (c) is documentation and an installer flag; (b) is a convenience on top of (a) and is the only mode that creates spend on the user's behalf, so it comes with the budget work (§8).

**Not adopted:** Confluent as the Fabric's log (replacing Iggy or Fluss inside Loams). It would make a Loams install depend on a vendor service for its own writes, which contradicts D1 and §70's principle 2, and it would put every internal event on the user's Confluent bill.

### 4.2 Data paths (D1822)

```
user apps ──produce──▶ Confluent topic ──Kafka source──▶ Loams stream ──links──▶ collection / table / graph
Debezium (user's)  ──▶ CDC topics      ──Kafka source, format: debezium──▶ Loams stream (change events)
Loams stream ──Kafka sink (idempotent or transactional)──▶ Confluent topic ──▶ user apps
Loams Postgres ──Postgres CDC source──▶ Loams stream ──Kafka sink──▶ Confluent topic   (CDC out, outbound only)
```

- **The `confluent-cloud` target profile** (`connectors/profiles/confluent-cloud.yaml`, module `kafka`): defaults `security_protocol: sasl_ssl`, `sasl.mechanism: plain` with the API key as the username and a secret reference for the API secret, `tls.verify_full: true` with the system roots (`ssl.ca.location=probe`), `max_record_bytes` within the cluster's limit (Basic and Standard accept 8 MB messages, [cluster types](https://docs.confluent.io/cloud/current/clusters/cluster-types.html), 2026-10-20), and an optional `schema_registry` block with basic auth.
- **Exactly-once sinks** need transactions and the compacted `__loams_checkpoints` topic. Basic supports compaction (up to 1,500 compactable partitions, same page). Exactly-once on Basic is shown in Confluent's feature table according to a search on 2026-10-25, but the page read did not state it (verify in CC0 Task 0 on the oracle cluster; if it fails, the profile narrows `sink.transactional` as Event Hubs' does).
- **Debezium decoding:** a Kafka record whose value is a Debezium envelope (`before`, `after`, `op`, `source`, `ts_ms`; JSON with or without the schema wrapper, or Avro through the Schema Registry) becomes a Loams change event with `loamsop` `c`/`u`/`d`/`r`, the key from the record key, and the source position from `source` (LSN, binlog position or SCN). Tombstones become deletes or are dropped by configuration. This lets a user's existing Debezium topics feed collections and tables with no Loams CDC connector against their database.
- **Ordering and delivery** are the Kafka family's: per partition, at-least-once into the stream, deduplicated by `<topic>/<partition>/<offset>` in `ProduceCloudEvents` (effectively once).

### 4.3 Confluent or the Iggy `kafka` profile (mode c)

| | Iggy `kafka` profile (D1798) | Confluent Cloud |
|---|---|---|
| Runs | On the Loams VM, loopback by default | Confluent's cloud, public or private endpoint |
| Kafka compatibility | Preview; no transactions or exactly-once (planned in the fork) | Kafka itself; transactions (verify on Basic) |
| Cost | The VM's CPU, memory and disk | Confluent's bill (§8) |
| Wired to the engine | Not yet (FL1's `loams_sink`) | Through the Kafka connectors (D1821) |
| Durability | One node on one disk; back up `iggy-data` | Replicated; 99.5% SLA on Basic, 99.9% on Standard (same page) |

`install.sh --kafka iggy|confluent|none` (default `none`) chooses; `iggy` and `confluent` together is allowed only with `--kafka iggy,confluent` and means Iggy for local apps and connectors to Confluent. Neither is in the default stack (D1798 stands).

### 4.4 The VM connector runtime (D1821)

Mode (a) needs something that runs connector instances on a BYOC VM, and nothing does today. Options:

| Option | For | Against |
|---|---|---|
| **1. Wait for CN2's `NativeRuntime` and FL1's Fabric** | One runtime, built once | Both Planned with no date; FL1 brings Iggy and Fluss, which the light stack deliberately leaves out |
| **2. A `loams-fabric connect` role now (recommended)** | Small: the SDK's `Pipeline` already does budgets, retries, checkpoints, the DLQ and rate limits; the engine's `ProduceCloudEvents` and `FetchCloudEvents` already dedup; `FileSecretStore` exists | Another role to maintain until CN2 absorbs it |
| 3. A Kafka Connect worker beside Loams | Users know it | Java, a second connector model, the BSL/Confluent Community licence questions for many connectors; contradicts §33's native-first rule |

**Recommendation: option 2,** scoped so CN2 Task 4's `NativeRuntime` replaces its scheduler and keeps everything else:
- **Role:** `loams-fabric connect --instances /etc/loams/connect/*.yaml` in the existing `loams-fabric` image, as the `connectors` compose profile (off by default, D1798's rule for optional profiles).
- **Fabric I/O:** a `FabricWriter` over `StreamService.ProduceCloudEvents` and a `FabricReader` over `FetchCloudEvents`, on the engine's **connect socket** (CC1 ruling R1.1, 2026-10-26, amending this bullet's earlier "`connect` shares the engine's network namespace": a process there could forge Tailscale identity headers on the serve target): a Unix socket in a volume only the engine and `connect` mount, which takes a Loams API key and nothing else; the key from `secrets/`.
- **State:** checkpoints and instance status in the bucket under `_meta/connect/<instance>/`, written with create-only versions (the conditional writes the store probe checks, D1356); one process per install (single writer, like the light stack).
- **Admin API** on loopback (and the tailnet for the console): `GET /v1/connect/instances`, `POST …/{id}:pause`, `…:resume`, `PATCH …/{id}` for `rate_limit.bytes_per_sec`; the guard (§8.2) and the CLI use it.
- **Families allowed:** native only (Kafka, Postgres, MySQL and others with a native runtime); fallback runtimes (Camel, Debezium Server) stay Kubernetes-only (CN1).
- **Egress:** the instance's egress allow-list names the cluster's bootstrap host and port and the Schema Registry host (the SDK's `check_egress`).

## 5. Provisioning with OpenTofu (D1823)

### 5.1 The provider

| Fact | Value | Source |
|---|---|---|
| Licence | **Apache-2.0** (`LICENSE` in the repository; GitHub reports `Apache-2.0`) | [confluentinc/terraform-provider-confluent](https://github.com/confluentinc/terraform-provider-confluent), read 2026-10-25 |
| Built on | `hashicorp/terraform-plugin-sdk/v2` v2.16.0, an MPL-2.0 SDK (HashiCorp kept its SDKs on MPL when the products moved to BSL, [announcement](https://www.hashicorp.com/en/blog/hashicorp-adopts-business-source-license), 2023-08-10) | `go.mod` at `master`, read 2026-10-25 |
| On the OpenTofu registry | Yes, up to 2.91.0 | [registry API](https://registry.opentofu.org/v1/providers/confluentinc/confluent/versions), read 2026-10-25 |
| Releases | v2.91.0 2026-10-14, v2.90.0 2026-10-08, v2.89.0 2026-10-06 | GitHub releases API, read 2026-10-25 |
| Ephemeral resources or write-only attributes | None: the docs have no `ephemeral-resources`, and `confluent_api_key.secret` is a sensitive attribute stored in state | `docs/` at v2.89.0 |
| OAuth | The provider block accepts `oauth { oauth_external_access_token, oauth_identity_pool_id }` or a client-credentials flow; a few resources (catalog integration, custom connector plugins, Flink artefacts, Tableflow) still need a Cloud API key | `docs/index.md` at v2.89.0 |

Loams depends on the provider binary from the registry, as BY1 does on `hashicorp/aws` (D1362); no provider source is copied. The pin is a release at least 14 days old (2.89.0 on 2026-10-25), moved by Dependabot with the same rule. OpenTofu only (D1150).

### 5.2 The module

```
deploy/tofu/modules/confluent/     variables.tf  main.tf  outputs.tf  versions.tf  tests/
deploy/tofu/examples/aws-vm-confluent/   (aws-vm + confluent; gcp-vm-confluent and azure-vm-confluent in CC3)
```

**Inputs:** `name`; `cloud` and `region` (wired from the VM module's outputs); `environment_id` (an existing environment, or empty to create `loams-<name>`); `cluster_id` (an existing cluster for mode a, or empty to create one); `cluster_type` (`basic` default, `standard`; `enterprise`, `dedicated`, `freight` only with `allow_premium_cluster_types = true`); `max_ecku` (default 1); `availability` (`SINGLE_ZONE` default; `MULTI_ZONE` for Standard needs `max_ecku >= 2`); `topics` (a map: name → partitions, `retention_ms`, `retention_bytes`, `cleanup_policy`); `consumer_group_prefix`; `transactional_id_prefix`; `create_api_keys` (default `false`, §7.2); `secret_prefix` (the BY1 naming); `confluent_budget_monthly_usd`; `allow_cross_region` (default `false`); `confluent_networking` (`public` default, `privatelink` with Enterprise or Dedicated, CC3).

**Resources:** `confluent_environment` (if created; Stream Governance Essentials is attached by default and free, with 100 schemas, [packages](https://docs.confluent.io/cloud/current/stream-governance/packages.html), 2026-09-25), `confluent_kafka_cluster` with `basic { max_ecku }` or `standard { max_ecku }` and `deletion_protection = true` unless `name` starts with `loams-ci-`, `confluent_kafka_topic` per entry plus `__loams_checkpoints` (compacted, one partition) when any sink uses `exactly_once`, `confluent_service_account` `loams-<name>-data` and `loams-<name>-metrics`, `confluent_kafka_acl` per §7.1, `confluent_role_binding` (MetricsViewer for the metrics account), and the empty secret containers in the cloud's secret store (`aws_secretsmanager_secret`, `google_secret_manager_secret`, `azurerm_key_vault` secrets created by the CLI).

**Outputs:** `bootstrap_endpoint`, `cluster_id`, `environment_id`, `schema_registry_endpoint`, `data_service_account_id`, the secret names (never values), `confluent_ceiling_monthly_usd` and `confluent_unbounded` (§8.1), and the rendered connector instance files for `loams-fabric connect`.

**Tests** (`tofu test` with mock providers, as BY1's modules): refuses `enterprise`/`dedicated`/`freight` without the flag; refuses a region other than the VM's without the flag; every topic has `retention.bytes` set; no ACL grants CREATE or DELETE on topics, ALTER or ALTER_CONFIGS, or any ACL operation to the data account; no `confluent_api_key` resource unless `create_api_keys`; the example root module with `create_api_keys = true` has an `encryption` block with `enforced = true` (a static check over the example).

**Who runs it:** the user, with their own Confluent Cloud API key (environment variables `CONFLUENT_CLOUD_API_KEY` and `CONFLUENT_CLOUD_API_SECRET`, which the provider reads and does not store in state) and their cloud CLI's credentials, through `loams byoc deploy --cloud aws --confluent` (BY1's `loams byoc`, D1361) or plain `tofu apply`. The provisioning key never reaches the VM.

## 6. Region, networking and latency

### 6.1 Co-location (D1824)

- **Region codes are the clouds' own.** Confluent lists AWS (`us-east-1`, `ap-south-1`, …), Azure (`eastus`, `centralindia`, …) and Google Cloud (`us-central1`, `asia-south1`, …) under the providers' names; Basic and Standard are offered in every listed region; Enterprise is missing in some (for example AWS `ca-west-1`, Azure `mexicocentral`, Google Cloud `europe-west3`); Freight is AWS-only in eleven regions; GovCloud and Jio are Dedicated only ([regions](https://docs.confluent.io/cloud/current/clusters/regions.html), 2026-10-23).
- **So the map is the identity,** checked against `deploy/byoc/confluent/regions.toml` (cloud, region, types offered), generated from Confluent's region listing (the endpoint the `confluent kafka region list` command uses, verify in CC0) and refreshed by a weekly job that opens a PR when it changes. The BY1 defaults (`us-east-1`, `us-central1`, `eastus`) are all listed.
- **Cross-region or cross-cloud** is refused unless `allow_cross_region = true`, because it adds the cloud's internet egress price to every byte (for example AWS's data transfer out to the internet, verify the current rate) and tens of milliseconds to every round trip (estimate). Mode (a) with a user's existing cluster in another region gets the same warning from `loams byoc confluent check`.

### 6.2 Networking (D1825)

| Cluster type | Networking | Loams' default | Cost note |
|---|---|---|---|
| Basic, Standard | Public endpoints only | TLS (public CA, full hostname verification) and SASL/PLAIN with a scoped API key, outbound from the VM on 9092 and 443 | The VM already has outbound access (D1359). Traffic to a public endpoint may be charged by the cloud even in one region (verify per cloud: AWS public-IP traffic within a region, a NAT gateway's per-GB charge for a private subnet) |
| Enterprise | Private networking; PrivateLink, Private Service Connect and Private Network Interface; on AWS with PrivateLink capped at 10 eCKU ([cluster types](https://docs.confluent.io/cloud/current/clusters/cluster-types.html), 2026-10-20) | Opt-in in CC3: an interface endpoint in the VM's VPC and private DNS | $1.75–$2.25 per eCKU-hour, about $1,280–$1,640 a month at one eCKU around the clock, plus the cloud's endpoint charges ([pricing](https://www.confluent.io/confluent-cloud/pricing/)) |
| Dedicated | VPC peering, Transit Gateway, PrivateLink | Not offered by the module (bring an existing cluster, mode a) | "Contact us" pricing |

- **Nothing connects into Loams.** No security-group rule, firewall rule or Tunnel route is added for Confluent. A consequence, stated in the guide: Confluent's fully managed source connectors (Debezium on Confluent) cannot read a Loams Postgres. CDC out of Loams uses Loams' own Postgres CDC source and the Kafka sink (outbound), §4.2.
- **IP filtering** on the Confluent side (`confluent_ip_filter`) would let a user allow only the VM's egress address; which types support it is (verify) and it is an optional input when supported.

### 6.3 Latency

Not measured. Same-region produce with `acks=all` is expected in single-digit milliseconds and cross-region in tens to hundreds (estimate). CC1's nightly records p50 and p99 produce and end-to-end latency from the AWS VM to a `us-east-1` Basic cluster, and CC0 records one run from the same harness against the `ap-south-1` oracle, so the guide can quote measured numbers.

## 7. Security

### 7.1 Identities and ACLs (D1826)

Basic clusters "do not support RBAC roles for resources within the Kafka cluster" ([cluster types](https://docs.confluent.io/cloud/current/clusters/cluster-types.md), read 2026-10-25), so in-cluster access is ACLs, as the oracle does today.

| Principal | Where its key lives | Grants |
|---|---|---|
| `loams-<install>-data` | The cloud secret store → the Fabric's file secret store on the VM | Topic READ, WRITE, DESCRIBE, DESCRIBE_CONFIGS on the module's topics (LITERAL) or the user's listed topics; group READ, DESCRIBE on `consumer_group_prefix` (PREFIXED, default `loams-<ns>-`); transactional-id WRITE, DESCRIBE on `transactional_id_prefix` (PREFIXED); cluster IDEMPOTENT_WRITE. **Never** topic CREATE, DELETE, ALTER or ALTER_CONFIGS, and no ACL operation, so Loams cannot add partitions, raise retention or create topics |
| `loams-<install>-metrics` | The cloud secret store → the guard | MetricsViewer (read-only; organisation scope, so it can read every cluster's metrics, which the guide says) |
| `loams-<install>-ops` (optional) | The cloud secret store, only if `guard.cluster_throttle = true` | CloudClusterAdmin on this one cluster, used only to lower `max_ecku`. Off by default (Q1338) |
| A billing reader (optional) | **Not on the VM by default:** the user runs `loams confluent cost reconcile` from their machine or CI; on the VM only with `--billing-key-on-host` | The Costs API needs OrganizationAdmin or BillingAdmin ([invoices and costs](https://docs.confluent.io/cloud/current/billing/invoices-and-costs.md), read 2026-10-25), a broad role, which is why it stays off the host |
| The user's provisioning key | The user's shell only | Whatever the user has; used by `tofu` and `loams byoc confluent key create` |

### 7.2 Secrets (D1827)

`confluent_api_key` keeps the secret in state as a sensitive attribute, and the provider has no ephemeral or write-only form. OpenTofu has had ephemeral resources and write-only attributes since 1.11 ([1.11.0](https://opentofu.org/blog/opentofu-1-11-0), December 2025), but they need provider support that this provider does not have. Options:

| Option | State | Verdict |
|---|---|---|
| 1. The module creates the key; state holds it in plain text | Plain | **Rejected** (the owner's rule: no keys in state in plain text) |
| 2. The module creates the key; OpenTofu state and plan encryption `enforced = true`, key provider `aws_kms`, `gcp_kms` or `azure_vault` on the install's own KMS key ([state encryption](https://opentofu.org/docs/language/state/encryption/), read 2026-10-25) | Encrypted with the user's key | Allowed (`create_api_keys = true`), for users who want one apply |
| 3. **The CLI creates the key out of band** with the user's Confluent credentials (`POST /iam/v2/api-keys`) and writes it straight into the cloud secret store; state holds only the secret's name | No key | **Default** |
| 4. No key: OAuth with the VM's cloud identity through a Confluent identity provider and pool (AWS IAM, Entra ID / managed identity and generic OIDC are listed, [identity providers](https://docs.confluent.io/cloud/current/security/authenticate/workload-identities/identity-providers/oauth/identity-providers.html), 2026-09-25) | No key | **Target** (CC3): needs OAUTHBEARER in `loams-connector-kafka` (an rdkafka token-refresh callback fed from the VM's identity token, no `curl` feature) and per-type support (verify) |

The rest of the path:
- **Fetch at boot:** `loams-byoc-secrets` (BY1) gains the optional names `<prefix>/confluent-<integration>-api-key`, `-api-secret`, `-sr-key`, `-sr-secret`, `-metrics-key`, `-metrics-secret`, and writes them as files under the Fabric's secret root (`secrets/connect/<namespace>/confluent-<integration>/{username,password,...}`, mode 0600, `umask 077`), never into `.env`. The connector instances name them (`sasl.password: confluent-<integration>/password`).
- **Never in an image or user data:** cloud-init carries secret names only (BY1's rule); `tests/byoc/static` gains a check that rendered user data and `.env` contain no `CONFLUENT_` value.
- **Rotation:** `loams byoc confluent key rotate --integration <i>` creates a new key, writes a new secret version, asks the connect role to restart the integration's instances, waits for them to run, then deletes the old key after a grace period (default 1 hour).
- **Redaction:** the SDK's `Scrubber` already removes resolved secrets from positions, errors, DLQ records and status; the support bundle (D1376) lists integration names, never values.

## 8. Budget limits

### 8.1 What can be hard-capped (D1828)

| Lever | Enforced by | Hard? | What it bounds |
|---|---|---|---|
| **Cluster type** | The module | Hard | Enterprise ($1.75–$2.25 per eCKU-hour), Freight ($2.25, minimum 2 eCKU) and Dedicated need an explicit flag |
| **`max_ecku`** | Confluent ("You can set a lower maximum eCKU to cap cost", which "can cause throttling", [cluster types](https://docs.confluent.io/cloud/current/clusters/cluster-types.html), 2026-10-20) | Hard | eCKU charges at most `max_ecku × price × 730` a month. **Basic at `max_ecku = 1` costs $0 in eCKUs** (the first eCKU is free); Standard at 1 is at most $547.50 |
| **Per-eCKU capacity** | Confluent | Hard, but large | Basic 5 MBps in, 15 MBps out per eCKU; Standard 25 in, 75 out (same page). At Basic `max_ecku = 1` and $0.05/GB that bounds throughput charges at about $657 (in) and $1,971 (out) a month: a bound, not a budget |
| **Topic count, partitions** | The module; the data account has no CREATE | Hard for Loams | Basic allows 30 partitions per eCKU and 1,500 per cluster |
| **`retention.bytes` per partition** | The module; the data account has no ALTER_CONFIGS | Hard for Loams-managed topics | Storage at most `Σ partitions × retention.bytes × $0.08` per GB-month (pre- or post-replication billing, verify) |
| **Loams' connector byte rate** | The connect role (`RateLimit.bytes`, per instance) | Hard for Loams' traffic | Loams' share of throughput charges |
| **Client quotas** | Confluent, "Enterprise, Freight, and Dedicated clusters" only ([client quotas](https://docs.confluent.io/cloud/current/clusters/client-quotas.html), 2026-09-25) | Hard on those types | Per-principal ingress and egress; not available on Basic or Standard |
| **A monthly spend cap** | Nobody | **Not available** | Confluent has no budget API; the guard below is best-effort |

`confluent_ceiling_monthly_usd` sums what is bounded (eCKUs, Loams-managed storage, Loams' connector rates × $/GB) and `confluent_unbounded` lists what is not (other clients' throughput on a shared cluster, Schema Registry beyond 100 schemas, anything created outside the module). `loams byoc deploy` prints both before it applies (D1381's estimate, extended).

### 8.2 The budget guard (D1829)

```
hourly:  Metrics API ──▶ estimate (price table) ──▶ month to date + forecast ──▶ ladder
daily:   Costs API (optional reader) ──▶ reconcile the estimate, record drift
ladder:  50% alert · 80% alert · 90% throttle · 100% pause (opt-in) · month rollover resumes
```

- **Signals.**
  - The Metrics API (MetricsViewer; [Metrics API](https://docs.confluent.io/cloud/current/monitoring/metrics-api.html), 2026-09-25) has `elastic_cku_count`, `received_bytes`, `sent_bytes`, `retained_bytes` and `partition_count` per cluster (the public descriptor list at `api.telemetry.confluent.cloud/v2/metrics/cloud/descriptors/metrics`, read 2026-10-25). The guard multiplies them by `deploy/byoc/confluent/prices.toml` (per cloud, region and type, with its date and source URL). On a cluster shared with other apps (mode a), it estimates Loams' share from Loams' own principal where the metrics break down by principal (verify) and otherwise from the connect role's byte counters.
  - The Costs API (`GET /billing/v1/costs`) returns daily lines with `original_amount`, `discount_amount` and `amount` per resource, a window of at most one month, up to a year back, and "Cost data can take up to 72 hours to become available" ([invoices and costs](https://docs.confluent.io/cloud/current/billing/invoices-and-costs.md), read 2026-10-25). It reconciles; it never drives the ladder alone, because it is days late. The budget compares against `original_amount` by default, so credits or discounts do not hide usage (`--count-discounts` changes that).
- **Ladder** per integration, on the month-to-date estimate (or the forecast, if `guard.on = forecast`):
  - **50% and 80%:** an alert: an OTLP log event `loams.confluent.budget` (D1375's collector), a console notice, and an optional webhook (Slack, Teams or generic).
  - **90%: throttle.** The guard sets every Loams instance on that cluster to `guard.floor_bytes_per_sec` (default 64 KiB/s) through the connect role's admin API. With `guard.cluster_throttle = true` and the ops key, it also lowers `max_ecku` to the type's minimum (Basic and Standard: 1), which slows every client of the cluster; it is off by default.
  - **100%: pause** (`guard.pause_at_limit = true`, off by default, Q1337). Source instances stop consuming: their data waits in Confluent within the topic's retention, so the guard refuses to pause a source whose topic retention is shorter than the time left in the month and alerts instead. Sink instances stop producing: their data waits in the Loams stream, which is durable in the bucket.
  - **Resume:** at 00:00 UTC on the first of the month, or `loams confluent budget resume --integration <i>`, which records who resumed.
  - **Never:** delete a cluster, topic, key or ACL; change retention; touch clients other than Loams' own except through the opt-in `max_ecku` throttle.
- **Where it runs:** inside `loams-fabric connect` as a periodic task (it owns the instances it throttles), with its state in the bucket (`_meta/connect/guard/<integration>.json`, create-only versions), and as `loams confluent guard run --once` for debugging and for hosts with no connect role (CC3's Kubernetes tier gets a CronJob).
- **Honest labels:** every number carries `estimate (prices of <date>)` or `reconciled (Costs API, through <date>)`. The guide says plainly that the guard is best-effort, acts on data up to an hour old, and cannot stop spend by other clients of a shared cluster.
- **Confluent's own notifications:** the guide tells users to keep "Free usage is 80% consumed" (required, relevant to promotional credits) and to turn on the service-quota notices ([notifications](https://docs.confluent.io/cloud/current/monitoring/configure-notifications.html), 2026-09-25). They are not budgets.

### 8.3 Showing spend (D1830)

- `loams confluent cost [--integration i] [--month YYYY-MM]`: month to date, forecast, budget, ceiling, unbounded items, last reconciliation and its drift.
- `loams byoc status` (D1361) gains one line per integration: `confluent lkc-… basic us-east-1  $3.10 of $25 (estimate 2026-10-25T10:00Z)`.
- `GET /v1/integrations/confluent/spend` on the connect role's admin API (loopback and tailnet), the same JSON the console reads.
- The console: a card under Settings → Integrations (the CF1 shell), with the ladder's state and the resume button. Until CF1's shell lands, the CLI and API are the surface.

## 9. Ostrium's Confluent credits (D1831)

### 9.1 Facts and limits

- **Amount:** **$2,400** for one year from 2026-10-25 (the owner, Q1333, answered 2026-10-26; the oracle notes of 2026-10-25 said $2,000). The plan is made against $2,400: a monthly allowance of $200, of which the plan below spends about $57 with a ceiling of $100.
- **They are Ostrium's, not users'.** Promotional credits apply to the organisation that holds them. A user's install runs against the user's own Confluent organisation and bill; nothing in Loams can or will route a user's usage to Ostrium's organisation (it would also break D1378: Ostrium hosts no customer data plane). The guide says so.
- **When they run out or expire:** Confluent requires a payment method "to continue using Confluent Cloud after your free promotion" ([billing](https://docs.confluent.io/cloud/current/billing/overview.html), 2026-09-25); what happens to running clusters without one is (unverified). Ostrium's guard (§9.3) pauses CI before that point.

### 9.2 What they pay for, per month (estimate, list prices)

| Use | Shape | Per month |
|---|---|---|
| **Kafka gateway oracle** (existing) | Persistent Basic `lkc-120zwvj`, `ap-south-1`, `max_ecku` 1; report-only diff runs from the owner's machine | under $1 (eCKU free; MB of traffic and storage) |
| **Nightly BYOC + Confluent round trip** (CC1 Task 10) | An ephemeral Basic cluster in `us-east-1` beside the BY1 AWS nightly VM, ≤ 40 minutes, ≤ 200 MB in and ≤ 400 MB out per run | about $1 (30 × ~$0.03) |
| **Weekly Standard row** (CC2) | One Standard cluster at `max_ecku` 1 for ≤ 1 hour: `MULTI_ZONE` off, same round trip, plus a transactions check | about $4 (4.3 × $0.75 + traffic) |
| **Monthly Enterprise + PrivateLink row** (CC3, Q1339) | One Enterprise cluster at 1 eCKU for ≤ 2 hours with an AWS interface endpoint (paid from the AWS test account) | about $5 |
| **Design-partner demos** | A Standard cluster in the partner's region for a working day, torn down after | up to $25 (about 4 demos at ~$6) |
| **Quarterly benchmark** | Standard at `max_ecku` 4 for ≤ 4 hours, ~200 GB through the connectors (results published with the method) | about $11 averaged ($32 a quarter) |
| **Contingency** | Spikes, reruns | $10 |
| **Total** | | **about $57; ceiling $100** |

That is about $700 a year, under a third of $2,400, and leaves room to raise the demo line if design partners want more. The ceiling of $100 a month is enforced by §9.3, not by hope.

### 9.3 Discipline

- **Three environments:** `default` (the oracle, existing), `loams-ci` (nightly and weekly rows; nothing persistent except service accounts) and `loams-demo` (one cluster per demo, named `loams-demo-<partner>-<date>`).
- **Names and TTL:** every CI resource is `loams-ci-<run-id>-*`; clusters carry no tags, so the reaper reads `metadata.created_at` and the name. TTL 3 hours by default.
- **Teardown:** `tofu destroy` in an `always()` step, then a check that `GET /cmk/v2/clusters?environment=<loams-ci>` lists none of the run's clusters; an hourly reaper workflow deletes `loams-ci` clusters and API keys past their TTL and reports (never deletes) anything else in the environment older than a day. Demo clusters get a 24-hour TTL the reaper reports, and the person who made one deletes it.
- **Concurrency 1** per row (`concurrency: byoc-confluent`), as §70 §12.2.
- **Ostrium's own guard:** the same guard code, pointed at Ostrium's organisation with a $60 monthly alert threshold, $80 for a second alert and $100 for a **CI stop** (a repository variable `CONFLUENT_CI_ENABLED=false` that every Confluent workflow checks first). The Costs API reader for it runs on the local CI host (laptop A, `deploy/ci-hosts`), where a BillingAdmin key can sit in a 0600 file outside any repository; GitHub-hosted runners never hold it (D1832).
- **Confluent's notifications** to the owner's address: "Free usage is 80% consumed" stays on.

## 10. The HashiCorp free tier (D1833)

### 10.1 What is free today

| Product | Free? | Facts | Source |
|---|---|---|---|
| **HCP Terraform** | **Yes: the Free plan** | "Free organizations are limited to 500 managed resources"; one concurrent remote run and one agent run; unlimited users; the private registry included; the legacy free plan ended 2026-03-31 | [HCP Terraform overview](https://developer.hashicorp.com/terraform/cloud-docs/overview), read 2026-10-25; the end date from the 2026-10-23 research ([`docs/research/tidb-x-and-hcp.md`](../research/tidb-x-and-hcp.md) §3.1) |
| HCP trial credit | One-time | $500 for new organisations ("a $500 credit to get you started"); services stop when it runs out without a card (research §3.1) | [HashiCorp pricing](https://www.hashicorp.com/en/pricing), read 2026-10-25 |
| HCP Vault Secrets | **Retired** | End of sale 2025-06-30; end of life "earlier of the Flex contract expiry or July 1, 2026" | [IBM notice](https://www.ibm.com/support/pages/node/7263986), read 2026-10-25 |
| HCP Vault Dedicated | No free tier | Development edition is billed and capped at 25 clients a month | [pricing definitions](https://developer.hashicorp.com/hcp/docs/hcp/admin/billing/pricing-definitions), read 2026-10-25 |
| HCP Packer | No free allowance | Image buckets billed hourly, get-channel requests billed; registries are deactivated "with invalid payment methods or zero trial credits" | [pricing definitions](https://developer.hashicorp.com/hcp/docs/hcp/admin/billing/pricing-definitions); [registry](https://developer.hashicorp.com/hcp/docs/packer/manage-registry), read 2026-10-25 |
| HCP Boundary | No free allowance | Billed per user "once for each month in which the User takes an authenticated action" | [pricing definitions](https://developer.hashicorp.com/hcp/docs/hcp/admin/billing/pricing-definitions), read 2026-10-25 |
| HCP Consul Dedicated, Vagrant | Retired / closing | Consul Dedicated retired 2025-11-12; HCP Vagrant ends 2026-12-31 (research §3.1) | Research, read 2026-10-23 |

### 10.2 Licence position

- Terraform (≥ 1.6), Vault, Boundary, Packer, Consul and Nomad are BSL 1.1, with IBM as licensor (research §3.2). Loams ships none of them, copies none of their code, and runs no Terraform CLI in CI (D1150). The modules are OpenTofu (MPL-2.0) code.
- Providers are not the products: `confluentinc/confluent` is Apache-2.0, and `hashicorp/aws`, `google`, `azurerm` and `random` are MPL-2.0 dependencies fetched from the OpenTofu registry, as BY1 already does.
- Using an HCP SaaS as an operator, for Ostrium's own test infrastructure, is a service relationship, not a licence question; the HashiCorp AUP forbids bypassing limits and bulk accounts (research §3.3), which the use below does not do.

### 10.3 Is HCP Terraform Free worth using?

OpenTofu can use HCP Terraform through the `cloud` block or the `remote` backend for state with local execution; OpenTofu's docs recommend the `cloud` block, warn that "Features implementation might vary between different TACOS", and describe a remote-version check that `-ignore-remote-version` overrides ([remote backend](https://opentofu.org/docs/language/settings/backends/remote/), read 2026-10-25). HCP's remote runs execute Terraform, not OpenTofu (research §3.3), so runs stay in our CI either way.

| | R2 (S3 backend) | HCP Terraform Free (state only) |
|---|---|---|
| Already in use | Yes (D1729's default; R2 is Ostrium's object store) | No |
| Locking | S3 backend lockfile on conditional writes (verify on R2 with the pinned OpenTofu) | Built in |
| State history and UI | Bucket versioning; no UI | A UI, but it cannot show state that OpenTofu encrypts client-side, which we require for anything holding a key |
| Credentials in CI | An R2 token scoped to one bucket | An HCP team token; GitHub OIDC into HCP is for HCP's own runs, not for CLI clients (verify) |
| Risk | None new | IBM may change the free plan (it ended the legacy one); OpenTofu compatibility is best-effort on HashiCorp's side |
| Fits 500 resources | — | Easily: the persistent Confluent test root is about 20 resources |

**Recommendation:** keep R2 as the default for every Ostrium root, including the Confluent test roots; it is one backend, one credential type, and works with encryption. HCP Terraform Free adds little beyond a UI that encrypted state makes empty. If the owner wants to use it anyway (the owner said "we can use it"), use it **only** as a state-only backend for the one persistent root `loams-ci-confluent-base` (environments, service accounts, role bindings), with local execution, OpenTofu state encryption on, and a documented `tofu init -migrate-state` back to R2 (Q1336). Per-run nightly roots use R2 or local state destroyed in the same job. Never required for users; the modules configure no backend (BY1 ruling).

## 11. Testing and conformance

- **PR CI (no Confluent account):** the profile's config tests; the Debezium decoder on fixtures (Debezium's documented example envelopes, Apache-2.0, recorded with attribution); the connect role against an in-process engine and `apache/kafka:4.3.1` (SASL_SSL with a test CA, so the TLS + SASL path is covered); `tofu test` on the module with mock providers; the guard on recorded Metrics and Costs API fixtures (ids redacted) and a fake connect admin API; the static secret checks.
- **Nightly (AWS + Confluent, CC1 Task 10):** the BY1 AWS VM row plus the `confluent` module in `us-east-1`; then a Postgres → Loams stream → Kafka sink (transactional) → Confluent → Kafka source → collection round trip with counts, order per key and no duplicates; a Debezium-envelope topic fed by a test producer into a table; a kill-restart of the connect role mid-batch; a rotation; the guard's ladder against a $0.10 test budget (alert, throttle and pause observed, resume checked); the exposure scan (no new inbound port); latency numbers; teardown.
- **Weekly:** the same on Standard. **Monthly:** Enterprise with PrivateLink (CC3).
- **The oracle:** the Kafka gateway's report-only Confluent diff (fork) is unchanged.

## 12. Docs and site (D1834)

- **`docs/guides/byoc/confluent.md`:** the three modes, the decision table of §4.3, the module, secrets, the budget table of §8.1 with its hard/best-effort column, the guard, the cost table, what is not supported (inbound managed connectors), troubleshooting.
- **`docs/guides/connectors/confluent-cloud.md`:** the profile, Schema Registry, Debezium decoding, ACLs; and a row in `docs/guides/connectors/kafka.md`'s "Tested against" table once the nightly is green.
- **`docs/guides/kafka.md`:** a "Kafka endpoint: Iggy or Confluent" section.
- **loams.dev (loams-site):** a "With Confluent Cloud" section on the AWS, Google Cloud and Azure deploy pages; an optional **Confluent** add-on in `/cost`: cluster type (Basic, Standard), `max_ecku`, GB in and out, GB retained, from `lib/confluent-prices.ts` with the source URL and read date shown, labelled list prices, no credits. One site PR (CodeRabbit reviews site PRs).
- **`docs/design/networking-map.md`:** a row for outbound Kafka to Confluent.

## 13. Phases (D1835)

| Phase | Gate | Contents |
|---|---|---|
| **CC0** | This design approved | Rulings on Ostrium's credits (§16's verify items); latency from the oracle; no product code |
| **CC1** | CC0; the BY1 AWS owner checklist for the nightly | `loams-fabric connect` and the `connectors` profile; the `confluent-cloud` profile; Debezium decoding; the fetcher and key commands; the `confluent` module and the AWS example; the nightly; the two guides |
| **CC2** | CC1 | The guard and its ladder; spend surfaces; the weekly Standard row; Ostrium's own guard and CI stop; the site |
| **CC3** | CC2; Google Cloud and Azure test accounts (BY1 owner actions) | OAUTHBEARER and workload identity; PrivateLink on Enterprise and its monthly row; `gcp-vm-confluent` and `azure-vm-confluent` examples and rows |

## 14. What not to build

- No Confluent marketplace or "Connect with Confluent" listing for now (the reasoning of D1580; revisit with the owner, Q1341).
- No Kafka Connect worker, no Confluent-licensed connector images, no ksqlDB or Flink jobs.
- No Confluent-managed connectors reading from Loams (inbound; D1359).
- No use of Ostrium's credits or organisation for any user's traffic.
- No automatic deletion of a user's clusters, topics, keys or ACLs, ever.
- No HCP product as a dependency of anything a user installs; no Terraform CLI; no BSL binary.

## 15. Relation to earlier decisions

| Decision | Relation |
|---|---|
| D1 (bucket as truth), §70 principle 2 | Kept: Confluent is an edge, never Loams' log (D1820) |
| D1150, D1362 (OpenTofu; modules on official providers) | Applied: the `confluent` module on Confluent's Apache-2.0 provider |
| D1356, D1358 (cloud identity; secrets fetched at boot) | Extended: Confluent keys in the cloud secret store, fetched into the Fabric's file store (D1827); OAuth as the no-key target |
| D1359, D1311 (never public) | Kept: outbound only; managed inbound connectors not supported (D1825) |
| D1361 (`loams byoc`) | Extended: `--confluent`, `loams byoc confluent key|check` |
| D1380 (test cost guards) | Applied to Confluent: TTL, reaper, concurrency 1, a CI stop (D1831) |
| D1381 (customer cost guards) | Extended: `confluent_ceiling_monthly_usd` and the guard, since Confluent has no budget API to call (D1828–D1830) |
| D1580 (no marketplaces) | The same rule for Confluent's programmes (D1835) |
| D1729, Q1273 (HCP optional, R2 default) | Applied, not amended: HCP Terraform Free allowed state-only for one root if the owner wants (D1833, Q1336) |
| D1796–D1798 (Iggy `kafka` profile) | Unchanged; Confluent is the documented alternative (§4.3) |
| CN2 Task 4 (`NativeRuntime`) | `loams-fabric connect` is the VM slice CN2 absorbs (D1821) |

## 16. Risks

| Risk | Mitigation |
|---|---|
| A user reads "budget limit" as a hard cap and gets a large bill from other clients of a shared cluster | §8.1's hard/best-effort table in the guide and in `loams byoc deploy`'s output; `confluent_unbounded` listed; Basic `max_ecku = 1` as the default |
| The guard's estimate drifts from the bill (price changes, regional prices) | Dated price table, weekly freshness job, daily reconciliation that records drift and alerts above 10% |
| Pausing a source loses data past retention | The guard refuses to pause such a source and alerts instead (§8.2) |
| Exactly-once does not work on Basic | CC0 checks on the oracle; the profile narrows `sink.transactional` if it fails |
| The connect role diverges from CN2's `NativeRuntime` | Same SDK `Pipeline`, `SecretStore` and manifests; CN2 Task 4 is told to absorb it; the role's own code is the scheduler and Fabric I/O only |
| Keys leak through state, logs or bundles | Out-of-band keys by default, enforced encryption on the module path, the Scrubber, static checks on rendered files |
| Credits run out early or are smaller than thought | §9.2 spends about $57 a month of a $200 allowance ($2,400, Q1333); Ostrium's guard stops CI at $100 a month |
| HCP's free plan changes | R2 is the default; HCP holds at most one migratable root |

## 17. Open questions (Q1333–Q1341), answered 2026-10-26

| # | Question | Default |
|---|---|---|
| Q1333 | The credit amount and expiry: $2,400 (owner) or $2,000 (oracle notes), from 2026-10-25 for one year? | Plan for $2,000; the owner checks Billing → Payment and promotions in the Confluent console. **Answered (owner, 2026-10-26): $2,400; plan against $2,400** (spend plan unchanged) |
| Q1334 | The nightly's region: `us-east-1` next to the BY1 AWS nightly, or `ap-south-1` next to the oracle and the owner? | `us-east-1` (the VM and the cluster must share a region; the AWS test account is there). **Answered (owner, 2026-10-26): the default** |
| Q1335 | The order of modes: a → c → b, with (b) only after the guard exists? | Yes; (b) lands in CC1 with `cluster_type = basic` and `max_ecku = 1` only, Standard after the guard (CC2). **Answered (owner, 2026-10-26): the default** |
| Q1336 | HCP Terraform Free for `loams-ci-confluent-base`'s state? | No: R2 for everything (D1729); yes only if the owner wants the HCP UI, state-only with encryption. **Answered (owner, 2026-10-26): the default** |
| Q1337 | Pause at 100% of budget: opt-in or default on for installer-provisioned clusters? | Opt-in everywhere. **Answered (owner, 2026-10-26): the default** |
| Q1338 | May the guard lower `max_ecku` (needs a cluster-admin key on the VM)? | Off by default; opt-in per integration. **Answered (owner, 2026-10-26): the default** |
| Q1339 | The monthly Enterprise + PrivateLink row (about $5 of credits plus AWS endpoint charges from the AWS test credits)? | Yes, in CC3, once the AWS account checklist is done. **Answered (owner, 2026-10-26): the default** |
| Q1340 | Design-partner demos on our credits: up to about $25 a month, one cluster per demo, deleted the same day? | Yes. **Answered (owner, 2026-10-26): the default** |
| Q1341 | Confluent's partner programmes (a "Connect with Confluent" listing)? | Not now (as D1580); revisit with a design partner. **Answered (owner, 2026-10-26): the default** |

## 18. Sources

Read on 2026-10-25 unless a date is given.

- Confluent: [pricing](https://www.confluent.io/confluent-cloud/pricing/); [cluster types](https://docs.confluent.io/cloud/current/clusters/cluster-types.html) (2026-10-20) and its [Markdown form](https://docs.confluent.io/cloud/current/clusters/cluster-types.md); [regions](https://docs.confluent.io/cloud/current/clusters/regions.html) (2026-10-23); [client quotas](https://docs.confluent.io/cloud/current/clusters/client-quotas.html) (2026-09-25); [billing overview](https://docs.confluent.io/cloud/current/billing/overview.html) (2026-09-25); [invoices and costs](https://docs.confluent.io/cloud/current/billing/invoices-and-costs.md); [Costs API reference](https://docs.confluent.io/cloud/current/ccloud/list-billing-v-1-costs/); [notifications](https://docs.confluent.io/cloud/current/monitoring/configure-notifications.html) (2026-09-25); [Metrics API](https://docs.confluent.io/cloud/current/monitoring/metrics-api.html) (2026-09-25) and the [metric descriptors](https://api.telemetry.confluent.cloud/v2/metrics/cloud/descriptors/metrics?resource_type=kafka); [Stream Governance packages](https://docs.confluent.io/cloud/current/stream-governance/packages.html) (2026-09-25); [OAuth identity providers](https://docs.confluent.io/cloud/current/security/authenticate/workload-identities/identity-providers/oauth/identity-providers.html) (2026-09-25); [Confluent for Startups](https://www.confluent.io/startups/).
- The provider: [repository and LICENSE](https://github.com/confluentinc/terraform-provider-confluent); [OpenTofu registry versions](https://registry.opentofu.org/v1/providers/confluentinc/confluent/versions); resource docs at tag `v2.89.0` (`docs/resources/confluent_kafka_cluster.md`, `confluent_api_key.md`, `confluent_kafka_client_quota.md`, `confluent_identity_pool.md`, `docs/index.md`).
- OpenTofu: [remote backend](https://opentofu.org/docs/language/settings/backends/remote/); [cloud block](https://opentofu.org/docs/language/settings/tf-cloud/); [state encryption](https://opentofu.org/docs/language/state/encryption/); [1.11.0 release](https://opentofu.org/blog/opentofu-1-11-0).
- HashiCorp: [HCP Terraform overview](https://developer.hashicorp.com/terraform/cloud-docs/overview); [pricing](https://www.hashicorp.com/en/pricing); [pricing definitions](https://developer.hashicorp.com/hcp/docs/hcp/admin/billing/pricing-definitions); [HCP Packer registry](https://developer.hashicorp.com/hcp/docs/packer/manage-registry); [Vault Secrets notice](https://www.ibm.com/support/pages/node/7263986); [BSL announcement](https://www.hashicorp.com/en/blog/hashicorp-adopts-business-source-license) (2023-08-10); the 2026-10-23 research [`docs/research/tidb-x-and-hcp.md`](../research/tidb-x-and-hcp.md).
- In this repository: `fabric/crates/loams-connector-kafka/`, `fabric/crates/loams-connector-sdk/src/{ratelimit,secrets}.rs`, `fabric/crates/loams-flow/src/{runtime,secrets}/`, `fabric/crates/loams-fabric/src/main.rs`, `proto/loams/stream/v1/stream.proto`, `connectors/profiles/azure-event-hubs.yaml`, `connectors/registry/kafka.yaml`, `deploy/tofu/`, `deploy/byoc/lib/loams-byoc-secrets`, `deploy/release/compose.yaml`, `docs/guides/kafka.md`, `docs/guides/connectors/kafka.md`.
