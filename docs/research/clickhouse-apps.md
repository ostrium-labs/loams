# Open-source applications that use ClickHouse (checked 2026-10-22)

This survey checks which open-source applications keep their data in ClickHouse, so that Loams House (design [75](../design/75-loams-house-engine.md), plan [HE1](../plans/2026-10-21-he1-loams-house-engine.md)) can pick its next gate applications. The owner asked for it on 2026-10-22, together with the queued gates OpenMeter, Helicone and a BI-tools gate (plan HE1 Tasks 13 to 15). **Owner's decisions, 2026-10-22:** Laminar replaces Helicone (Opik next); FSL is treated like BSL under D11 (D1574); the order after SigNoz is the BI tools, then OpenMeter, then Laminar.

**How it was checked.** Only the GitHub REST API, each repository's `LICENSE` file, its compose files and dependency manifests (read as raw files), Docker Hub's tag API and the projects' own docs. Nothing was cloned and no image was pulled. Licences are read from the `LICENSE` file, not from GitHub's licence detector, which reports `NOASSERTION` for split licences. Anything not confirmed is marked "(verify)".

**Protocol.** "HTTP" is ClickHouse's HTTP interface (port 8123); "native" is its TCP protocol (port 9000). Client libraries tell them apart: `@clickhouse/client`, `clickhouse-connect`, `clickhouse-jdbc` 0.9+, the Rust `clickhouse` crate, Ruby `clickhouse-activerecord` and Elixir `ch` speak HTTP; `clickhouse-go` v2 (unless set to HTTP), `ch-go` and Python `clickhouse-driver` speak native. Today the House speaks HTTP only; the native listener is being built for SigNoz (D1571).

## 1. The verified table

"Stack" counts the services in the project's own self-host compose file. "Maintained" means a push in the last month unless noted.

### Current gate applications and the ones already mined

| App | Repo | Licence (from `LICENSE`) | Latest release | ClickHouse | Protocol | Stack | Fit for Loams |
|---|---|---|---|---|---|---|---|
| Langfuse | `langfuse/langfuse` | MIT, except `ee/`, `web/src/ee/` and `worker/src/ee/` under the Langfuse Enterprise licence (needs a subscription). `ee/` holds SSO settings, multi-tenant SSO, the audit-log viewer, admin API, billing, UI customisation, verified domains and SFDC sync. **Copyright is now "ClickHouse, Inc."**: Langfuse joined ClickHouse in January 2026 (README; langfuse.com/blog/joining-clickhouse) | v4.67.0, 2026-10-21; v3 still patched (v3.225.12, 2026-10-12) | Required | Both: queries HTTP, migrations native (R0.5) | 6: web, worker, Postgres, ClickHouse, Redis, MinIO | **Gate 1.** MIT core runs without `ee/` |
| OpenPanel | `Openpanel-dev/openpanel` | AGPL-3.0, no split | v3.9.1, 2026-10-21, but **every v3 release is marked pre-release** and Docker Hub has no v3 image; newest stable image 2.3.0 (2026-08-18) | Required | HTTP (`@clickhouse/client`) | 8: Caddy, Postgres, Redis, ClickHouse, Redpanda, api, dashboard, worker | **Gate 2.** Run unmodified (AGPL) |
| SigNoz | `SigNoz/signoz` | MIT, except `ee/` and `cmd/enterprise/` under the SigNoz Enterprise licence (production use needs a subscription). Collector (`signoz-otel-collector`) AGPL-3.0 | v0.146.0, 2026-10-14 (weekly) | Required | Native (`clickhouse-go`) | Compose is deprecated for "Foundry" (`SigNoz/foundry`, AGPL-3.0); still needs ZooKeeper | **Gate 3** (D1571), MIT build `signoz/signoz-community` only |
| Plausible CE | `plausible/analytics` | AGPL-3.0, **but `extra/` is proprietary** ("no rights to use, distribute or otherwise exploit", `extra/COPYING.txt`). The CE build (`MIX_ENV=ce`) compiles `lib/` only and leaves `extra/` out | v3.2.1, 2026-05-15 (no newer CE release) | Required | HTTP (`ecto_ch`) | 3: app, Postgres, ClickHouse | Mined. Use only the `community-edition` image |
| Umami | `umami-software/umami` | MIT, no split | v3.4.0, 2026-09-17 | **Optional** (Postgres always; ClickHouse when `CLICKHOUSE_URL` is set) | HTTP (`@clickhouse/client`) | 2 by default (app, Postgres) | Mined. Easy, but its schema fails on ClickHouse 26.7+ (R0.6) |
| HyperDX / ClickStack | `hyperdxio/hyperdx` | MIT, no split. ClickStack is ClickHouse's product | `@hyperdx/app@2.41.0`, 2026-10-13 | Required | Both: app HTTP, migrations and collector native | 4: MongoDB, collector, app, ClickHouse | Mined |
| OpenLIT | `openlit/openlit` | Apache-2.0, no split | Platform `openlit-2.1.0`, 2026-09-10 (other tags are SDKs) | Required | HTTP (`@clickhouse/client`) | 3: ClickHouse, NATS, app | Mined. Small and HTTP-only |
| PostHog | `PostHog/posthog` | MIT, except `ee/` under the PostHog Enterprise licence (production use needs a subscription); `ee/` holds billing, the AI assistant (`hogai`), session-recording and survey pieces and more. **`PostHog/posthog-foss`** is a maintained MIT-only mirror (pushed 2026-10-22) | No versioned releases; docs say self-hosting is "officially unsupported" | Required | Both (`clickhouse-driver` native and `clickhouse-connect` HTTP) | About 39 services in `docker-compose.hobby.yml` (Kafka, ZooKeeper, Temporal, Elasticsearch, many Rust and Node services) | Skipped (stack size) |

