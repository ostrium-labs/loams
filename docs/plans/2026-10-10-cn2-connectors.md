# CN2 — Loams Flow Connectors in Production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact names, paths, event types or defaults, use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Planned** (2026-10-10). Track CN, design [§33](../design/33-connectors.md) (D352–D359, Q348–Q359), after [CN1](2026-10-01-cn1-starred-connectors.md). §33 §8 defines CN2 as "every P2 row of Appendix A through stock Camel components (`loams-connect`) or Iggy plugins, each with a manifest and contract tests". This plan adds the production work that §33 §9–§11 leave open: the fleet on Kubernetes, connector observability and the Q359 activity counters, egress and secret hardening, the Camel upkeep CN-R2 and CN-R5 call for, remote ingress for webhooks and OTLP once the auth plan (Q30) allows it, and the upstream contributions of Q357. §33 has no production design document of its own (unlike §46 for PG2), so the gaps this plan fills by ruling are listed as CN2-Q1 to CN2-Q15 under "Open questions" and need the owner's answer before the tasks that name them.
>
> **As built at `1dc6e8a3` (dev, 2026-10-10).** The plan is reconciled with this state. Task 0 re-checks it.
> - **CN1 Tasks 1–2 are merged.** `fabric/crates/loams-flow` has `manifest.rs`, `registry.rs` and `validate.rs` (`ConnectorSpec`, `RuntimeSpec { kind, reference, version }`, `Registry::{load, get, list, starred, by_category}`, `validate_manifest`, `semantic_rules`, `check_use`, `LicenceGate`). `fabric/proto/loams/flow/v1/{connector,instance}.proto` exist, and there is no `flow.proto` yet. `connectors/registry/` holds **204 manifests**: 21 ★, 3 unstarred P1 (Zulip, ItsPlane, Forgejo, CN1 Task 15), Grafeo (D634), the rest generated stubs. `catalog.csv` assigns the **73 P2 rows** like this: 63 `camel`, 9 `openapi` (Delta Lake, Kestra companion, Segment, SendGrid, PostHog, HubSpot, Shopify, GitLab, Prometheus) and 1 `native` (Grafeo). `scripts/connectors/{gen_registry.py,camel_catalog.py,kestra_catalog.py,matrix.py,appendix.py}` and the `fabric.yml` jobs `connectors-unit` and `connectors-drift` run.
> - **Not built:** CN1 Task 3 (instances, `FlowService`, runtime supervisors, `SecretStore`, `loams-flow-conformance`), Tasks 4–14 (the ★ connectors and the gate), Task 12 (`loams-connect`; `fabric.yml`'s `connect-routes` job is a switched-off placeholder) and the Rust half of Task 15. Only `connect/routes/templates/{zulip,itsplane,forgejo}-import.yaml.tmpl` exist.
> - **FL1 is Planned, not built.** There is no `loams-fabric` binary, no `ingest`, no `loams-fabric-envelope`, no `deploy/fabric/` and no Iggy or Fluss stack in the tree. The `fabric/` workspace holds `loams-flow`, `loams-flow-proto` and HS1's House crates (`loams-chdb*`, `loams-house*`).
> - **Grafeo moved.** D741 and §48 §4.4 moved `loams-graph` and `loams.graph.v1` into the engine workspace (`proto/loams/graph/v1/graph.proto`). `connectors/registry/grafeo.yaml` still describes the embedded, auth-`none` engine of D634 (b).
>
> CN2a (registry truth) can start now. CN2b onward waits for CN1 Tasks 3 and 12 and FL1's `ingest` (see Execution order).

**Goal:** every P2 connector of §33 Appendix A shipped and proven, and the whole connector set (★ and P2) runnable in production:
- every P2 row has an honest manifest: its runtime is the one Loams ships, its capabilities come from that runtime's catalog, its licence passes D359, and a contract suite proves it against a real service, a recorded fixture or a nightly real account;
- the runtimes that P2 needs and CN1 does not: generic Camel route templates, Iggy P2 sinks, Debezium Server for SQL Server, Oracle, MongoDB and MariaDB, and *profiles*, which let a ★ module serve a protocol-compatible system or a declarative HTTP API;
- the fleet on Kubernetes (`loams-connect`, Debezium Server, the Iggy connectors runtime, native tasks), with a Helm chart;
- connector metrics, alerts, the Q359 activity counters (with no billing), an egress guard, production secret stores and a threat model;
- the Camel upkeep of CN-R2 and CN-R5, remote ingress for webhooks and OTLP when Q30 lands, and the Q357 upstream plugins.

The exit is the "Exit criteria for production" checklist at the end of this plan, with the owning tasks.

**Architecture** (§33 §5, CN1's Architecture):
- **`loams-flow`** stays the one owner of the registry, instance validation, runtime supervision and native connectors. CN2 adds a **profile** layer (§ Shared contracts). A profile is data: a ★ module run against a compatible system (a *twin*: Redpanda on the Kafka module, Valkey on the Redis module, R2 on the S3 module), or a declarative HTTP API description run by the ★ HTTP and webhook connectors. A profile is not a new runtime kind.
- **`loams-connect`** (CN1 Task 12) is the unmodified Camel 4.22.x runtime in Main mode. CN2 adds two generic route templates (`source`: `<scheme>` → shaping → `iggy:`; `sink`: `iggy:` → shaping → `<scheme>`) and one *overlay* per component, holding its URI options and shaping. **No Loams-written Java** (owner, 2026-10-01), and no Groovy, JavaScript or bean classes inside the YAML either.
- **Debezium Server** stays one container per CDC instance (CN1 Ruling 6), now for SQL Server, Oracle, MongoDB and MariaDB too. The HTTP sink posts to `ingest`.
- **Iggy connectors runtime** gains the P2 sinks it already has upstream (MongoDB, Redshift, Delta, RabbitMQ, each verified in Task 0).
- **Per-direction runtimes.** A manifest may name one runtime per direction when the best component differs: a MongoDB batch source on Camel with the sink on Iggy's `mongodb_sink`, or a Neo4j sink on Camel with a native source.
- **Production fleet.** `loams-flow` renders Kubernetes objects for each runtime. Supervision stays single-writer per instance (one `flow` process) until FL3's task leases land, enforced by a fenced instance lease on `_fabric.flow_objects` (Task 21).

**Tech Stack:**
- Rust 1.97 (`fabric/` workspace `rust-version`), edition 2024, workspace lints, arrow 59 (CN1 Ruling 4).
- Existing `fabric/` dependencies: `jsonschema` 0.58, `serde_norway`, `connectrpc` 0.9 and `buffa` 0.9.2, `prost` 0.14.
- New Rust dependencies (Task 0 verifies each version, licence and build cost; each must be at least 14 days old):
  - `arrow-csv` and `arrow-json` 59, already in `fabric/Cargo.lock` transitively (Apache-2.0);
  - `prost-reflect` (MIT OR Apache-2.0, verify) for descriptor-driven Protobuf;
  - `kube` and `k8s-openapi` (Apache-2.0; no crate in either lockfile uses them yet, so Task 0 picks the newest release at least 14 days old);
  - `rmcp` (verify licence; only if CN2-Q9 keeps MCP in CN2).
- Runtimes, pinned by digest:
  - Apache Camel 4.22.x, the patch of CN1 Task 12 (Task 0 records Camel's LTS list);
  - Debezium Server 3.7.0.Final;
  - the Iggy connectors runtime at FL1's pinned `dina-kar/iggy` revision, with Iggy 0.9.0.
- Test services, pinned by digest, CI-only (Task 4's `kind = "ci-service"`):
  - Brokers: RabbitMQ (MPL-2.0), ActiveMQ Artemis (Apache-2.0), NATS (Apache-2.0), Pulsar (Apache-2.0), Mosquitto (EPL-2.0), Redpanda (BSL, D60 precedent).
  - AWS: floci 2.1.0 (D60) for SQS, SNS, EventBridge, DynamoDB, DynamoDB Streams, SES and Lambda (Task 0 records which floci serves).
  - GCP: the Pub/Sub emulator and fake-gcs-server (BSD-2-Clause).
  - Azure: Azurite (MIT); the Event Hubs and Service Bus emulators (Microsoft EULA, CN2-Q7).
  - Databases: SQL Server Developer (EULA, CN2-Q7), Oracle Database Free (CN2-Q6), MariaDB 11 LTS (GPL-2.0), CockroachDB (CN2-Q7), MongoDB 8 (SSPL, D60 precedent), Apache Cassandra 5, Valkey 8, Neo4j Community (GPL-3.0), Qdrant (Apache-2.0).
  - Other: Trino (Apache-2.0), OpenBao (MPL-2.0, in place of BSL Vault), Keycloak (Apache-2.0), stripe-mock (MIT), Splunk (EULA, CN2-Q7), an SFTP server (`atmoz/sftp`, MIT, verify).
- Tools: compose (docker or podman) for the `connectors` and `connectors-p2` profiles; kind for Kubernetes e2e (CI only); `camel` JBang for route validation (inside the Camel image, never installed on the host); `buf`.

**Spec:**
- [§33](../design/33-connectors.md), all of it, with D352–D359 and Q348–Q359 in the [decision log](../design/13-decision-log.md).
- [§32](../design/32-loams-flow-fabric-house.md): §5.4 (envelope, D334), §5.5 (adapters table, the CN2 row), §5.7 (bridges), §6 (Flow; FL3's routes and leases).
- [CN1](2026-10-01-cn1-starred-connectors.md), its rulings 1–9 and its execution rulings 1–13 (all carried in), and [`cn1-dependency-spike.md`](cn1-dependency-spike.md) §5 (Camel's module tree versus catalog JSON; `mcp-server` absent).
- [FL1](2026-10-01-fl1-fabric-foundation.md), as built when CN2b starts.
- [§48](../design/48-loams-graph-production.md) §4.4 and D741 (Grafeo is a `loams.graph.v1` client), D634 (GQL, no Bolt), [`graph-db-rust-spike.md`](graph-db-rust-spike.md) (Neo4j's `producerOnly`).
- [§27](../design/27-usage-hooks.md) §3.7 and D548–D552 (observers carry plain structs; no billing in this repository), D60 (CI-only services), D111 and Q30 (loopback until the auth plan), D189 (secrets).

## Global Constraints

- **Worktree and branches.** Work in `~/Documents/Ostriumlabs/loams-wt/cn2-connectors`. Use one branch per milestone: `feat/cn2a-registry`, `feat/cn2b-runtimes`, `feat/cn2c-p2-connectors` (split per family task if a PR passes about 1,500 lines) and `feat/cn2d-production`. Each is based on `dev`, with stacked PRs targeting `dev`. Use `git commit -s` (DCO). Commit areas: `flow`, `connectors`, `connect`, `deploy`, `ci`, `docs`.
- **Rust builds** use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR` and never build in `/tmp`. Run one cargo build at a time (jobs and linker from `~/.cargo/config.toml`). Build the touched crate only: `cargo test --manifest-path fabric/Cargo.toml -p loams-flow`, not `--workspace`. The `kafka` feature (librdkafka) stays off for local builds unless the task is Kafka's or a Kafka twin's. Compose stacks and kind run in CI, or locally only while no cargo build runs.
- **Buy first** (D354). Before writing anything native, the task checks the Camel component (`camel-catalog` 4.22.x JSON for direction, `apache/camel`'s module tree for existence, as `cn1-dependency-spike.md` §5 rules), the Iggy plugin list at the pinned revision, and Debezium Server's connectors. A native module, or a profile on one, is written only where this plan's Task 2 assignment table says so.
- **Java is deferred** (owner, 2026-10-01). No `.java`, `.kt`, `.groovy` or `pom.xml` anywhere. Camel YAML templates may use only the languages `simple`, `constant`, `header`, `jq`, `jsonpath` and `jsonata`, and no `beans:` entry with a `type:` or `#class:` reference. Task 5's `templates_use_only_allowed_languages` enforces this. A capability that needs Java is declared narrower, and the gap is recorded in the manifest's `notes`.
- **Capabilities come from the catalog.** A Camel-backed manifest may declare a source only if `camel-catalog` marks the component as having a consumer (not `producerOnly`), and a sink only if it has a producer (not `consumerOnly`). Task 3's `camel_direction_matches_catalog` runs on every PR from a vendored catalog snapshot, plus weekly against upstream.
- **Every manifest validates, and every declared capability has a test** (D353 rule 3). CN1 enforces this for ★ manifests; CN2 extends `every_declared_capability_has_a_test` to every manifest whose `status` is `preview` or `stable`. A P2 row is `preview` only when its suite is green, and it stays `planned` otherwise.
- **Licence gate** (D359). This adds two kinds to `connectors/licences.toml` (Task 4): `ci-service` (an unmodified service image run only in CI, never shipped or run by default, as D60 rules for ScyllaDB Alternator) and `user-supplied` (a driver the user mounts, as CN1 Ruling 8). Neither kind may appear in an image Loams builds or in the default compose profile.
- **No secret** in a manifest, an instance spec, a rendered Camel route, a Debezium properties file, an Iggy TOML, a Kubernetes object other than a `Secret`, a log line, a metric label or an error (CN1 Task 3's `no_secret_in_rendered_configs_or_logs`, extended in Task 20 to Camel's and Debezium's own logs).
- **Loopback only** (D111) for `FlowService` and every Loams-served connector endpoint until Task 23's gate (Q30) opens.
- **No billing** (D548–D552). Q359's counters are open observability, delivered to `ConnectorActivityObserver` as plain structs with no wire format, buffer or persistence. No field, metric or file name contains `price`, `invoice`, `credit`, `billable` or `meter`. `scripts/ci/no-metering.sh` stays green, and this plan does not touch its allowlist.
- **Pins.** Exact versions for every new crate and image, and images pinned by digest. A new dependency must be at least 14 days old. Record each pin in the task's commit message and in `cn2-dependency-spike.md`.
- **Tests skip without services**, as in CN1 (`LOAMS_CONNECTORS_STACK`; CN2 adds `LOAMS_CONNECTORS_P2_STACK` for the `connectors-p2` compose profile, and `LOAMS_<CONNECTOR>_*` credentials for nightly real-account jobs).

## Review Focus

1. **A manifest claims a capability its runtime cannot deliver.** A Camel `producerOnly` component declared as a source, an Iggy sink declared transactional, a twin claiming what the twin service lacks. Expected: refused by validation or caught by the suite. Tests: Task 3 `camel_direction_matches_catalog`; Task 9 `every_declared_capability_has_a_test` (all `preview` manifests); Task 12 `twin_suite_runs_against_twin_service`.
2. **Loams-written Java enters the repository**, through a file, a Groovy or JavaScript expression, or a bean class in a YAML route. Expected: CI fails. Tests: Task 5 `templates_use_only_allowed_languages`, `no_java_sources_in_tree`.
3. **A secret leaks** through a rendered route, a Debezium properties file, an Iggy TOML, a Kubernetes object, Camel's or Debezium's own logs, or an error. Expected: never. Tests: Task 20 `no_secret_in_camel_or_debezium_logs`, `no_secret_in_rendered_kube_objects`; Task 6 `profile_secrets_by_reference_only`.
4. **An instance reaches somewhere it must not (SSRF).** Examples: the cloud metadata endpoint, loopback, the Kubernetes API, or another tenant's Service, through an HTTP profile, a Camel URI or a Debezium host. Expected: refused at validation and at connect time. Tests: Task 20 `egress_guard_refuses_metadata_loopback_and_cluster`, `egress_guard_rechecks_after_dns`.
5. **Loss or unbounded duplication when a Camel route or a Debezium Server restarts.** Expected: at-least-once, with duplicates only within the declared delivery. Tests: Task 5 `camel_source_kill_restart_no_loss`, `camel_sink_commits_after_ack`; Task 8 `debezium_sqlserver_restart_resumes`.
6. **CloudEvents already produced upstream lose their identity.** Examples: Event Grid, Pub/Sub with `ce-` attributes, Kafka-compatible brokers with `ce_` headers. Expected: pass-through keeps `type`, `source` and `id` (§33 §6). Tests: Task 11 `eventgrid_cloudevents_pass_through`, `pubsub_ce_attributes_pass_through`; Task 12 `redpanda_ce_headers_pass_through`.
7. **A CI-only or user-supplied component ships.** Expected: the image and default compose carry none. Test: Task 4 `no_ci_only_or_user_supplied_in_images`.
8. **Billing creeps in** through the activity observer. Expected: plain structs only, and the marker guard stays green. Tests: Task 19 `activity_observer_has_no_wire_format`, `no_metering_guard_green`.
9. **Two supervisors run the same instance** after a `flow` restart or a Kubernetes reschedule. Expected: the older holder is fenced. Test: Task 21 `two_flow_processes_one_instance_runner`.

---

## File structure

```
connectors/schema/connector.schema.json                    Task 1 (runtime per direction, profile, sink.reply, notes)
fabric/proto/loams/flow/v1/connector.proto                  Task 1 (mirrors the schema)
connectors/registry/catalog.csv                             Task 2 (runtime assignment; Debezium row split)
connectors/registry/<id>.yaml                               Tasks 2, 3 (P2 manifests), 10–18 (status preview)
connectors/registry/handwritten.txt                         Tasks 2, 3
connectors/schemas/<id>.config.json                         Task 3 (generated from Camel options), 10–18
connectors/profiles/<id>.yaml                               Tasks 6, 12, 17, 18
connectors/licences.toml                                    Task 4 (ci-service, user-supplied)
connectors/vendor/camel-catalog-4.22.x.json.zst             Task 3 (snapshot, with its SHA-256 in README)
docs/design/33-connectors.md                                Task 2 (Appendix A cells), Task 25 (status)
scripts/connectors/{camel_manifest.py,no_java.py,profiles.py}   Tasks 3, 5, 6
fabric/crates/loams-flow/src/
  manifest.rs  validate.rs  registry.rs                     Task 1
  profile.rs                                                Task 6
  runtime/{camel.rs,iggy.rs,debezium.rs}                    Tasks 5, 7, 8 (extend CN1 Task 3's)
  runtime/kube/{mod.rs,connect.rs,debezium.rs,iggy.rs,native.rs,lease.rs}   Task 21
  formats/{csv.rs,ndjson.rs,protobuf.rs}                    Task 16
  connectors/{graph.rs,neo4j.rs,prometheus.rs,mcp.rs}       Tasks 15, 18
  egress.rs  activity.rs  metrics.rs                        Tasks 19, 20
  secrets/{file.rs,kube.rs}                                 Task 20
fabric/crates/loams-flow-conformance/src/{fixture.rs,services.rs}   Task 9
fabric/crates/loams-flow/tests/{p2_<family>.rs,profiles.rs,kube.rs,egress.rs,activity.rs}
connect/routes/templates/{source.yaml.tmpl,sink.yaml.tmpl}  Task 5
connect/routes/overlays/<id>.yaml                           Tasks 5, 10–18
deploy/fabric/compose.yaml (profile connectors-p2)          Task 9
deploy/fabric/connectors/debezium/{sqlserver,oracle,mongodb,mariadb}.properties.tmpl   Task 8
deploy/fabric/connectors/iggy/{mongodb_sink,redshift_sink,delta_sink,rabbitmq_sink}.toml.tmpl   Task 7
deploy/helm/loams-connectors/                               Task 21
deploy/observability/connectors/{dashboards/,alerts.yaml}   Task 19
conformance/connectors/fixtures/<id>/                       Task 9 (recorded HTTP fixtures)
docs/security/connectors-threat-model.md                    Task 20
docs/runbooks/connectors/                                   Task 25
docs/guides/connectors/<id>.md, index.md (generated)        Tasks 10–18, 25
docs/plans/cn2-dependency-spike.md  docs/plans/cn2-exit-report.md   Tasks 0, 25
.github/workflows/fabric.yml (connectors-p2, connect-routes, connectors-nightly, connectors-kind)
```

## Shared contracts (all tasks use these names)

### Manifest additions (Task 1 writes them; YAML is the source, and the proto mirrors it one to one)

```yaml
runtime:
  kind: camel                  # default for both directions (unchanged meaning)
  ref: mongodb
  version: "camel 4.22.x"
  source:                      # optional: overrides kind/ref for the source direction
    kind: camel
    ref: mongodb
  sink:                        # optional: overrides kind/ref for the sink direction
    kind: iggy
    ref: mongodb_sink
  profile: null                # optional: a profile id under connectors/profiles/ (twins and HTTP profiles)
capabilities:
  sink:
    reply: false               # new: the sink emits one reply event per input event to a declared reply topic
notes: []                      # new: free-form strings; capability narrowing reasons (e.g. "java-deferred: no typed DTOs")
```

Semantic rules added to `validate::semantic_rules`:
- R-CN2-1: a direction override names a direction the manifest declares.
- R-CN2-2: `profile` is set only when `runtime.kind` (or the direction's) is `native` or `iggy`, and the profile file exists and validates.
- R-CN2-3: `sink.reply` implies `envelope.emits` is non-empty and the config schema has `reply_topic`.
- R-CN2-4: a `camel` direction's `ref` exists in the vendored catalog snapshot, with the matching consumer or producer (Task 3).
- R-CN2-5: `status: preview` or `stable` implies a non-empty `conformance` that includes `contract`.

`proto_and_yaml_agree` covers the new fields. The bump is `specVersion` minor for every manifest that gains a field, and major for none (D353 rule 4: nothing narrows).

### Profiles (Task 6)

```yaml
# connectors/profiles/<id>.yaml
apiVersion: loams.flow/v1
kind: Profile
id: valkey
module: redis                  # the ★ native module (loams_flow::connectors::<module>) or "http"
defaults: {}                   # instance-config defaults merged under the user's config
narrows: [ ]                   # capability paths the twin does NOT have (e.g. "sink.transactional")
# HTTP profiles only:
http:
  base_url: "https://api.hubspot.com"
  auth: { scheme: oauth2 | bearer | basic | header, header: "Authorization" }
  resources:
    contacts:
      list: { method: GET, path: "/crm/v3/objects/contacts", items: "/results", cursor: { next: "/paging/next/after", param: "after" } }
      upsert: { method: POST, path: "/crm/v3/objects/contacts/batch/upsert", batch: 100 }
  rate_limit: { requests: 100, per_seconds: 10, honour_headers: ["Retry-After", "X-HubSpot-RateLimit-Remaining"] }
  webhook: { scheme: hmac-sha256, header: "X-HubSpot-Signature-v3" }   # handled by the ★ webhooks source
```

```rust
pub struct Profile { pub id: String, pub module: String, pub defaults: serde_json::Value,
                     pub narrows: Vec<String>, pub http: Option<HttpProfile> }
pub fn load_profiles(dir: &Path) -> Result<BTreeMap<String, Profile>, ProfileError>;
pub fn effective_capabilities(spec: &ConnectorSpec, p: Option<&Profile>) -> Capabilities; // spec minus `narrows`
```

### Camel route contract (Task 5)

- Generated route id: `loams-<namespace>-<instance>-<source|sink>`. File: `/etc/loams-connect/routes/<namespace>/<instance>.<direction>.yaml`.
- Source template: `from: <scheme>:<path>?<overlay options>` → `setHeader` steps that build `ce_specversion`, `ce_type` (`io.loams.dev.flow.<id>.<event>.v1`), `ce_source` (`/connectors/<instance>/<resource>`), `ce_id` (the overlay's `id_expression`, which must be stable across re-reads), `ce_subject`, `loamsconnector` and `loamsinstance` → `to: iggy:<stream>/<topic>`. Records that already carry CloudEvents attributes take the overlay's `passthrough` branch, which keeps `type`, `source` and `id`.
- Sink template: `from: iggy:<stream>/<topic>?consumerGroup=flow-<instance>` → shaping (`jq`) → `to: <scheme>:<path>?…`. The consumer offset commits only after the producer step returns (Task 0 verifies `camel-iggy`'s manual-commit option; if it has none, the sink declares `delivery.sink: at_most_once` and the gap is a CN2 ruling plus an upstream issue).
- Overlay (`connect/routes/overlays/<id>.yaml`): `scheme`, `source_options`, `sink_options`, `id_expression`, `subject_expression`, `event`, `shaping_jq`, `passthrough` (`ce-headers`, `ce-attributes` or `none`), and `secrets` (option names filled from mounted files through Camel property placeholders `{{file:/run/secrets/<name>}}`, verify the 4.22 syntax).

### Event names for P2 (extends CN1 Ruling 2; `io.loams.dev.flow.<connector-id>.<event>.v1`)

| Family | Events |
|---|---|
| Brokers (AMQP, JMS, NATS, Pulsar, MQTT, SQS, SNS, Pub/Sub, Event Hubs, Service Bus, WebSocket) | `message` |
| EventBridge, Event Grid | pass-through when the input is a CloudEvent, else `event` |
| Relational and warehouse batch reads (Camel `sql`, Trino, Databricks, Redshift) | `rows` (JSON rows from Camel; Arrow only on native or ADBC paths) |
| CDC (Debezium SQL Server, Oracle, MongoDB, MariaDB; DynamoDB Streams) | `change` (with `loamsop`, `loamslsn`) |
| Documents (MongoDB, Cassandra, DynamoDB, OpenSearch, Qdrant) | `document` |
| Graph (Neo4j, Grafeo) | `rows` (GQL result batches) |
| Objects (GCS, Azure Blob, ADLS, SFTP, MinIO/RustFS, R2) | `object`, `object-deleted` |
| SaaS and HTTP profiles | `item` (polled), `delivery` (webhooks) |
| AI (OpenAI, Anthropic, MCP) | `reply` |
| Observability (Splunk, Prometheus) | `log`, `metric` |

### Activity observer (Task 19; Q359)

```rust
pub trait ConnectorActivityObserver: Send + Sync + 'static {
    fn on_activity(&self, a: &ConnectorActivity);          // plain struct, no wire format (D548)
}
pub struct ConnectorActivity { pub namespace: u64, pub instance: String, pub connector: String,
    pub runtime: RuntimeKind, pub direction: Direction, pub events: u64, pub bytes_in: u64,
    pub bytes_out: u64, pub api_calls: u64, pub window_end: SystemTime }
```

The open implementation turns it into the `loams_flow_connector_*` metrics (Task 19). `loams-platform` links its own implementation. This repository persists nothing.

### Egress guard (Task 20)

```rust
pub struct EgressPolicy { pub allow_cidrs: Vec<IpNet>, pub deny_cidrs: Vec<IpNet>, pub allow_hosts: Vec<String> }
impl EgressPolicy {
    pub fn check_config(&self, spec: &ConnectorSpec, config: &serde_json::Value) -> Result<(), EgressError>; // hosts named in config
    pub fn check_addr(&self, addr: &SocketAddr) -> Result<(), EgressError>;                                  // after DNS, at connect
}
// Default deny: 127.0.0.0/8, ::1, 169.254.0.0/16, fe80::/10, fd00:ec2::254, the cluster's pod and service CIDRs,
// and the Loams control-plane Services, unless the namespace admin allowlists them.
```

---

## Execution order

1. Task 0.
2. **CN2a** (Tasks 1–4) at once: it needs only `loams-flow` as built and the registry scripts.
3. **CN2b** (Tasks 5–9) after CN1 Task 3 (instances, runtimes, `SecretStore`, the contract kit), CN1 Task 12 (`loams-connect`) and FL1's `ingest`. Task 7 also needs FL1's Iggy stack, and Task 8 needs CN1 Task 8 (Debezium for Postgres and MySQL).
4. **CN2c** (Tasks 10–18) after CN2b. The family tasks are independent of each other and may run in parallel worktrees, one cargo build at a time on the build machine. Task 12 (twins) also needs the ★ modules it reuses: CN1 Tasks 6, 7, 9, 10, 11 and 13.
5. **CN2d** (Tasks 19–25): Tasks 19–22 after CN2b, beside CN2c. Task 23 is gated on Q30's auth plan. Task 24 is gated on the owner's go-ahead for upstream PRs (§23 §2.2). Task 25 comes last.

---

### Task 0: Reconcile with the code as built, and the dependency spike

**Files:** `docs/plans/cn2-dependency-spike.md`; this plan's "Rulings made during execution".

Steps:
1. Answer each of the following and record the answer, with file paths or URLs, as a ruling:
   - Is the "As built" block above still true? Check `fabric/crates/loams-flow/src/`, `fabric/proto/loams/flow/v1/`, `connect/`, `deploy/fabric/` and FL1's and CN1's plan status lines. Name which CN1 tasks and FL1 tasks have merged since `1dc6e8a3`.
   - For each of the 63 Camel P2 refs: does the component exist in `apache/camel` at the CN1 Task 12 tag, what is its consumer and producer support in `camel-catalog`, and does it need generated Java classes (gRPC stubs, Protobuf messages, Salesforce DTOs) or a bean to work? Output: a table in the spike document, which Task 2 consumes.
   - `camel-iggy`: its URI options at the pinned version, whether the consumer commits offsets manually (after the route's last step) or automatically, and its Iggy client version against Iggy 0.9.0.
   - Iggy's connectors at FL1's pinned revision: is each of `mongodb_sink`, `redshift_sink`, `delta_sink` and a RabbitMQ sink present, and what are its config keys?
   - Debezium Server 3.7.0.Final: the SQL Server, Oracle, MongoDB and MariaDB connector classes, their required source settings (SQL Server CDC enablement, Oracle LogMiner and `ojdbc` placement, MongoDB replica set, MariaDB GTID), and the HTTP sink's CloudEvents properties as CN1 Task 8 recorded them.
   - floci 2.1.0's coverage of SQS, SNS, EventBridge, DynamoDB Streams, SES and Lambda invoke. Where it falls short, choose a recorded fixture.
   - The licences, and the CI terms, of every test image in the Tech Stack (CN2-Q6, CN2-Q7).
   - `kube`/`k8s-openapi`, `prost-reflect` and (if CN2-Q9 keeps it) `rmcp`: version, licence, release date, and the build cost with a single crate built.
   - Whether `loams-flow-proto` can generate a `loams.graph.v1` client from `proto/loams/graph/v1/graph.proto` (a cross-workspace `build.rs` include), or whether a copy plus a drift check is needed (Task 15).
2. Commit `docs(connectors): cn2 task 0 rulings and dependency spike`.

## CN2a — Registry truth (Tasks 1–4)

### Task 1: Per-direction runtimes, profiles, `sink.reply` and `notes`

**Files:** modify `connectors/schema/connector.schema.json`, `fabric/proto/loams/flow/v1/connector.proto`, and `fabric/crates/loams-flow/src/{manifest.rs,validate.rs,registry.rs}`. Tests go in `fabric/crates/loams-flow/tests/{validate.rs,registry.rs}`.

**Interfaces:**
- `RuntimeSpec` gains `source: Option<RuntimeOverride>`, `sink: Option<RuntimeOverride>` and `profile: Option<String>`, where `RuntimeOverride { kind, reference, version }`, plus `RuntimeSpec::for_direction(Direction) -> (RuntimeKind, &str)`.
- `SinkCaps.reply: bool` (serde default `false`) and `ConnectorSpec.notes: Vec<String>`.
- Rules R-CN2-1 to R-CN2-5 of the shared contract in `semantic_rules`. R-CN2-4 reads the snapshot that Task 3 vendors; until Task 3 lands, it is a no-op behind a `cfg` guard and a `TODO(CN2 Task 3)`.
- `Filter` gains `runtime_for_direction`, so that `ListConnectors` can answer "which connectors source through Camel".

Tests:
- `override_direction_must_be_declared`
- `profile_requires_native_or_iggy`
- `reply_requires_reply_topic`
- `preview_requires_contract_suite`
- `for_direction_falls_back_to_default`
- `proto_and_yaml_agree` (extended to the new fields)
- `existing_204_manifests_still_validate`: no regression on the tree as built.

Steps: tests (FAIL) → schema → proto → Rust → PASS → `gen_registry.py --check` still green → commit `flow: per-direction runtimes, profiles, sink replies and notes in manifests`.

### Task 2: The P2 runtime assignment

**Files:** `connectors/registry/catalog.csv` (adds the columns `source_runtime`, `sink_runtime`, `profile`; `gen_registry.py` reads them), `scripts/connectors/{gen_registry.py,matrix.py}`, `connectors/registry/*.yaml` (regenerated), `docs/design/33-connectors.md` (Appendix A cells and the totals line), and `fabric/crates/loams-flow/tests/registry.rs`.

**Semantics.** This table assigns runtimes, using Task 0's Camel table to settle the cells marked (verify). Rows not listed keep `camel` with their current `ref`.

| Rows | Assignment | Why |
|---|---|---|
| Redpanda | `native` `kafka` + profile `redpanda` | Kafka API twin; the ★ module exists (D354 "where Loams has the code") |
| Valkey, Redis Streams | `native` `redis` + profiles `valkey`, `redis-streams` | Twins of ★ Redis; Redis Streams is the ★ module's Streams source |
| MinIO / RustFS, Cloudflare R2 | `native` `s3` + profiles | `object_store` twins of ★ S3 |
| Aurora, CockroachDB, pgvector | `native` `postgresql` + profiles (Aurora also `mysql`, chosen by config `engine`) | Wire twins; pgvector maps `vector` columns |
| MariaDB | `native` `mysql` + profile `mariadb`; CDC through the new `debezium-mariadb` manifest | Wire twin |
| Azure SQL | `camel` `sql` (as SQL Server), profile-free; CDC through `debezium-sqlserver` | No native SQL Server module |
| OpenSearch | `iggy` `elasticsearch_sink` / `elasticsearch_source` + profile `opensearch` | CN1 Task 10 already tests Iggy's ES plugins against OpenSearch |
| DuckDB | `native` `adbc` + profile `duckdb` | CN1 Task 11's allowed DuckDB driver |
| MongoDB | source `camel` `mongodb`, sink `iggy` `mongodb_sink` (verify) | Rust sink with no JVM; Camel for batch reads |
| Redshift | source `camel` `aws2-redshift-data`, sink `iggy` `redshift_sink` (verify) | As MongoDB |
| RabbitMQ / AMQP | source `camel` `amqp`, sink `iggy` RabbitMQ sink (verify) or `camel` | As MongoDB |
| Delta Lake | sink `iggy` `delta_sink`; source not shipped in CN2 (CN2-Q12) | Iggy has the sink; the row stops being `openapi` |
| Debezium-SQL Server / Oracle | **split** into `debezium-sqlserver` and `debezium-oracle`, `runtime: debezium` | D357 runs external CDC on Debezium Server, not Camel's embedded engine (CN2-Q5) |
| Debezium-MongoDB | `runtime: debezium` (was `camel` `debezium-mongodb`) | D357 |
| CSV / NDJSON, Protobuf | `native` `formats::{csv,ndjson,protobuf}` | Formats are native codecs (CN1 Task 5); Camel's protobuf dataformat needs generated Java |
| gRPC | `planned`, `notes: ["java-deferred: camel-grpc needs generated stubs"]` unless CN2-Q4 picks a native dynamic client | Java deferral |
| Kestra companion, Segment, SendGrid, PostHog, HubSpot, Shopify, GitLab | `native` `http` + an HTTP profile each | No Camel or Iggy component; profiles are the CN2 bridge to CN3's generator (CN2-Q3) |
| Prometheus | `native` `prometheus` (remote-write receiver as a source; remote-write as a sink) | No Camel component; a small native module over `ingest`'s listener |
| Neo4j | sink `camel` `neo4j`; source `native` `neo4j` over Neo4j's HTTP Query API (CN2-Q8) | `camel-neo4j` is `producerOnly` (`graph-db-rust-spike.md`) |
| Grafeo | `native` `graph`, auth `key`, client of `loams.graph.v1` | §48 §4.4 and D741 supersede D634 (b) |
| MCP | `native` `mcp` if CN2-Q9 says so; else `priority: P3` | No `mcp-server` in Camel 4.22.1 (`cn1-dependency-spike.md` §5) |

Totals after the split: 205 manifests, 21 ★, and 74 P2 rows. Appendix A's "Debezium-SQL Server / Oracle" row becomes two rows, and Appendix A's runtime-bearing cells and the totals line are edited to match. `matrix.py --check` keeps the appendix and the CSV identical.

Tests:
- `registry_has_205_entries_and_21_starred` (renamed from CN1's 203/204 count)
- `p2_has_no_openapi_runtime`: no P2 row keeps `openapi` unless its `notes` explain why.
- `every_twin_names_an_existing_module_and_profile`
- `debezium_rows_run_on_debezium_server`
- `appendix_matches_csv`

Commit `connectors: assign the P2 runtimes, split the Debezium row, and mark the twins`.

### Task 3: P2 manifests from the Camel catalog

**Files:** `scripts/connectors/camel_manifest.py`, `connectors/vendor/camel-catalog-4.22.x.json.zst` and its README with the SHA-256, `connectors/registry/<id>.yaml` for every Camel-backed P2 row (moved into `handwritten.txt` once detailed), and `connectors/schemas/<id>.config.json`. Tests: `fabric/crates/loams-flow/tests/registry.rs` and `scripts/connectors/test_camel_manifest.py`.

**Semantics:** for each Camel direction, `camel_manifest.py` reads the component's JSON from the snapshot and writes:
- `capabilities.source` and `capabilities.sink` from `consumerOnly` and `producerOnly`;
- `streaming` or `batch` from the component's consumer kind (scheduled polling consumer → `batch: true`, `resumable` only if the overlay names a high-water `id_expression`);
- a config schema from the endpoint's path and query parameters (types, enums, defaults, `required`; `secret: true` → `"writeOnly": true` and listed in `secrets`);
- `auth` from the options present (for example `sasl*`, `sslContextParameters` → `mtls`, `accessKey`/`secretKey` → `iam`), mapped to Appendix A's legend.

Delivery is `at_least_once` for sources and as Task 0 found for `camel-iggy` sinks. The generator never overwrites a field that a human edited after generation: it keeps a `generated:` block, and `--check` reports any difference.

Tests:
- `camel_direction_matches_catalog`: R-CN2-4, over all Camel directions.
- `generated_config_marks_secrets_write_only`
- `generator_is_idempotent`
- `snapshot_checksum_matches_readme`
- `weekly_upstream_diff_reports_only`: the network job in `connectors-drift` reports, and never rewrites.

Commit `connectors: generate P2 manifests and config schemas from camel-catalog`.

### Task 4: The licence gate for CI-only services and user-supplied drivers

**Files:** `connectors/licences.toml`, `fabric/crates/loams-flow/src/validate.rs` (`LicenceGate`), `scripts/connectors/image_licences.py`, and `.github/workflows/fabric.yml`.

**Semantics:** two new component kinds:
- `ci-service`: image, digest, SPDX id or `LicenseRef-<vendor>-EULA`, and the reason. It may appear only in `deploy/fabric/compose.yaml` profiles whose name starts with `connectors` and in CI workflow files.
- `user-supplied`: driver, SPDX id or `LicenseRef-*`, and the mount path. It may appear only as a documented mount, for example the Oracle `ojdbc` and MySQL Connector/J.

`image_licences.py` lists every file in each image Loams builds (`loams-connect`, the Debezium wrapper image if any, `loams-fabric`) and the default compose profile, and fails on any component of either kind. The denied list (AGPL, BSL, SSPL, ELv2, `NOASSERTION`, unknown) still applies to everything shipped. A `ci-service` may carry a denied id, which is the D60 precedent.

Tests:
- `ci_service_allowed_only_in_ci_profiles`
- `user_supplied_never_in_image`
- `no_ci_only_or_user_supplied_in_images`
- `denied_ids_still_refused_for_shipped`
- `licence_gate_refuses_flagged`: CN1's test, unchanged.

Commit `connectors: licence kinds for CI-only services and user-supplied drivers`.

## CN2b — Runtimes for P2 (Tasks 5–9)

### Task 5: Generic Camel route templates and overlays

**Files:** `connect/routes/templates/{source.yaml.tmpl,sink.yaml.tmpl}`, `connect/routes/overlays/README.md`, `fabric/crates/loams-flow/src/runtime/camel.rs` (extends CN1 Task 12's), `scripts/connectors/no_java.py`, and `.github/workflows/fabric.yml` (`connect-routes` switched on). Tests go in `fabric/crates/loams-flow/tests/camel.rs`.

**Interfaces:**
- `CamelRuntime::render(inst: &Instance, spec: &ConnectorSpec, overlay: &Overlay, dir: Direction) -> RenderedRoute { path, yaml, secret_mounts }`. The output is deterministic: byte-identical for identical inputs.
- `Overlay` as in the shared contract, loaded and validated against `connect/routes/overlays/overlay.schema.json`.

Tests:
- `rendered_routes_are_valid_yaml_dsl`: every overlay through `camel` JBang validation inside the pinned image, in CI.
- `render_is_deterministic`
- `templates_use_only_allowed_languages`
- `no_java_sources_in_tree`
- `source_sets_ce_headers_and_stable_id`
- `passthrough_keeps_type_source_id`
- `camel_source_kill_restart_no_loss`
- `camel_sink_commits_after_ack`, both against the `timer`/`log` components and a real broker from Task 9.
- `route_reload_replaces_one_instance_only`

Commit `connect: generic Camel source and sink templates with per-component overlays`.

### Task 6: Profiles — twins and HTTP profiles

**Files:** `fabric/crates/loams-flow/src/profile.rs`, `connectors/profiles/README.md`, `scripts/connectors/profiles.py` (the schema check), the `connectors::{http,webhook}` modules (CN1 Task 4) gaining profile execution, and the native `NativeRuntime` gaining `profile` resolution. Tests go in `fabric/crates/loams-flow/tests/profiles.rs`.

**Interfaces:**
- `Profile`, `load_profiles` and `effective_capabilities` as in the shared contract.
- `HttpProfileSource::new(profile, resource, config, secrets)` reuses CN1's polling engine (cursor, items pointer, `Retry-After`).
- `HttpProfileSink` reuses CN1's sink: batches up to `batch`, sends `Idempotency-Key: <ce_id>` where the API accepts it, and otherwise dedupes by the upstream's natural key named in `upsert.key`.
- A webhook scheme named in a profile must be one CN1 Task 4 supports, or an addition made here with fixtures.

Tests:
- `twin_inherits_module_minus_narrows`
- `http_profile_paginates_and_resumes`
- `http_profile_honours_rate_limit_headers`
- `http_profile_upsert_idempotent_on_retry`
- `profile_secrets_by_reference_only`
- `unknown_profile_refused`

Commit `flow: profiles for protocol twins and declarative HTTP APIs`.

### Task 7: Iggy P2 sinks

**Files:** `deploy/fabric/connectors/iggy/{mongodb_sink,redshift_sink,delta_sink,rabbitmq_sink}.toml.tmpl` (each only if Task 0 found the plugin), and `fabric/crates/loams-flow/src/runtime/iggy.rs` (the template registry). Tests go in `fabric/crates/loams-flow/tests/iggy_p2.rs`.

**Semantics:** `IggyRuntime` renders each plugin's TOML from the instance, with secrets as file paths (CN1 Task 3). For each plugin, the manifest declares exactly what Task 0 read from its README and code: upsert key, delete handling, batch size, and whether it is transactional. A plugin that Task 0 did not find falls back to the Camel component and its manifest's `runtime.sink`.

Tests:
- `mongodb_sink_upserts_by_key`
- `redshift_sink_batches` (fixture; nightly real account)
- `delta_sink_commits_table_versions` (RustFS)
- `rabbitmq_sink_publishes_with_ce_headers`
- `kill_restart_iggy_sink_no_loss`
- Contract suites for each.

Commit `connectors: Iggy sinks for MongoDB, Redshift, Delta and RabbitMQ`.

### Task 8: Debezium Server for SQL Server, Oracle, MongoDB and MariaDB

**Files:** `deploy/fabric/connectors/debezium/{sqlserver,oracle,mongodb,mariadb}.properties.tmpl`, `fabric/crates/loams-flow/src/runtime/debezium.rs` (extends CN1 Task 8's), `connectors/registry/{debezium-sqlserver,debezium-oracle,debezium-mongodb,debezium-mariadb}.yaml`. Tests go in `fabric/crates/loams-flow/tests/cdc_p2.rs`.

**Semantics** (§33 D357, §7): CN1 Task 8's model, with these per-source settings:
- **SQL Server:** CDC enabled per table by the user, with the plan's runbook giving the statements. The position is the LSN triple.
- **Oracle:** LogMiner. `ojdbc` is `user-supplied`, mounted at `/debezium/lib/ojdbc*.jar`. Supplemental logging is the user's job, and the runbook gives the commands.
- **MongoDB:** change streams on a replica set. `loamslsn` is the resume token's cluster time plus ordinal.
- **MariaDB:** Debezium's MariaDB connector with GTID.

Offsets and schema history are in files on a volume (Q350). Each source has a monitoring rule:
- SQL Server: CDC cleanup job lag;
- Oracle: archive log retention against the LogMiner position;
- MongoDB: oplog window against the resume token's age.

Tests:
- `sqlserver_cdc_converges`
- `oracle_cdc_converges` (CN2-Q6 decides whether CI or nightly)
- `mongodb_cdc_converges`
- `mariadb_cdc_converges`: each is a random workload of 10,000 operations into a Fluss PK `_current` table, which must equal the source.
- `debezium_sqlserver_restart_resumes`
- `oracle_without_ojdbc_refused_with_message`
- `mongodb_oplog_window_metric`
- Contract and `cdc` suites.

Commit `connectors: CDC for SQL Server, Oracle, MongoDB and MariaDB through Debezium Server`.

### Task 9: The P2 test estate: services, fixtures and nightly accounts

**Files:** `deploy/fabric/compose.yaml` (profile `connectors-p2`, every image from the Tech Stack pinned by digest), `fabric/crates/loams-flow-conformance/src/{fixture.rs,services.rs}`, `conformance/connectors/fixtures/<id>/`, and `.github/workflows/fabric.yml` (jobs `connectors-p2`, which is path-filtered and sharded by family, and `connectors-nightly`, which runs on a schedule with secrets).

**Interfaces:**
- `ServiceKind::{Container { compose_service }, Fixture { dir }, Nightly { env: &[&str] }}`. Each P2 manifest's harness declares one or more.
- `FixtureProxy`: a record-and-replay HTTP proxy for Camel and HTTP-profile traffic. On record, secrets are scrubbed by header and JSON-pointer rules. On replay, it matches by method, path and a canonical body. It fails on an unmatched request.
- `every_declared_capability_has_a_test` is extended to all `preview` manifests.

Tests:
- `fixture_replay_rejects_unrecorded_request`
- `fixture_recording_scrubs_secrets`: a grep for the test secrets finds none.
- `every_p2_preview_manifest_has_a_harness`
- `every_declared_capability_has_a_test`

Commit `ci: the P2 connector test estate, fixtures and nightly real-account jobs`.

## CN2c — The P2 connectors (Tasks 10–18)

Every family task follows the same shape, and each connector is done when:
1. its manifest is detailed (Task 3's generator output, reviewed and moved into `handwritten.txt`);
2. its overlay, profile or template exists;
3. its config schema marks secrets write-only;
4. its contract suite is green against its `ServiceKind`, with `roundtrip` where both directions exist and `kill-restart` for every streaming source;
5. it has a guide in `docs/guides/connectors/<id>.md`;
6. its `status` moves to `preview`.

Each task's tests list only what goes beyond the contract suite.

### Task 10: Brokers I — RabbitMQ/AMQP, ActiveMQ/JMS, NATS, Pulsar, MQTT

**Files:** overlays `{rabbitmq-amqp,activemq-jms,nats,pulsar,mqtt}.yaml`, manifests, `fabric/crates/loams-flow/tests/p2_brokers.rs`, guides.

**Semantics:**
- AMQP sources ack after the Iggy producer step returns, with `ce_` headers mapped from AMQP application properties. JMS and Artemis use client acknowledgement.
- NATS: JetStream durable consumers when configured (`resumable: true`), core NATS otherwise (`resumable: false`, declared).
- Pulsar: a shared or failover subscription; Pulsar's own CloudEvents properties pass through.
- MQTT 5: QoS 1 for both directions, and user properties map to `ce_` headers.

Tests:
- `amqp_ack_after_fabric`
- `jms_client_ack_redelivers_on_kill`
- `nats_core_declares_not_resumable`
- `nats_jetstream_resumes`
- `pulsar_ce_properties_pass_through`
- `mqtt_qos1_no_loss_on_restart`

Commit `connectors: AMQP, JMS, NATS, Pulsar and MQTT through loams-connect`.

### Task 11: Brokers II (cloud) — SQS, SNS, EventBridge, Pub/Sub, Event Hubs, Service Bus, Event Grid, WebSocket and gRPC

**Files:** overlays for each, manifests, `fabric/crates/loams-flow/tests/p2_cloud_brokers.rs`, guides.

**Semantics:**
- SQS deletes a message only after the Fabric acknowledges, with the visibility timeout taken from the config.
- SNS and Event Grid are sinks plus webhook sources. Event Grid uses the CloudEvents schema, so input passes through. The SNS subscription-confirmation handshake is handled in the overlay.
- Pub/Sub: `ce-` attributes pass through, and the message is acked after the Fabric acknowledges.
- Event Hubs uses checkpoints in Azure Blob (Azurite in CI).
- Service Bus uses peek-lock and completes after the Fabric acknowledges.
- WebSocket declares `resumable: false`.
- gRPC follows CN2-Q4.

Tests:
- `sqs_delete_after_ack`
- `sns_subscription_confirmation`
- `eventgrid_cloudevents_pass_through`
- `pubsub_ce_attributes_pass_through`
- `eventhubs_checkpoint_resume`
- `servicebus_peek_lock_redelivery`
- `websocket_declares_not_resumable`

Commit `connectors: cloud brokers and WebSocket through loams-connect`.

### Task 12: Protocol twins

**Files:** `connectors/profiles/{redpanda,valkey,redis-streams,minio-rustfs,cloudflare-r2,aurora,cockroachdb,pgvector,mariadb,opensearch,duckdb}.yaml`, manifests, `fabric/crates/loams-flow/tests/p2_twins.rs`, guides.

**Semantics:** each twin runs the ★ module's own contract suite against the twin's service. Anything that fails goes into `narrows`, with a note. Known differences, to be verified:
- CockroachDB has no `COPY … TO STDOUT (FORMAT binary)` (verify), so its batch source uses the `SELECT` fallback path.
- R2 has no event notifications in CI, so it is polling only.
- Aurora IAM tokens, nightly only.
- pgvector needs a `vector` ↔ Arrow `FixedSizeList<Float32>` mapping.
- Redpanda is checked against the `ce_` header layout of D270.

Tests:
- `twin_suite_runs_against_twin_service`: parametrised over all twins.
- `cockroach_batch_uses_select_fallback`
- `pgvector_roundtrip_fixed_size_list`
- `redpanda_ce_headers_pass_through`
- `r2_declares_no_notifications`

Commit `connectors: protocol twins on the starred modules`.

### Task 13: Relational and warehouse through Camel — SQL Server, Azure SQL, Oracle, Trino, Databricks and Redshift (source)

**Files:** overlays, manifests, `fabric/crates/loams-flow/tests/p2_relational.rs`, guides.

**Semantics:** Camel `sql` with a high-water query, the CN1 Task 12 pattern (the idempotent repository Task 0 verified), and drivers per CN1 Ruling 8:
- `mssql-jdbc` (MIT) may ship in the image if Task 0 confirms its licence; otherwise it is `user-supplied`.
- `ojdbc` is `user-supplied`.
- The Trino JDBC driver is Apache-2.0.
- The Databricks JDBC driver follows Task 0's licence finding.

Rows are JSON in `rows` events. These are not bulk paths: bulk reads from these systems go through ADBC where a driver exists, as Task 0 records. Upsert sinks use `MERGE` (SQL Server, Oracle) through `sql` batch mode.

Tests:
- `sqlserver_high_water_resumes`
- `oracle_merge_upsert`
- `trino_source_only`: the sink is refused by `check_use`.
- `databricks_fixture_replay`
- `redshift_data_api_source_fixture`

Commit `connectors: SQL Server, Oracle, Trino, Databricks and Redshift through loams-connect`.

### Task 14: NoSQL, search and vector — MongoDB, Cassandra, DynamoDB, DynamoDB Streams and Qdrant

**Files:** overlays, manifests, `fabric/crates/loams-flow/tests/p2_nosql.rs`, guides.

**Semantics:**
- MongoDB: a Camel batch source with a `_id` high-water mark, and the Iggy sink (Task 7).
- Cassandra: CQL batch reads by token range and upserts. Writes are idempotent by primary key.
- DynamoDB: a Camel scan source and `PutItem`/`BatchWriteItem` sink.
- DynamoDB Streams: a Camel `aws2-ddbstream` source with shard iterators. Its `change` events carry `loamsop` from the stream's `eventName`.
- Qdrant: the Camel `qdrant` component, checked against Qdrant and against Loams' own Qdrant gateway (`crates/loams-qdrant`), so a route into "Qdrant" can target a Loams collection unchanged.

Tests:
- `mongodb_batch_high_water`
- `cassandra_token_range_partitions`
- `dynamodb_streams_ops_map_to_loamsop`
- `qdrant_sink_to_qdrant_and_loams`

Commit `connectors: MongoDB, Cassandra, DynamoDB and Qdrant`.

### Task 15: Graph — Grafeo as a `loams.graph.v1` client, and Neo4j

**Files:** `fabric/crates/loams-flow/src/connectors/{graph.rs,neo4j.rs}`, `connectors/registry/{grafeo,neo4j}.yaml`, `fabric/crates/loams-flow-proto/build.rs` (the `loams.graph.v1` client, per Task 0's ruling), `fabric/crates/loams-flow/tests/p2_graph.rs`, guides.

**Semantics:**
- **Grafeo** (§48 §4.4): the sink sends `ExecuteBatch` of parameterised GQL `INSERT`/`MERGE` statements built from the instance's mapping. The source sends `ExecuteStream` of a GQL query, with a cursor property as the high-water mark. Auth is `key`, a Loams API key from the `SecretStore`. The manifest drops D634 (b)'s in-process claim, and its comment cites D741.
- **Neo4j:** the sink is Camel `neo4j` (Cypher `MERGE`). The source is native over Neo4j's HTTP Query API (`POST /db/{db}/query/v2`, verify the minimum server version), with a cursor property. There is no Bolt anywhere (D634 (c)).

Tests:
- `grafeo_sink_batches_through_loams_graph_v1`, against `loams dev --graph` when available, else a fake service.
- `grafeo_source_cursor_resumes`
- `grafeo_requires_api_key`
- `neo4j_sink_merge_idempotent`
- `neo4j_source_http_query_api_cursor`
- `no_bolt_dependency` (a `cargo tree` check)

Commit `connectors: Grafeo over loams.graph.v1 and Neo4j`.

### Task 16: Objects, lakehouse and formats — GCS, Azure Blob, ADLS Gen2, SFTP, Delta Lake, CSV/NDJSON and Protobuf

**Files:** overlays for GCS, Azure Blob, ADLS Gen2 and SFTP; `fabric/crates/loams-flow/src/formats/{csv.rs,ndjson.rs,protobuf.rs}`; manifests; `fabric/crates/loams-flow/tests/{p2_objects.rs,formats_p2.rs}`; guides.

**Semantics:**
- Object sources: listing with a high-water key, as CN1's S3. Event notifications where the cloud has them: GCS Pub/Sub notifications and Blob Event Grid, both nightly only. Ids are `"<container>/<key>@<etag>"`, per §33 §6.
- SFTP is a polling source; the sink writes and then renames, so readers never see partial files.
- Delta Lake is a sink only, through Task 7.
- Formats: `arrow-csv` and `arrow-json` 59 for CSV and NDJSON, decoding to `RecordBatch` (bulk stays columnar), and `prost-reflect` over a user-supplied `FileDescriptorSet` for Protobuf. Confluent framing for Protobuf is supported when a registry is configured (Q344's stub, as for Avro).

Tests:
- `gcs_and_blob_high_water_resume`
- `sftp_sink_atomic_rename`
- `csv_ndjson_roundtrip_types`
- `protobuf_descriptor_roundtrip`
- `bulk_path_has_no_row_decode`, for the CSV and NDJSON paths.

Commit `connectors: GCS, Azure Blob, ADLS, SFTP, Delta Lake sink, and CSV, NDJSON and Protobuf codecs`.

### Task 17: SaaS and collaboration

**Files:**
- Overlays for Salesforce, Zendesk, ServiceNow, Stripe, GitHub, Jira, Slack, Google Workspace, Twilio SMS and Amazon SES.
- HTTP profiles for Segment, SendGrid, PostHog, HubSpot, Shopify, GitLab and the Kestra companion.
- Manifests, `fabric/crates/loams-flow/tests/p2_saas.rs` and guides.

**Semantics:**
- All of these are tested on fixtures, plus nightly runs on real sandboxes where credentials exist. Stripe uses `stripe-mock` in CI.
- Salesforce uses raw JSON mode (no generated DTOs, since Java is deferred), so `notes` records the narrowing. CDC through Salesforce's change events is a streaming source that resumes from its replay id.
- GitHub, Jira and Slack sources feed CN1 Task 15's import templates. Those templates now run on real overlays rather than placeholders, and Task 17 replays them end to end on fixtures.
- Webhooks for Stripe, GitHub, Slack and Shopify reuse CN1 Task 4's schemes. HubSpot v3 signatures are added in Task 6 if they are missing.
- Kestra companion: a sink that starts Kestra flows through its webhook trigger, and a source that receives Kestra's webhook. No Kestra plugin is written (Q356, deferred with Java).

Tests:
- `salesforce_cdc_replay_id_resumes` (fixture)
- `stripe_mock_roundtrip`
- `github_to_forgejo_import_replays`
- `jira_to_itsplane_import_replays`
- `slack_to_zulip_import_replays`
- `hubspot_profile_upsert_batches`
- `shopify_webhook_verifies`
- `kestra_companion_triggers_flow` (fixture)

Commit `connectors: SaaS and collaboration connectors, and the import routes on real sources`.

### Task 18: Observability, infrastructure, identity, AI and protocols

**Files:**
- Overlays for Splunk, Lambda / Cloud Run / Azure Functions, Vault, Keycloak, OpenAI, Anthropic and GraphQL.
- `fabric/crates/loams-flow/src/connectors/{prometheus.rs,mcp.rs}` (`mcp.rs` only if CN2-Q9 keeps it).
- Manifests, `fabric/crates/loams-flow/tests/p2_misc.rs` and guides.

**Semantics:**
- Splunk: a HEC sink with acknowledgements on, so the sink is idempotent only if HEC's indexer ack is on, as declared. The source is a search export, fixture only.
- Prometheus: a remote-write receiver on `ingest`'s listener under `/v1/namespaces/{ns}/fabric/prometheus/{instance}/write` (snappy protobuf, with one `metric` event per sample batch per series), and a remote-write sink.
- Functions: invoke as a sink with `sink.reply` (the response becomes a `reply` event).
- Vault: tested against OpenBao (the Vault API). It is a sink (write secrets) and a source (read leases); its `notes` say it is not Loams' own secret store.
- Keycloak: admin events as a source, and user and group upserts as a sink.
- OpenAI and Anthropic: enrichment sinks with `sink.reply: true`, rate limits declared, and the prompt templates in the instance config. No key ever appears in a reply event.
- GraphQL follows Task 0's direction finding.
- MCP follows CN2-Q9.

Tests:
- `splunk_hec_ack_declared`
- `prometheus_remote_write_roundtrip`
- `lambda_invoke_emits_reply`
- `openbao_kv_roundtrip`
- `keycloak_admin_events_source`
- `openai_reply_has_no_key_and_keeps_subject` (fixture)
- `anthropic_reply_fixture`
- `graphql_direction_matches_catalog`

Commit `connectors: observability, infrastructure, identity, AI and GraphQL`.

## CN2d — Production (Tasks 19–25)

### Task 19: Connector metrics, alerts and the activity observer

**Files:** `fabric/crates/loams-flow/src/{metrics.rs,activity.rs}`, `deploy/observability/connectors/{dashboards/connectors.json,alerts.yaml}`, the Camel overlay template (JMX or Micrometer metrics exposed through Camel Main's metrics endpoint, verify), Debezium's metrics scrape, and the Iggy runtime's metrics. Tests go in `fabric/crates/loams-flow/tests/activity.rs`.

**Interfaces:**
- Metric families, all labelled `namespace`, `instance`, `connector`, `runtime` and `direction`, never with a secret or a user-supplied free string:
  - `loams_flow_connector_events_total`
  - `loams_flow_connector_bytes_total{dir="in|out"}`
  - `loams_flow_connector_api_calls_total`
  - `loams_flow_connector_errors_total{class}`
  - `loams_flow_connector_lag_seconds`
  - `loams_flow_connector_restarts_total`
  - `loams_flow_connector_dlq_total`
- `ConnectorActivityObserver`, as in the shared contract, called once per runtime scrape window (default 60 s) per instance. The Camel, Debezium and Iggy figures come from each runtime's own metrics. Native figures come from the task's counters.
- `InstanceStatus` (CN1 Task 3) gains `last_error`, `lag_seconds` and `restarts` from the same source.
- Alerts:
  - instance down for more than 5 min;
  - lag above the instance's `lag_alert_seconds` (default 300);
  - CDC slot or oplog risk (from Task 8 and CN1 Task 8);
  - DLQ growth;
  - restart loop (more than 5 in 10 min).

Tests:
- `metrics_labels_bounded`
- `activity_observer_called_per_window`
- `activity_observer_has_no_wire_format`: the type has no `Serialize` impl, and no socket or file is opened.
- `each_alert_fires_in_a_test` (promtool rule tests)
- `no_metering_guard_green`

Commit `flow: connector metrics, alerts and the activity observer`.

### Task 20: Security — egress guard, secret stores, log scrubbing and the threat model

**Files:**
- `fabric/crates/loams-flow/src/egress.rs` and `fabric/crates/loams-flow/src/secrets/{file.rs,kube.rs}` (CN1 Task 3's `FileSecretStore` moves here).
- Every runtime renderer, which gains egress checks and a NetworkPolicy.
- `docs/security/connectors-threat-model.md`.
- Tests in `fabric/crates/loams-flow/tests/{egress.rs,secrets_p2.rs}`.

**Interfaces:**
- `EgressPolicy`, as in the shared contract. It applies in two places:
  - at `ValidateInstance`, to every host named in config;
  - at connect time, after DNS: native connectors go through a resolver hook on `reqwest` and `tokio` connectors, and Camel, Debezium and Iggy pods through a Kubernetes NetworkPolicy rendered from the same policy, with the policy's ipBlocks.
- `KubeSecretStore`: Kubernetes Secrets in the instance's namespace, mounted read-only into runtime pods as files. A Dapr-backed store (D189) follows when `loams-dapr` exists (CN2-Q15).
- Log scrubbing: the Camel image's logging config masks known secret option names, and Debezium's masks `*.password`. A line filter in the supervisor drops any line containing a resolved secret's bytes before forwarding, and counts it.
- Threat model: tenant isolation of the runtime pods (one namespace group per tenant, or per instance for heavy instances); SSRF; secret exposure; webhook forgery; a malicious Camel component option (`#bean`, `exec:` and `file:` refused by the overlay schema); and supply chain (digests, signed images).

Tests:
- `egress_guard_refuses_metadata_loopback_and_cluster`
- `egress_guard_rechecks_after_dns` (DNS rebinding)
- `network_policy_rendered_from_policy`
- `no_secret_in_camel_or_debezium_logs`
- `no_secret_in_rendered_kube_objects`
- `overlay_refuses_bean_exec_file_schemes`
- `kube_secret_store_reads_mounted_file`

Commit `flow: egress guard, Kubernetes secret store, log scrubbing and the connector threat model`.

### Task 21: The connector fleet on Kubernetes, and the Helm chart

**Files:** `fabric/crates/loams-flow/src/runtime/kube/{mod.rs,connect.rs,debezium.rs,iggy.rs,native.rs,lease.rs}`, `deploy/helm/loams-connectors/`, and `.github/workflows/fabric.yml` (job `connectors-kind`). Tests go in `fabric/crates/loams-flow/tests/kube.rs` (golden) and `tests/it_kind.rs` (`#[ignore]`, CI).

**Interfaces:**
- `KubeRuntime` implements CN1's `ConnectorRuntime` for each kind and renders:
  - `loams-connect`: one Deployment per namespace group (default: per Loams namespace), with routes in a ConfigMap per instance, reloaded by Camel's route reloading;
  - Debezium Server: one StatefulSet per CDC instance with a PVC for offsets and history (Q350);
  - Iggy connectors runtime: one Deployment per namespace group;
  - native connectors: tasks inside `loams-fabric flow` pods.
- Rendering is deterministic (golden tests) and labelled `loams.dev/namespace`, `loams.dev/instance`, `loams.dev/connector` and `loams.dev/runtime`.
- `InstanceLease`: a fenced lease per instance on `_fabric.flow_objects` (CAS on `version`, holder and expiry), held by the `flow` process that supervises it. FL3's metastore leases replace it behind the same trait (CN2-Q10).
- Helm chart: values for runtime images (digests), resource budgets per runtime (from Task 22's measurements), NetworkPolicies (Task 20), ServiceMonitors (Task 19) and an optional PodDisruptionBudget per runtime.

Tests:
- `render_golden_per_runtime`
- `two_flow_processes_one_instance_runner`
- `stale_lease_holder_fenced`
- `it_kind_mixed_fleet` (a Camel source, a Debezium CDC instance, an Iggy sink and a native source all running; kill each pod; no loss)
- `helm_lint_and_template_golden`

Commit `deploy: the connector fleet on Kubernetes and the loams-connectors chart`.

### Task 22: Camel upkeep — version bumps, Quarkus measurement and JVM budgets

**Files:** `connect/README.md` (the bump procedure), `.github/workflows/fabric.yml` (job `camel-bump`, manual dispatch), `docs/plans/cn2-dependency-spike.md` (measurements), and the `deploy/helm/loams-connectors/values.yaml` budgets.

**Semantics:**
- **Bump procedure (CN-R5):** a Camel bump PR regenerates the catalog snapshot (Task 3), reruns `camel_direction_matches_catalog`, validates all routes, and runs every Camel-backed contract suite. A narrowed capability bumps that manifest's major version (D353 rule 4).
- **Q352 measurement:** Camel Main on the JVM against Camel Quarkus JVM and native, comparing startup, RSS with 1, 10 and 50 routes, and throughput on the AMQP and SQL overlays. The owner decides from the numbers whether Quarkus replaces Main (CN-R2). Until then, Main stays.
- **JVM budgets:** heap and container memory per route count, recorded and written into the chart's defaults.

Tests:
- `camel_bump_dry_run_green`: the job run against the current pin.
- `budget_values_match_measurements`

Commit `connect: the Camel bump procedure, the Quarkus measurement and JVM budgets`.

### Task 23: Remote ingress for webhooks, OTLP and Prometheus (gated on Q30)

**Files:** the edge configuration that Q30's plan names (Envoy, D184), `fabric/crates/loams-flow/src/connectors/{webhook.rs,otlp.rs,prometheus.rs}` (listener binding), and docs.

**Semantics:** when the unified auth plan has landed, the `ingest` listener's connector routes are exposed through the edge with TLS and per-route credentials. Webhooks keep their provider signatures as a second factor. OTLP and Prometheus remote-write take a per-instance bearer key. Until then, this task does not start, and the loopback rule holds (§33 §2.2).

Tests:
- `edge_route_requires_credential`
- `webhook_signature_still_checked_behind_edge`
- `otlp_remote_with_key`
- `loopback_only_before_gate` (always runs)

Commit `fabric: remote ingress for webhooks, OTLP and Prometheus through the edge`.

### Task 24: Upstream contributions (Q357)

**Files:** PRs on `dina-kar/iggy`, posted upstream only with the owner's go-ahead (§23 §2.2); `docs/plans/cn2-exit-report.md` (the list of upstream issues and PRs).

**Semantics:**
- Port the native Kafka, ADBC and Kinesis connectors (CN1 Tasks 6, 11 and 13) to Iggy's connector SDK as plugins.
- The `loams-flow` modules stay the source of truth until the plugins are released.
- Open issues upstream for every gap CN2 recorded: `camel-iggy` commit semantics, missing Camel catalog JSON (`cn1-dependency-spike.md` §5), and Debezium property gaps.

Tests: each plugin's own tests in the fork; the CN1 contract suites, run against the plugin builds through `IggyRuntime`, are green.

Commit (fork): `connectors: kafka, adbc and kinesis plugins`. Commit (here): `docs(connectors): upstream contributions`.

### Task 25: The CN2 gate, catalog page, runbooks and exit report

**Files:** `fabric/crates/loams-fabric/tests/e2e.rs` (`cn2_*`), `docs/guides/connectors/index.md` (regenerated), `docs/runbooks/connectors/{camel.md,debezium-cdc.md,iggy-runtime.md,secrets.md,egress.md}`, `docs/plans/cn2-exit-report.md`, `docs/plans/README.md` (CN2 status), `docs/design/33-connectors.md` (§8 rollout and §9 as built), and `CHANGELOG.md`.

**Semantics:** the gate is green when:
- every P2 manifest is `preview` with its suites green, or `planned` with a `notes` entry and an open question that owns it;
- the 24-hour mixed soak on kind passes: 20 instances across all four runtimes, with pod kills every 15 minutes, no loss, duplicates within the declared delivery, and no alert flapping;
- the three import routes (Slack to Zulip, GitHub to Forgejo, Jira to ItsPlane) run end to end on fixtures;
- the CDC convergence of Task 8 holds for all four new sources.

The exit report lists, per connector: throughput and latency, duplicate counts in kill tests, the narrowings recorded, and the upstream issues opened.

Tests:
- `cn2_mixed_soak` (CI scheduled, 24 h)
- `cn2_cdc_four_sources_converge`
- `cn2_import_routes_end_to_end`
- `catalog_page_matches_registry`
- `p2_all_preview_or_explained`

Steps: each runbook step is executed once on kind and marked verified → commit `connectors: the CN2 gate, catalog page, runbooks and exit report`.

---

## Exit criteria for production (with the owning tasks)

- [ ] **Registry truth:** per-direction runtimes, profiles, the P2 runtime assignment, the Debezium split and catalog-derived capabilities. Appendix A equals the CSV: Tasks 1–3.
- [ ] **Licences:** CI-only and user-supplied components are never in an image. D359 holds for everything shipped: Task 4.
- [ ] **Runtimes:** generic Camel templates with no Java, profiles, Iggy P2 sinks, and Debezium for SQL Server, Oracle, MongoDB and MariaDB: Tasks 5–8.
- [ ] **Every P2 connector** is `preview` with green suites, or `planned` with an owned reason: Tasks 9–18.
- [ ] **Observability:** metrics, dashboards, every alert tested, and the activity observer with no billing: Task 19.
- [ ] **Security:** the egress guard, Kubernetes secrets, log scrubbing, the threat model, and no secret anywhere: Task 20.
- [ ] **Fleet:** all four runtimes on Kubernetes, the fenced instance lease, and the Helm chart: Task 21.
- [ ] **Camel upkeep:** the bump procedure, the Q352 numbers to the owner, and JVM budgets: Task 22.
- [ ] **Remote ingress:** behind the edge once Q30 lands, loopback until then: Task 23.
- [ ] **Upstream:** the Q357 plugins proposed, and gaps filed: Task 24.
- [ ] **Gate:** the soak, CDC convergence, import routes, catalog page, runbooks and exit report: Task 25.

## Self-review

- **Spec coverage.**

  | §33 section | Task(s) |
  |---|---|
  | §1 D352 registry, instances | 1, 2, 3 |
  | §1 D353 capabilities, refusal, tests per capability | 1, 3, 9, 10–18 |
  | §1 D354 runtimes, buy first | 2, 5, 6, 7, 8, 22 |
  | §1 D355 envelope, pass-through | 5, 11, 12, shared event names |
  | §1 D356 bulk as Arrow | 12, 13 (not bulk, declared), 16 |
  | §1 D357 CDC | 8, 14 |
  | §1 D358 rollout (CN2 = P2) | 2, 10–18, 25 |
  | §1 D359 licence gate | 4 |
  | §2.2 remote ingress | 23 |
  | §8 rollout, §9 gate (extended to CN2) | 25 |
  | §10 open core, platform fleet | 19 (observer), 21 |
  | §11 CN-R1 drift | 3, 22 |
  | §11 CN-R2 JVM footprint | 21, 22 |
  | §11 CN-R3 Debezium pods | 8, 21 |
  | §11 CN-R5 `camel-iggy` preview | 0, 5, 22, 24 |
  | §11 CN-R6 SaaS changes | 9, 17 |
  | §11 CN-R7 secrets | 20 |
  | Q352 | 22 |
  | Q357 | 24 |
  | Q359 | 19 |
  | Appendix A P2 rows | 2, 10–18 |

- **Types.** The manifest additions, `Profile`, the Camel route contract, the P2 event names, `ConnectorActivityObserver` and `EgressPolicy` are defined once, in the shared contracts. CN1's `ConnectorRuntime`, `Source`, `Sink`, `SecretStore` and `ConnectorHarness` are used unchanged.
- **Review Focus.** Items 1–9 each name an owning test (Tasks 3, 5, 6, 8, 9, 11, 12, 4, 19, 20, 21).
- **Placeholders.** Cells marked (verify) are Task 0's checks, and every one has a named fallback.

## Open questions

These need an owner answer before the task named. Each has a default the plan follows if the owner accepts it.

| # | Question | Default | Needed by |
|---|---|---|---|
| CN2-Q1 | A manifest with one runtime per direction (this plan's schema change), or one manifest per runtime with suffixed ids (`mongodb-camel`, `mongodb-iggy`)? | Per direction: one id per system keeps Appendix A one row per connector | Task 1 |
| CN2-Q2 | Are the protocol twins (Redpanda, Valkey, Redis Streams, MinIO/RustFS, R2, Aurora, CockroachDB, pgvector, MariaDB, OpenSearch, DuckDB) a correct reading of D354's "native where Loams has the code", in place of the Camel components Appendix A names? | Yes: fewer JVM routes, and the ★ suites already exist | Task 2 |
| CN2-Q3 | The nine P2 rows with no Camel or Iggy component: ship them in CN2 as declarative HTTP profiles on the ★ HTTP connector, or move them to CN3's generator (Q358)? | Profiles in CN2. CN3's generator later emits the same profile format | Task 2 |
| CN2-Q4 | gRPC and anything else needing generated Java while Java is deferred: a native dynamic client (`tonic` with `prost-reflect`), narrowed capabilities, or wait for Java? | Protobuf as a native format now. gRPC stays `planned` until the owner picks native or Java | Task 2 |
| CN2-Q5 | Split Appendix A's "Debezium-SQL Server / Oracle" row into two manifests on Debezium Server (catalog 204 → 205), instead of Camel's embedded `debezium-*`? | Yes (D357) | Task 2 |
| CN2-Q6 | Oracle: `ojdbc` is under Oracle's Free Use Terms (not OSI). Is Oracle Database Free acceptable as a CI-only image, or is Oracle tested only nightly by the owner's account, or on recorded fixtures? | `user-supplied` driver; CI on Oracle Free only if the owner accepts its terms for CI, else nightly | Task 8 |
| CN2-Q7 | Proprietary-EULA test images (SQL Server Developer, the Event Hubs and Service Bus emulators, Splunk, CockroachDB): extend D60's CI-only precedent to them? | Yes, as `ci-service` with `LicenseRef-*`, never shipped | Task 4 |
| CN2-Q8 | Appendix A.3 says Neo4j's source is "a native connector over the engine's gRPC Query Service", which does not match any code path. Read it as native over Neo4j's HTTP Query API (no Bolt, D634 (c)), or make Neo4j sink-only? | HTTP Query API; amend the A.3 note in Task 2 | Task 2 |
| CN2-Q9 | MCP has no Camel component at 4.22.1. Native MCP client (tool-call sink with `reply`) in CN2, or move the row to P3? | Move to P3 unless a launch customer needs it (Q355's rule) | Task 2 |
| CN2-Q10 | Kubernetes rendering and multi-process supervision: CN2 Task 21 (with a `_fabric.flow_objects` lease replaced later by FL3's), or wait for FL3? | CN2 renders; FL3 replaces the lease behind the same trait | Task 21 |
| CN2-Q11 | Remote ingress (Task 23): is Q30's auth plan still the gate, or does the owner want an interim static-token edge route for webhooks? | Q30 stays the gate | Task 23 |
| CN2-Q12 | Delta Lake source: a native `deltalake`-crate reader in CN2, or sink-only until CN3? | Sink-only in CN2 | Task 2 |
| CN2-Q13 | AI connectors before FL3's enrichment steps: model them as sinks with `sink.reply` and a reply topic (this plan), or wait for FL3's `map`/enrich step? | `sink.reply` now; FL3 can wrap it | Task 1 |
| CN2-Q14 | Grafeo's client of `loams.graph.v1` from the `fabric/` workspace: generate it from the engine's proto path across workspaces, or keep a checked copy with a drift test? | Generate across workspaces if `build.rs` can; else a copy plus a drift test | Task 15 |
| CN2-Q15 | The production secret store: `KubeSecretStore` now and Dapr (D189) when `loams-dapr` exists, or block on Dapr? `loams-dapr` is not in the tree today | Kubernetes Secrets now | Task 20 |

## Rulings made during execution

(Task 0 and later tasks append here.)
