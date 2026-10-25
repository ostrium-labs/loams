# 04 — Hosted Loams Cloud on Cloudflare, by BYOC into Loam's Own Account

> **Imported on 2026-10-19.** Written in the former private `ostrium-labs/loams-platform` repository; open source (Apache-2.0) in the monorepo since the owner's order of 2026-10-19. "This repository", "proprietary" and "private" below describe that repository and the open-core boundary of the time, which no longer holds; see [the platform docs index](../README.md).

Status: **Approved** (owner defaults, 2026-10-02: "do suggested for all") · 2026-10-02. Proprietary. Source: the owner's rulings of 2026-10-01, "all run on cloudflare using byoc", and of 2026-10-02, to move the Cloudflare and OpenRTB designs out of operon. This document says where every part of the hosted Loams Cloud runs. Loam Cloud deploys into **Loam's own Cloudflare account** (the domain `loams.dev` is there) exactly as a BYOC customer would deploy into theirs, and customers can BYOC into their own Cloudflare accounts. It builds on doc 03 (the Cloudflare target, PD20–PD25) and doc 01 (metering and billing). The engine it hosts is open source in operon; operon never depends on this document (operon D220, D440).

Numbering: decisions **PD29–PD44**, questions **Q-HC-1–Q-HC-12**. Bare D-numbers and "operon §NN" point into operon's `docs/design`. Markers as in operon: **(verify)** means not checked against a primary source; **(estimate)** means computed, not measured. Cloudflare facts were read on 2026-10-02 at the pages in §12, which carry their own "last updated" dates.

---

## 1. Summary