### LLM and agent observability

| App | Repo | Licence | Latest release | ClickHouse | Protocol | Stack | Fit for Loams |
|---|---|---|---|---|---|---|---|
| Helicone | `Helicone/helicone` | Apache-2.0, no `ee/` split | GitHub releases and **every Docker Hub image stop at 2025-08-21** (`helicone-all-in-one`, `jawn`, `web`; `worker` 2025-06-04). The public repository is now a mirror: recent PRs (#5841, #5842, #5844) "sync" fixes "from the private repo". The site banner says "Helicone is no longer part of Mintlify. Details to come soon." The production Helm chart is on request only (`docker/README.md`) | Required | HTTP (`@clickhouse/client`, `@clickhouse/client-web` in the Workers) | 9 core in `docker/docker-compose.yml`: Postgres (Supabase migrations), ClickHouse 24.3, MinIO, MinIO setup, Mailhog, Redis, migrations, jawn, web; plus Cloudflare Workers built locally and Kafka with ZooKeeper behind profiles | **Weak**: images 14 months old, development private, ownership unsettled. Queued first, then **dropped for Laminar** (owner, 2026-10-22; R14.2) |
| Laminar | `lmnr-ai/lmnr` | Apache-2.0 (`LICENSE.md`), no split in the repository. `frontend-ee` and `app-server-ee` images exist on GHCR, built from code not in the repository (verify) | v0.3.2, 2026-10-16 | Required (Postgres for state, Quickwit for search) | HTTP only (Rust `clickhouse` crate 0.15, `@clickhouse/client`) | 5: Postgres, ClickHouse 26.5, Quickwit, frontend, app-server (7 in the full file: adds RabbitMQ, Valkey) | **Task 14 gate** (owner, 2026-10-22). Smallest LLM stack; adds the Rust client's `RowBinary` path |
| Opik | `comet-ml/opik` | Apache-2.0, no split (a CLA for contributors) | 2.2.113, 2026-10-21 (several a day) | Required (MySQL for state, Redis) | HTTP (`clickhouse-java` 0.10 client-v2 and R2DBC over HTTP; Liquibase migrations over JDBC on 8123) | 14 names, many behind profiles: MySQL, Redis, ClickHouse 26.3, ZooKeeper, MinIO, Java and Python backends, guardrails, frontend, init jobs | **Next candidate after Laminar.** Its schema is clustered (`ON CLUSTER` 82 times, `ReplicatedReplacingMergeTree`), with `Map`, `JSON` and the `Time` type: it reuses SigNoz's cluster-DDL work over HTTP. 22.8k stars |

### APM and observability

| App | Repo | Licence | Latest release | ClickHouse | Protocol | Stack | Fit for Loams |
|---|---|---|---|---|---|---|---|
| Coroot | `coroot/coroot` | Apache-2.0. A paid Enterprise Edition image (`coroot-ee`) exists; its code is not in the repository (verify) | v1.28.1, 2026-10-21 | Required for traces, logs and profiles; metrics in Prometheus by default | Both, native by default (`protocol` setting) | 5: coroot, node agent, cluster agent, Prometheus, ClickHouse 24.3 | Good. Runs over HTTP by configuration |
| Uptrace | `uptrace/uptrace` | **AGPL-3.0** | Stable v2.0.3 (2026-04-30); v2.1.0-rc.1 (2026-10-07) is a pre-release | Required (Postgres for metadata) | **Native only** (its own `ch` client) | 9: ClickHouse, Postgres, collector, Alertmanager, Vector, Keycloak, Grafana, Prometheus, Mailpit | Native test case; `AggregatingMergeTree` views and `SimpleAggregateFunction` |
| gigapipe (was qryn) | `metrico/gigapipe` (renamed from `metrico/qryn`) | **AGPL-3.0** | v5.5.3, 2026-09-30 | Primary store | Both; the code defaults to native, the docs say HTTP (verify) | No root compose; e2e compose has the app and 2 ClickHouse nodes | Fair. Distributed and `Replicated*` tables, aggregate states |
| Jaeger v2 (built-in ClickHouse) | `jaegertracing/jaeger` | Apache-2.0 | v2.22.0, 2026-10-06 | Optional (one of several stores) | Both, native by default | Jaeger plus ClickHouse (and Keeper in some files) | Good. The storage feature gate is stable; the schema is still called alpha in ADR 008 (verify) |
| jaeger-clickhouse plugin | `jaegertracing/jaeger-clickhouse` | Apache-2.0 | 0.13.0, 2022-11-08 | — | Native | — | **Skip: dead.** README says it uses Jaeger's deprecated gRPC plugin API and points to Jaeger v2's built-in support |

### Metering and billing

| App | Repo | Licence | Latest release | ClickHouse | Protocol | Stack | Fit for Loams |
|---|---|---|---|---|---|---|---|
| OpenMeter | `openmeterio/openmeter` (Kong since 2025-09) | Apache-2.0, no split | v1.0.0-beta.236, 2026-10-09 (weekly; still "beta" tags, not marked pre-release) | Required for usage events and meter queries; Postgres for entitlements, billing and the catalogue | **Native only** (`clickhouse-go` v2 with no protocol option; default address `127.0.0.1:9000`) | 6 OpenMeter processes (server, sink worker, balance worker, notification service, billing worker, jobs) plus Kafka, ClickHouse, Postgres, Redis (dedupe) and Svix (webhooks, optional) | **Good, and it dogfoods Loams Meter (§63).** Queued as Task 13 |
| Lago | `getlago/lago` (+ `lago-api`, `lago-front`) | **AGPL-3.0** | v1.55.0, 2026-10-07 | **Optional**: Postgres by default, ClickHouse with `LAGO_CLICKHOUSE_ENABLED` | HTTP (`clickhouse-activerecord`) | 8 by default without ClickHouse; the ClickHouse path is only in the dev compose (about 30 services, Redpanda) | Weak. Needs the **Kafka table engine** and aggregate states |
| Flexprice | `flexprice/flexprice` | **AGPL-3.0** | v2.1.40, 2026-10-17 | Required | Native (`clickhouse-go`) | 10: Postgres, Kafka, ClickHouse, Redis, Temporal, Temporal UI, Kafka UI, api, consumer, worker | Later; native |

### Product analytics, events and messaging

| App | Repo | Licence | Latest release | ClickHouse | Protocol | Stack | Fit for Loams |
|---|---|---|---|---|---|---|---|
| Dittofeed | `dittofeed/dittofeed` | MIT. Multi-tenancy is a closed-source `dittofeed-ee` image (`docker-compose.ee.yaml`), not code in the repository | v0.23.0, 2025-12-01; v0.24.0 alphas since | Required (Postgres for config) | HTTP (`@clickhouse/client`) | 7 in the "lite" file: lite app, admin CLI, Temporal, Temporal UI, Postgres, ClickHouse, MinIO | Good. Heavy `JSONExtract*` and materialized views |
| Rybbit | `rybbit-io/rybbit` | **AGPL-3.0** | v2.9.0, 2026-09-12 (tag v2.9.1 newer) | Required | HTTP (`@clickhouse/client`) | 6: Caddy, ClickHouse 26.3, Postgres, Redis, backend, client | Good. Needs the new `JSON` type, `Map`, TTL, `SummingMergeTree` and `AggregatingMergeTree` views. 13k stars |
| Jitsu | `jitsucom/jitsu` (branch `newjitsu`) | MIT, no split | Stable 2.14.0, 2026-07-09; near-daily pre-releases | ClickHouse is one destination and one events-log backend; whether the stack boots without it is not confirmed (verify) | Both (bulker: native by default, HTTP by setting; events log: HTTP) | 13 in a compose file marked "for exploration only" (MongoDB, Keycloak, Redpanda, …) | Weak. ClickHouse is not the centre |
| Trench | `FrigadeHQ/trench` | MIT | Not archived. Last app release none since 2024-12; last commit 2026-04-06 (a security fix); no deprecation notice | Required | HTTP | 3: api, ClickHouse 24.8, Kafka | **Skip.** Low activity, and it needs the Kafka table engine |
| Swetrix | `Swetrix/swetrix` | **AGPL-3.0** | v5.4.1 tag; tracker 4.8.0, 2026-10-20 | Required (MySQL for accounts) | HTTP | MySQL, ClickHouse, Redis, backend (no root compose found, verify) | Small; later |

### Network and security

| App | Repo | Licence | Latest release | ClickHouse | Protocol | Stack | Fit for Loams |
|---|---|---|---|---|---|---|---|
| Akvorado | `akvorado/akvorado` | **AGPL-3.0** | v2026.10.0, 2026-10-03 | Required | **Native only** | 9: Kafka, Kafka UI, Redis, orchestrator, console, inlet, outlet, ClickHouse 26.8, Traefik | Hard. Dictionaries loaded over HTTP from the orchestrator, `ON CLUSTER`, `Replicated*`, `Distributed`, TTL |
| RITA v5 | `activecm/rita` | **GPL-3.0** | v5.1.2, 2026-05-07 | Required | **Native only** | 3: rita, syslog-ng, ClickHouse | Small native test case; aggregate states, views, TTL, `IPv6` |

### BI tools

| Tool | Repo | Licence | Latest release | Protocol | Fit for Loams |
|---|---|---|---|---|---|
| Grafana ClickHouse data source | `grafana/clickhouse-datasource` | Apache-2.0 (the plugin). **Grafana itself is AGPL-3.0**: run it unmodified | v4.23.1, 2026-10-21 | **Both**: `clickhouse-go` v2, native by default, HTTP by the `protocol` setting | **BI gate** (Task 15) |
| Metabase ClickHouse driver | Now in Metabase core, `metabase/metabase` `modules/drivers/clickhouse` (Apache-2.0 code inside an **AGPL-3.0** repository; top-level `enterprise/` is under the Metabase Commercial License). The old `ClickHouse/metabase-clickhouse-driver` is **archived** (moved to core in Metabase 54) | v0.64.1, 2026-10-07 | HTTP (`clickhouse-jdbc` 0.9.8) | **BI gate** (Task 15). Use the AGPL image `metabase/metabase`, never `metabase/metabase-enterprise` |
| Superset with clickhouse-connect | `apache/superset`, `ClickHouse/clickhouse-connect` | Apache-2.0 both | Superset 6.1.0, 2026-05-13; clickhouse-connect v1.11.0, 2026-10-16 | HTTP | Optional in the BI gate |
| Redash | `getredash/redash` | BSD-2-Clause | v26.9.0, 2026-09-24 | HTTP (`requests` to 8123) | Optional in the BI gate. Schema browse is one `system.columns` read |

### Other notable applications found

| App | Repo | Licence | Latest release | ClickHouse | Protocol | Note |
|---|---|---|---|---|---|---|
| Trigger.dev | `triggerdotdev/trigger.dev` | Apache-2.0 | v4.7.6, 2026-10-20 | In the self-host compose (ClickHouse 26.2); the setting is optional in code (verify) | HTTP | 16.6k stars; about 10 services |
| LangWatch | `langwatch/langwatch` | Apache-2.0, except `enterprise/` (commercial) | 3.21.0, 2026-10-21 | Required (".env.example: Required") | HTTP | LLM observability; stack not counted (verify) |
| OpenReplay | `openreplay/openreplay` | AGPL-3.0, some MIT directories, `ee/` under its own licence | v1.28.0, 2025-09-25 (repository active) | Main analytics store (verify) | Both | Many services plus Kafka |
| Unkey | `unkeyed/unkey` | **FSL-1.1-ALv2** (not open source until each version converts; treated as BSL, D1574) | Per-component tags, 2026-10-21 | Required for analytics | Both | Skip: FSL |
| Sentry / Snuba | `getsentry/snuba` | **FSL-1.1-Apache-2.0** | 26.10.0, 2026-10-15 | Required | HTTP (`clickhouse-connect`) | Skip: FSL (as BSL, D1574) and the Sentry stack |
| AgentOps | `AgentOps-AI/agentops` | Root MIT, but **`app/` is Elastic License 2.0** | 0.4.21, 2025-08-29 | Required in `app/` | Both | Skip: ELv2 and stale |
| Meteroid | `meteroid-oss/meteroid` | AGPL-3.0 | Not checked | On by default (Cargo feature) | HTTP | Metering and billing |
| Langtrace | `Scale3-Labs/langtrace` | AGPL-3.0 | 4.0.11, 2025-04-17 | Required | HTTP | Skip: stale |
| ClickVisual | `clickvisual/clickvisual` | MIT | v1.1.2, 2026-09-03 | Required | Native | Log viewer |
| Infisical | `Infisical/infisical` | MIT, except `ee/` | — | Optional (audit logs) | HTTP | Not a main store |

## 2. Skip list

- **PostHog**: about 39 services, no versions, self-hosting "officially unsupported". If ever needed, use the MIT-only `posthog-foss` mirror.
- **jaeger-clickhouse plugin**: dead since 2022; Jaeger v2's built-in store replaces it.
- **Trench**: no release since 2024, and the Kafka table engine is the only ingest path.
- **Unkey, Sentry/Snuba**: FSL is source-available, not open source, until each version converts after two years. **FSL is treated like BSL under D11 (D1574, owner 2026-10-22)**: its code is never copied or ported, and it runs only where BSL would be allowed.
- **AgentOps**: `app/` is ELv2 (banned, D11), and stale.
- **RudderStack** (R0.6): ELv2, and ClickHouse is only a destination.
- **Highlight** (R0.6): stale releases.
- **Langtrace**: no release since 2025-04.
- **Lago as a gate**: ClickHouse is optional and needs the Kafka engine; it stays a Loams Meter export target (§63 D1130), over HTTP only.

## 3. Licence warnings

1. **Plausible's `extra/` is proprietary, not AGPL.** Only the community-edition image (built with `MIX_ENV=ce`, without `extra/`) may be run.
2. **Langfuse is now owned by ClickHouse, Inc.** The core stays MIT and the `ee/` split is unchanged; watch for licence changes on new releases.
3. **SigNoz, PostHog and Langfuse ship `ee/` code in their default images.** SigNoz has a clean MIT image (`signoz/signoz-community`, D1571); PostHog has `posthog-foss`; Langfuse's `ee/` code is in its one image but gated by a licence key (verify that nothing in `ee/` runs without a key).
4. **Metabase**: the repository is AGPL-3.0 with a commercial `enterprise/` directory; only `metabase/metabase` (AGPL) may be used. **Grafana** is AGPL-3.0. Both are run as unmodified containers, never linked or shipped (D1566).
5. **FSL** (Unkey, Snuba) is treated like BSL (D1574, amending D11): never copied or ported, run only where BSL would be allowed. **ELv2** (AgentOps `app/`, RudderStack) is banned by D11. Both are out as gate applications.
6. **Closed EE images next to open repositories**: Dittofeed (`dittofeed-ee`), Laminar (`*-ee` images), Coroot (`coroot-ee`). Use only the open images.
7. **AGPL applications** (OpenPanel, Plausible, Uptrace, gigapipe, Lago, Rybbit, Akvorado, Flexprice, Swetrix) and GPL-3.0 RITA run only as unmodified containers (§75 §18).
8. Supporting images: OpenMeter's compose file uses Redis 7.4 (source-available, not OSI) and Confluent's `cp-kafka` image; the harness swaps in Valkey (as R0.4) and an Apache Kafka image (verify that OpenMeter works with it).

## 4. Recommendations

1. **Swap Helicone for Laminar as the LLM and agent gate** (Opik next). *Approved by the owner 2026-10-22 (R14.2).* Helicone is Apache-2.0, but its public images stopped on 2025-08-21, development moved to a private repository that is synced in bulk, the production chart is on request, and its ownership is unsettled. A gate would test a 14-month-old build or need a source build we may not make (R0.7). Laminar is Apache-2.0, HTTP-only, 5 services, released 2026-10-16, and adds the Rust `clickhouse` crate's `RowBinary` traffic. Opik is the more popular choice (22.8k stars) and exercises clustered DDL over HTTP, but its stack is about three times larger.
2. **Gate order after SigNoz: the BI tools first, then OpenMeter, then the LLM gate.** *Approved by the owner 2026-10-22 (R13.2, R14.2, R15.2).* The BI gate needs no application stack, covers the `system.*` reads every House user's tools make, and runs over both protocols once SigNoz's native listener exists. OpenMeter is native-only, so it waits for that listener anyway, and its value grows when Loams Meter's ME1b windows exist to compare against.
3. **Native-protocol test cases**, after the native listener: RITA (3 services) and Coroot (native by default) are cheap; Uptrace and Akvorado are the hard ones.
4. **Mining-only candidates** (Task 5 style, not gates): Dittofeed, Rybbit, Laminar or Opik (whichever is not the gate), Coroot, Jaeger v2.
5. **Two features many of these applications need that the House refuses today**: the Kafka table engine (Lago, Trench) and dictionaries (Akvorado). Neither is in §75's Tier 0; record them as Tier 2 demand in the mining report.