| # | Decision | Status |
|---|---|---|
| PD29 | **The verdict: "all on Cloudflare" means Cloudflare is the edge, the front end, the object store and the stateless compute, not the whole system.** Cloudflare offers no managed Kubernetes, Containers have no persistent disk and are reached only through Workers and Durable Objects, and custom inbound TCP needs Enterprise Spectrum. TiKV, PD, Iggy, Fluss, Postgres (Neon) and WeSQL need real nodes with disks. They run on a **small k3s cluster** of rented or owned machines, connected to Cloudflare by **Cloudflare Tunnel**. No public inbound port is open except the TLS wire-protocol ports of PD35 (Postgres, MySQL, Kafka, Iggy) on the ingress nodes; all HTTP enters through Cloudflare (§2) | Approved (owner defaults, 2026-10-02) |
| PD30 | **R2 is the hosted bucket of truth** (operon D1, D381). Engine nodes on k3s read and write R2 with vended prefix credentials. R2's free egress pays for the distance; the NVMe hot tier (operon §04) hides the latency. RustFS is not run in the hosted cloud | Approved (owner defaults, 2026-10-02) |
| PD31 | **The k3s cluster runs the open-source distribution unmodified**: operon's umbrella chart, `loam-operator`, Argo CD, Knative (operon §38), Zitadel and OpenFGA (operon §76), TiKV, and the engine roles. The hosted cloud is a BYOC install of operon into Loam's own infrastructure plus this repository's components (`loam-meter`, `loam-usage`, the control plane). "Hosted" adds operation, never a fork | Approved (owner defaults, 2026-10-02) |
| PD32 | **Workers front everything public**: `loams.dev` (the site, from `loam-cloud`), `console.loams.dev`, `api.loams.dev` and the protocol endpoints. A front Worker terminates TLS at the edge, authenticates, rate-limits per tenant, and forwards to the k3s gateway through a **Workers VPC binding** to the Tunnel (public beta since April 2026), or through a Tunnel public hostname until VPC bindings are generally available | Approved (owner defaults, 2026-10-02) |
| PD33 | **Tenant functions on the hosted cloud:** `fetch` functions run on **Workers for Platforms**, in a dispatch namespace in untrusted mode (answers Q-CF-3 for the hosted cloud: yes, on Loam's account, only in WfP). `http-port` functions run on **Knative on k3s** (operon D441), with **Cloudflare Containers** as the burst and edge option behind a Durable Object (PD21's native fallback). The supervisor's T0/T1 run on k3s for functions that must sit next to the data (operon D170) | Approved (owner defaults, 2026-10-02) |
| PD34 | **Stateful services stay on k3s nodes with local NVMe**: PD and TiKV (the metastore and Live), Resonate on TiKV, Iggy and Fluss (the Event Fabric, operon §32), Neon (operon §28), WeSQL (operon §29), and the engine's query and log roles. Durable Objects hold only caches and leases (operon D382, PD20) | Approved (owner defaults, 2026-10-02) |
| PD35 | **Wire protocols that are not HTTP** (Postgres, MySQL, Kafka, Iggy's TCP and QUIC) are **not proxied by Cloudflare** in the beta. Spectrum for custom TCP needs an Enterprise plan. They are exposed from the k3s ingress nodes' own addresses with TLS and Loam's tokens. Workers reach Postgres through **Hyperdrive** (included in Workers Paid) over the Tunnel **(verify the private-database path)** | Approved (owner defaults, 2026-10-02) |
| PD36 | **Customer BYOC on Cloudflare has two parts**, matching operon D64. **Edge BYOC:** Loam deploys the front Worker, the WfP namespace and R2 buckets into the customer's Cloudflare account with a scoped API token. **Data BYOC:** the stateful part runs in the customer's Kubernetes (BYOC-local-meta) or Loam's k3s (BYOC-managed-meta), joined by the customer's Tunnel. The management agent is pull-based and never needs inbound access (operon §18 §8) | Approved (owner defaults, 2026-10-02) |
| PD37 | **Identity for the hosted cloud is a self-hosted open-source identity provider, not Clerk**, following the owner's 2026-10-01 ruling for auth with no paid plan, for the hosted service as well. Confirmed by the owner on 2026-10-02 (Q-HC-1); `loam-cloud`'s console moves off Clerk before the hosted beta; hosted multi-org tenancy uses Loam's own orgs (operon §19). **Amended by operon D1600** (2026-10-21): the provider is **Zitadel** at `auth.loams.dev`, unmodified, with OpenFGA for authorization, and the earlier provider is removed with no migration (operon §76) | Approved (owner defaults, 2026-10-02) · owner decision; provider amended by D1600 |
| PD38 | **Billing is Stripe directly from the beta** (PD37 confirmed; Q-HC-2). Clerk Billing (PD10's beta phase) goes away with Clerk; doc 01's Stripe path (PD7, PD10's GA phase) starts at beta instead. Stripe is a payment service with per-transaction fees, not a software licence | Approved (owner defaults, 2026-10-02) · owner decision |
| PD39 | **Metering on Cloudflare** (doc 01, PD23): the Tail Worker turns WfP and front-Worker trace events into host reports for `loam-usage`. Cloudflare's own CPU-ms figures from its analytics API reconcile them daily (PD26). On k3s, `loam-meter` reads cgroups as doc 01 describes, Knative pods included (PD28) | Approved (owner defaults, 2026-10-02) |
| PD40 | **Regions:** one k3s cluster in one region for the beta (three nodes, one failure domain each, TiKV replication 3), plus R2 location hints. Multi-region is a later fleet decision (Q-HC-6) | Approved (owner defaults, 2026-10-02) |
| PD41 | **Secrets and keys** are in Cloudflare Secrets Store for Workers and in the k3s cluster through Dapr secret stores (operon D189) backed by OpenBao. Neither holds the other's. Tunnel tokens are per cluster | Approved (owner defaults, 2026-10-02) |
| PD42 | **The enterprise tier sells operation, not withheld code.** With the engine open source (operon D220, no SSO tax), the enterprise tier is the SLA, support, the hosted service and BYOC management, compliance packs and certification. Appendix Z is a non-binding joke about where that could go | Approved (owner defaults, 2026-10-02); **superseded by D1323** (2026-10-21: no services offering, no SLA, no paid support; no enterprise tier) |
| PD43 | **A second region is a second k3s cluster with its own TiKV**, after the beta (Q-HC-6, refines PD40): no Raft group spans regions; R2 location hints per cluster; the control plane routes an org to one region | Approved (owner defaults, 2026-10-02) |
| PD44 | **One Loam CPU-ms rate for every runner** (Q-HC-10): WfP, Knative, the supervisor's tiers and external runners bill the same rate; reconciliation (PD26) still compares provider figures | Approved (owner defaults, 2026-10-02) |

## 2. What "all on Cloudflare" can and cannot mean today

Checked on 2026-10-02:

| Need | Cloudflare today | Evidence | Consequence |
|---|---|---|---|
| Kubernetes for Knative and the operators | **No managed Kubernetes** | No Kubernetes product in Cloudflare's developer platform docs; Containers are Durable-Object-driven instances, not pods | Knative runs on a k3s cluster Loam operates (PD29, PD31) |
| Containers for stateless services | **GA since 2026-04-13**; instance types up to `standard-4` (4 vCPU, 12 GiB, 20 GB disk), custom types up to 4 vCPU and 12 GiB with at least 3 GiB per vCPU; per account 1,500 vCPU, 6 TiB memory and 30 TB disk concurrently | Containers changelog 2026-04-13 ("Containers and Sandboxes are now generally available"); Containers limits page (last updated 2026-09-30) | Good for build steps, repacks, `http-port` bursts and sandboxes; too small and too ephemeral for TiKV or Neon |
| Persistent disk | **None.** "All disk is ephemeral by default … the next time it starts, it uses a fresh disk from the container image." Snapshots up to 20 GB, kept 30 days | Containers FAQ; limits page | No stateful store in Containers (PD34) |
| How Containers are reached | Through a Durable Object binding from a Worker (`env.MY_CONTAINER.get(id)`); no direct inbound from the internet is documented | Containers FAQ | Containers sit behind Workers, like everything else |
| Cold start | "often … in the 1–3 second range", image-dependent | Containers FAQ | Fine for bursts and builds, not for the query path |
| Container pricing | $0.000020 per vCPU-second, $0.0000025 per GiB-second, $0.00000007 per GB-second of disk, billed on active CPU; Workers Paid includes 375 vCPU-minutes, 25 GiB-hours and 200 GB-hours a month; egress $0.025/GB in North America and Europe with 1 TB included | Containers pricing (last updated 2026-08-28) | One `standard-2` (1 vCPU, 6 GiB) running flat out for a 30-day month costs about $52 in CPU plus $39 in memory **(estimate)**, before inclusions |
| Custom TCP inbound | Spectrum is on paid plans, but "Custom TCP/UDP applications require an Enterprise plan with Spectrum as a paid add-on" | Spectrum overview (last updated 2026-04-23) | Postgres, MySQL and Kafka wire go direct to k3s ingress (PD35) |
| Workers to a private network | **Workers VPC** network bindings, public beta in April 2026: `fetch()` and raw TCP `connect()` to any service behind a Tunnel or Cloudflare Mesh | Workers VPC changelog 2026-04-14 and 2026-05-21 | The front Worker reaches the k3s gateway privately (PD32) |
| Tunnel | `cloudflared`, Apache-2.0, release 2026.9.3 (2026-09-24); no charge for Tunnel itself **(verify current plan terms)** | GitHub `cloudflare/cloudflared` | The only path between Cloudflare and k3s |
| Object storage | R2: $0.015/GB-month, Class A $4.50/M, Class B $0.36/M, **free egress**; Infrequent Access $0.01/GB-month with $0.01/GB retrieval; conditional writes; one write per second per key | R2 pricing (read 2026-10-02); doc 03 §6 | The hosted bucket (PD30); WAL segment keys are unique, so the per-key write limit does not bind |
| Tenant code | Workers for Platforms: $25/month with 20 M requests, 60 M CPU-ms and 1,000 scripts included; then $0.30/M requests, $0.02/M CPU-ms, $0.02 per script | WfP pricing (last updated 2026-04-21) | CPU-ms billing matches operon's CPU-time premise (D180), so tenant `fetch` functions run there (PD33) |
| SQL at the edge | D1: 10 GB per database, 50,000 databases and 1 TB per account (Workers Paid) | D1 limits (last updated 2026-04-21) | Console session and edge config caches only; never tenant data |
| Postgres from Workers | Hyperdrive pooling and caching included in Workers Paid; unlimited queries on paid | Hyperdrive pricing (last updated 2026-06-18) | Workers that need SQL use Hyperdrive to Neon on k3s |

**So, plainly:** everything that is a front door, a stateless request handler, a tenant `fetch` function, a burst container or a blob can be on Cloudflare today. Everything that must keep data on a disk across restarts (a mounted or persistent volume, memory or a running process), or that needs a Kubernetes API, cannot. A container's writable root filesystem can be saved and restored from a snapshot (beta, 2026-09-30, §2a), which is a workspace, not a persistent disk. The smallest honest version of "all on Cloudflare" is **Cloudflare plus three rented machines behind a Tunnel**, with nothing reachable on those machines except through Cloudflare (and the non-HTTP wire ports of PD35).

## 2a. Update of 2026-10-02: what the Birthday Week 2026 betas change

Read on Cloudflare's own pages on 2026-10-02; details and sources in doc 03 §12a. Open adapters are in operon §42.

- **Containers are faster and can pause.** Median start is 648 ms (Cloudflare's figure, 2026-09-30), so the "1 to 3 s" cold start row above is out of date. Filesystem snapshots (**beta**) allow a workspace to be saved and restored, so the "persistent disk: none" row stays true for a running container, while a *workspace* can now survive between runs (PD74).
- **Hosted Functions.** `http-port` functions can use the `durable_object` scheduling policy (**beta**) to choose image and instance type per function (PD73). Knative on k3s stays the default (PD33).
- **Untrusted code.** Code generated by agents at run time runs in **Dynamic Workers** (**open beta**), with a database per generated app through **Durable Object Facets** (**open beta**) (PD76). Workers for Platforms stays for tenant-deployed `fetch` functions.
- **Hosted repositories.** Artifacts (**open beta**) hosts agent-workspace repositories and Loams Git mirrors, one namespace per org, in the org's US or EU jurisdiction (PD78). Limits to design around: 1 GB per repository, 32 MB per blob.
- **Analytics.** The Data Platform is the hosted analytics path (PD79): Pipelines for ingest, the R2 Data Catalog for Iceberg tables, and R2 SQL for queries. Cloudflare's [Basin announcement](https://blog.cloudflare.com/cloudflare-basin/) of 2026-10-01 (verified 2026-10-02) made the Data Platform **Basin, generally available**: Pipelines is now Basin Pipelines, R2 Data Catalog is Basin Catalog (a managed Iceberg REST catalog) and R2 SQL is Basin SQL; pricing is usage-based with no egress fees, and PyIceberg, DuckDB, Snowflake and Spark can read and write. Loams reads these tables through its external-REST-catalog adapter (operon §42 D571).
- **Deprecation.** The `Container` and `Sandbox` classes are maintained only through **2026-12-31** (PD75). The PD21 native fallback and anything else that names them moves to `this.ctx.container`.

| # | Decision | Status |
|---|---|---|
| PD79 | **Hosted analytics runs on Cloudflare's Data Platform** (Basin Pipelines, Basin Catalog, Basin SQL) for product analytics and usage rollups of the hosted cloud, with Iceberg tables Loams can also read. Billing-grade usage still comes from `loam-usage` (doc 01), never from this path | Proposed |

| # | Question | Owner | Needed by |
|---|---|---|---|
| Q-HC-13 | Basin's per-unit prices, catalog REST endpoint and token scopes, and whether Loams commits pass its Iceberg conformance (the rename and GA are confirmed) | Eng | Before PD79 is built |
| Q-HC-14 | Which regions and jurisdictions do Artifacts namespaces and Containers snapshots support, per org residency (GDPR, operon D-series) | Eng | Hosted beta |

## 3. Placement map

| Component | Where | Why |
|---|---|---|
| Site, blog, docs (`loam-cloud`) | Workers static assets (OpenNext) | Already Next.js; operon §24's preset path |
| Console (`console.loams.dev`) | Workers + the console API on k3s | The SPA is static; its API is the engine's (operon §19 P9) |
| Front Worker for `api.loams.dev` and protocol HTTP endpoints | Worker | TLS, WAF, per-tenant rate limits, token check (operon §38 D449 tokens), routing |
| Go vanity imports (`loams.dev/...`) | A tiny Worker answering `?go-get=1` | Static per module path |
| Tenant `fetch` functions | Workers for Platforms, untrusted mode (PD22, PD33) | CPU-ms billing; isolation by Cloudflare |
| Tenant `http-port` functions | Knative on k3s (default); Cloudflare Containers for burst (PD33) | Placement next to data by default |
| Git front end, sequencers, crates mirror, build cache | Workers and Durable Objects (doc 03 §3; operon §36) | Already designed for it |
| Repacks, builds, large compression | Cloudflare Containers (PD24) | Bursty CPU, no state |
| Async event mirror | Queues → `ProduceCloudEvents` over the Tunnel | The narrow waist (operon D363) |
| Engine query, log and hot tier | k3s nodes, NVMe | Threads, NVMe, gigabytes of RAM |
| PD, TiKV, Resonate, Live | k3s | Raft on local disks |
| Iggy, Fluss | k3s | Disks; Fluss needs ZooKeeper and the Flink tiering job (operon §32) |
| Neon (pageserver, computes), WeSQL | k3s | Disks and long-lived processes |
| Zitadel, OpenFGA, CloudNativePG | k3s | Small stateful services (operon §76 §12.2) |
| Bucket | R2 (PD30) | Free egress; S3 API |
| `loam-meter`, `loam-usage`, control plane | k3s (doc 01) | Reads cgroups; writes the `ControlStore` |
| Secrets | Secrets Store (Workers), OpenBao via Dapr (k3s) (PD41) | Each side has its own |

```
 users, agents, SDKs ──HTTPS──► Cloudflare edge ── front Worker (auth, limits) ──┐
                                   │  WfP dispatch → tenant fetch functions      │ Workers VPC binding
                                   │  Durable Objects → Containers (bursts)      │ (or Tunnel hostname)
                                   │  R2 (bucket of truth)  Queues  Hyperdrive   │
                                   └──────────────────────────────────────────────┤
                                                                     cloudflared ◄┘ (outbound only)
  Postgres / MySQL / Kafka wire ──TLS──►  k3s ingress nodes (3 machines, NVMe)
                                          Envoy → loam-gateway → engine roles, Knative, Live
                                          PD/TiKV · Resonate · Iggy/Fluss · Neon · WeSQL
                                          Zitadel · OpenFGA · CNPG · loam-meter · loam-usage
```

## 4. Tenancy in the hosted cloud

- **Orgs and namespaces** are operon's (§19 P2): an org has projects, a project has environments, an environment is one namespace. The hosted control plane is this repository's (doc 01's `loam-usage`, plus provisioning).
- **Isolation per layer:** R2 prefixes `ns/<ns>/` with vended credentials (D381); TiKV keyspaces with tenant prefixes (operon D374); a Kubernetes namespace per Loam namespace for Knative (operon D443); a WfP user Worker per function, tagged with the org and namespace for per-tenant limits; OpenFGA on every data call (operon D66).
- **Quotas are set here, enforced by operon** (operon D220): the plan sets the limits record; the engine and the operator enforce it; WfP's per-script limits are set from the same record.

## 5. Knative on k3s, and what Cloudflare replaces

Knative needs Kubernetes, so it lives on k3s (PD31). Cloudflare covers the parts of a serverless platform that Knative would otherwise do at the edge: TLS, global routing, request-level scale for `fetch` functions (WfP), and burst containers (Containers). Knative keeps `http-port` functions next to the data with scale to zero (operon D441). Knative Eventing inside the cluster is an adapter to Loam streams (operon D445); Cloudflare Queues bridge edge events into the same streams.

## 6. Customer BYOC into their own Cloudflare account (PD36)

| Customer has | Loam deploys | Data plane | Meta mode |
|---|---|---|---|
| A Cloudflare account only | Front Worker, WfP namespace, R2 buckets, Queues in the customer's account (scoped API token, Workers Scripts, R2 and Queues edit only) | Loam's k3s, the customer's R2 as their bucket | BYOC-managed-meta (D64) |
| Cloudflare and Kubernetes | The same, plus operon's chart in their cluster through their Argo CD, joined by their Tunnel | Their cluster | BYOC-local-meta (D64) |
| Kubernetes only | Operon's chart; no Cloudflare | Their cluster | Either |

The management agent is pull-based in every case. The customer's Cloudflare bill carries their Workers, WfP and R2 usage; Loam's invoice carries the subscription and anything that runs on Loam's k3s.

## 7. Costs (estimate)

Fixed at beta scale: Workers Paid $5/month; WfP $25/month; R2 at 1 TB about $15/month plus operations; Containers within inclusions for spikes and builds; Tunnel at no charge **(verify)**; three rented machines with NVMe (64 to 128 GB RAM each) in one region, at hosting-provider list prices **(verify at order time; not checked here)**. Doc 03 §8's startup credits (PD25) offset the Cloudflare part for a year once the first workload ships. The machines are the dominant cost, and they are the part credits do not cover.

## 8. Risks

| Risk | Mitigation |
|---|---|
| Workers VPC stays in beta | Tunnel public hostnames with Access service tokens as the fallback (PD32) |
| R2 latency from k3s on the query path | The NVMe hot tier and prefetch (operon §04); place the k3s region near the R2 location hint; measure in CF1 (doc 03) |
| One k3s region is one failure domain | TiKV replication 3 across machines; R2 is the durable truth, so the cluster can be rebuilt from the bucket (operon §10) |
| Spectrum is needed for non-HTTP wire later | Direct TLS ingress now (PD35); Enterprise Spectrum when the hosted load needs it |
| A self-hosted IdP (Zitadel since D1600) replaces Clerk in a console already built on Clerk | Decided 2026-10-02 (Q-HC-1); `loam-cloud`'s auth is behind OIDC, so the swap is a provider change plus the sign-in pages |
| Dependence on one vendor's edge | Every Cloudflare piece has an operon fallback (doc 03 §3's last column); the k3s cluster runs without Cloudflare except for the public entry |

## 9. Open questions

| # | Question | Owner | Needed by |
|---|---|---|---|
| Q-HC-1 | ~~Replace Clerk with a self-hosted open-source IdP for the hosted console now (PD37), or keep Clerk until GA~~ Answered 2026-10-02 by the owner: the recommended default — a self-hosted IdP now, open-source edition, for the hosted console; Clerk retires before the hosted beta (PD37; operon D404's "no paid plan"). The provider is Zitadel since operon D1600 (2026-10-21) | Founder | Resolved |
| Q-HC-2 | ~~Move billing to Stripe directly at beta (PD38), dropping PD10's Clerk Billing phase~~ Answered 2026-10-02 by the owner: the recommended default — Stripe directly from the beta; PD10's Clerk Billing phase is dropped (PD38) | Founder | Resolved |
| Q-HC-3 | The k3s machines: rented bare metal (which provider, region) or owned hardware. Owner action pending (2026-10-02): choose, and rent or buy, the three k3s machines (provider and region) | Owner action | Before the beta cluster |
| Q-HC-4 | ~~Workers VPC (beta) or Tunnel public hostnames for the front Worker's path to k3s at launch~~ Answered 2026-10-02 by the owner: the recommended default — Tunnel public hostnames with Access service tokens at launch, Workers VPC bindings once they are GA (PD32, §8) | Eng | Resolved |
| Q-HC-5 | ~~Expose Postgres and MySQL wire directly from k3s (PD35), or wait for Enterprise Spectrum~~ Answered 2026-10-02 by the owner: the recommended default — expose Postgres and MySQL wire directly from the k3s ingress nodes with TLS and Loam's tokens; Enterprise Spectrum only when revenue justifies it (PD35, §8) | Founder | Resolved |
| Q-HC-6 | ~~When to add a second region, and whether it is a second k3s cluster with its own TiKV or a stretched one~~ Answered 2026-10-02 by the owner: the chosen default (the doc gives no recommendation) — after the beta, a second region is a second k3s cluster with its own TiKV, never a stretched one (PD43); why: Raft across regions puts WAN latency on every write | Founder | Resolved |
| Q-HC-7 | ~~Does Hyperdrive reach a database behind a Tunnel without a public hostname **(verify)**~~ Answered 2026-10-02 by the owner: the plan's default — not an owner decision; CF1 checks it; if Hyperdrive cannot reach a database behind a Tunnel, Workers use a Tunnel public hostname with Access (PD32's fallback) | Eng | Resolved |
| Q-HC-8 | ~~Customer Cloudflare BYOC: the exact API-token permission set and how rotation is driven~~ Answered 2026-10-02 by the owner: the chosen default (the doc gives no recommendation) — the narrowest account token the customer creates for Workers scripts, the dispatch namespace and the named R2 buckets, stored in Secrets Store, rotated by the customer on a 90-day reminder; the exact permission list is written with the first BYOC customer; why: Loam never holds a broad customer token | Eng | Resolved |
| Q-HC-9 | ~~Cloudflare Containers as a `RunnerKind::External` for `http-port` bursts in operon terms: here only, or a documented open runner~~ Answered 2026-10-02 by the owner: the chosen default (the doc gives no recommendation) — here only; why: a Cloudflare Containers runner is part of the commercial Cloudflare target (operon D403) | Founder | Resolved |
| Q-HC-10 | ~~Price CPU-ms on WfP functions at Cloudflare's rate plus margin, or at one Loam rate for every runner~~ Answered 2026-10-02 by the owner: the chosen default (the doc gives no recommendation) — one Loam CPU-ms rate for every runner (PD44); why: placement is Loam's choice, so the price should not depend on it, and the pricing page stays simple | Founder | Resolved |
| Q-HC-11 | ~~R2 Infrequent Access for cold namespaces: the age threshold and retrieval-cost guard~~ Answered 2026-10-02 by the owner: the chosen default (the doc gives no recommendation) — namespaces with no reads for 90 days move to Infrequent Access, and move back when a month's retrieval cost exceeds that month's storage saving; why: R2's retrieval fee makes IA lose money on data that is read | Eng | Resolved |
| Q-HC-12 | ~~Interplanetary enterprise tier (Appendix Z). **For fun; not on the roadmap**~~ Answered 2026-10-02 by the owner: the recommended default — not on the roadmap (Appendix Z stays a joke) | Founder | Resolved |

## 10. Cross-references

operon §04 (hot tier), §10 (operations, DR), §18 §8 (BYOC, D64), §19 (orgs, tokens), §24 (runtime), §27 (hooks), §28 (Neon), §29 (WeSQL), §32 (Event Fabric), §36 (Loam Git), §38 (Knative, GitOps; D440), §76 (identity and authorization: Zitadel and OpenFGA; D1600); docs 01 and 03 here.

## 11. Contradictions, and how they are resolved

| Earlier | Here | Resolution |
|---|---|---|
| PD10: Clerk Billing in beta, Stripe at GA | Stripe from beta | PD38 (Q-HC-2, confirmed 2026-10-02) |
| `loam-cloud`: Clerk for auth | A self-hosted IdP, Zitadel since D1600 | PD37 (Q-HC-1, confirmed 2026-10-02; provider amended by operon D1600) |
| Doc 03 PD20: the engine's data plane stays on Kubernetes runners | Same, on k3s | Consistent; this document names the cluster |
| Doc 03 Q-CF-3: tenant code on Loam's account | WfP untrusted mode only | Answered by PD33 for the hosted cloud |
| operon D178: RustFS is the default store | R2 in the hosted cloud | No conflict: RustFS stays operon's self-hosted default; D381 lists `r2` as a provider |

## 12. Sources

Read on 2026-10-02.

- Cloudflare Containers: developers.cloudflare.com/changelog/2026-04-13-containers-sandbox-ga; developers.cloudflare.com/containers/platform-details/limits (last updated 2026-09-30); developers.cloudflare.com/containers/pricing (last updated 2026-08-28); developers.cloudflare.com/containers/faq.
- Workers for Platforms pricing: developers.cloudflare.com/cloudflare-for-platforms/workers-for-platforms/platform/pricing (last updated 2026-04-21).
- R2 pricing: developers.cloudflare.com/r2/pricing.
- D1 limits: developers.cloudflare.com/d1/platform/limits (last updated 2026-04-21).
- Hyperdrive pricing: developers.cloudflare.com/hyperdrive/platform/pricing (last updated 2026-06-18).
- Spectrum: developers.cloudflare.com/spectrum (last updated 2026-04-23).
- Workers VPC: developers.cloudflare.com/changelog/post/2026-04-14-vpc-networks and …/2026-05-21-vpc-networks-cloudflare-wan; developers.cloudflare.com/workers-vpc.
- `cloudflare/cloudflared` (Apache-2.0, 2026.9.3, 2026-09-24); `k3s-io/k3s` (Apache-2.0, v1.37.1+k3s1, 2026-09-30).
- Doc 03 §6 and §13 for Workers, Durable Objects and R2 limits read on 2026-10-01.

---

## Appendix Z: Enterprise tier, interplanetary HA (non-binding)

**For fun; not on the roadmap** (Q-HC-12). The owner asked for "multi-planetary HA and rollover etc. for fun". The physics and the standards below are real; the product is not.

**The enterprise tier, seriously.** Because the engine is open source, the enterprise tier sells the operation: an SLA, support, the hosted service and BYOC management, compliance packs and certification. It never sells withheld code (PD42). *Superseded by D1323 (2026-10-21): the services offering is removed, so there is no SLA, paid support or enterprise tier.*

**The constraint.** Earth–Mars one-way light time runs from about 3 minutes at closest approach (about 55 million km) to about 22 minutes near superior conjunction (about 400 million km). Around every solar conjunction, once per synodic period of about 26 months, the Sun sits between the planets and mission operators observe a communications moratorium of about two weeks (NASA's Mars solar conjunction practice). A Raft round trip of 6 to 44 minutes is not a quorum anyone wants, and two weeks of silence is not a partition a quorum can wait out.

**So there is one quorum per planet and asynchronous replication between planets.**

- **Per planet:** a full Loams Cloud region (PD, TiKV with replication 3, the engine, the bucket) with synchronous Raft inside the planet only.
- **Between planets:** each planet's change feed (operon's CDC streams, D270 CloudEvents) is shipped as bundles over **Delay/Disruption-Tolerant Networking**: the Bundle Protocol version 7 (RFC 9171), secured by BPSec (RFC 9172), with the Licklider Transmission Protocol (RFC 5326) on the deep-space link. BPv7 dropped BPv6's custody transfer from the core protocol; **custody transfer** comes back through the bundle-in-bundle encapsulation work in the IETF DTN group **(verify the draft's current state)**. Each bundle is retained by a custodian until the next hop accepts it, so a conjunction delays replication but loses nothing.
- **Consistency tokens carry a planet epoch:** `(planet_id, planet_epoch, offsets…)`. A token from Mars is honoured on Earth only after Earth has applied Mars's feed up to those offsets. Until then the read waits or is redirected. Reads are **read-your-planet's-writes**; cross-planet reads are bounded-stale by light time plus queueing.
- **Conflicting writes** resolve in one of two ways. Either the data type is a CRDT (counters, sets, sequence CRDTs for Live documents, operon §20), or each namespace has a **planet of record**: writes made elsewhere are forwarded as bundles and acknowledged after a round trip of 6 to 44 minutes, which the API reports honestly as `202 Accepted (in transit)`.

**Rollover and failover.**

- **Rollover** is a planned handoff of a namespace's planet of record, scheduled into an orbital window when light time is short and the link budget is good. Its steps are drain, ship the final epoch, flip, and bump `planet_epoch`.
- **Failover during conjunction is impossible**, because nothing gets through. Before every conjunction the control plane runs a **pre-scheduled planned failover**: each namespace's planet of record moves to wherever its writers will be for the next two weeks, or it is frozen read-only on the far side.
- Unplanned failover between planets is an operator decision with a 22-minute feedback loop. The runbook says so in bold.

**Metering** is in CPU-seconds per **sol** (24 h 39 min 35.244 s, 88,775.244 s) for Mars regions and per day for Earth regions. Invoices are issued per planet, because a single invoice across planets would need a cross-planet quorum on the ledger.

**The SLA:** "five nines, adjusted for relativistic time dilation". Five nines is about 5.26 minutes of downtime a year. On the Martian surface, clocks run faster than on Earth's by about 477 microseconds per day on average, with a seasonal swing (Ashby and Patla, NIST, 2024 **(verify)**). That comes to about 0.17 seconds a year, which the SLA credits back with great ceremony and no measurable effect.
