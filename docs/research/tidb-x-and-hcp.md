# TiDB X, TiDB Cloud, the HashiCorp free tier, and compute for Loams Cloud

Research date: **2026-10-23**. It was asked for by the owner for [design 78](../design/78-loams-cloud-on-tidb-x.md) (Loams Cloud on TiDB X). It is research only: nobody signed up for an account, created anything on TiDB Cloud or HCP, or cloned a repository.

**Legend.**
- **[V]:** checked today on a primary page (vendor docs, pricing page, release notes, licence file or GitHub API). The URL is given.
- **[V-claim]:** the vendor states it on a primary page, but it is the vendor's own figure and we cannot check it independently (customer counts, savings).
- **[U]:** unverified. The page did not say, the only source was secondary, or two primary pages disagree. The text says which.

Every price is in USD unless marked and is as read on 2026-10-23. Prices change, and design 71's dated free-tier table (D1401) is where Loams keeps live numbers.

## 1. Summary

1. **TiDB X is real and runs Starter, Essential and Premium** [V]. Object storage is the source of truth for all data, and the SQL layer is stateless. PingCAP's CTO says that "TiKV becomes stateless" [V-claim]. The docs are more careful: TiKV still writes Raft logs to local disk first and keeps a local cache [V].
2. **TiDB Cloud has no general compute product** [V]. The nearest things:
   - **Data Service:** HTTP endpoints that run SQL only. It is in preview, and only for Starter on AWS.
   - **Lake's external Python UDF SDK.**
   - **TiDB Cloud Zero:** temporary databases for agents.

   None of them runs a container, an agent, or a Factory app. **Loams Cloud cannot run on TiDB services alone.** TiDB X can be its state layer, and compute has to come from elsewhere.
3. **What TiDB services can host for Loams:**
   - SQL state (Starter, MySQL wire);
   - a Postgres trial (the PG preview);
   - agent workspaces (Filesystem);
   - SQL-only HTTP endpoints (Data Service);
   - analytics (Lake, which is paid with no free quota).

   **What they cannot host:**
   - raw or transactional KV access (so no Live, metastore or Durable on TiKV);
   - an S3-compatible bucket (so no Loams Git truth or engine bucket);
   - queues, compute, or identity.
4. **Starter's free tier is per organization:** 5 free instances, 25 GiB row storage, 25 GiB columnar storage and 250M RUs a month in total [V].
   - The **spending limit is a hard cap** that denies new connections [V]. That makes Starter safe for a free try-out.
   - The **default instance cap is 100** per organization [V, release notes 2026-06-02].
   - Starter has **no resource groups** [V]. Loams has to enforce per-tenant fairness itself.
5. **Filesystem** went to public preview on 2026-10-15 [V]. It is POSIX-compatible when mounted, needs no database, and has layers, checkpoints, forks, owner tokens and scoped tokens [V]. Its money model is risky for a free hosted service:
   - an organization gets a $5 monthly allowance;
   - with no card: one file system per region, 2,000 files, 2 GB, 500 MB per file;
   - with a card: **no spending cap** at all, and writes cost $0.50 per 1,000 [V].
6. **The PG preview** is pgwire v3, with B-tree, GIN and HNSW indexes [V]. It has no partitioning, FDW, logical replication or custom extensions, and no production SLA [V]. It is AWS-only [V]. The free quota is generous: 10 free instances per organization, each with 50 GiB and 500M RUs a month [V].
7. **Branching** is in public preview, on Starter only [V]. The default is **5 branches per organization** [V]. Essential stopped offering branches to new instances after 2026-07-14 [V].
8. **The MCP and API claim needs correcting.**
   - The **TiDB Cloud API** (public preview) covers clusters, billing, Data Service and IAM [V].
   - The **TiDB MCP Server** is SQL-level: databases, queries, DDL and database users [V]. It does not manage clusters, IAM or billing [V].
   - No official control-plane MCP server was found. Lake has its own MCP server [V].
9. **Credits.** No "$500 startup credits" programme was found for TiDB Cloud.
   - The TiDB AI Startup Program says "up to $100,000" for pre-seed to Series B startups on AWS or Alibaba Cloud [V].
   - A separate notice mentions about $300 of Essential credits that expire in 45 days [U: secondary search result].
   - **HCP's trial credit is exactly $500 for 6 months** [V]. The owner's $500 may be that.
   - Whether any TiDB credit covers Filesystem or Lake is not stated anywhere [U].
10. **HashiCorp:** the only lasting free tier left is **HCP Terraform Free**: 500 managed resources and one concurrent run [V].
    - Everything else runs on the one-time $500 trial credit and then bills or stops [V].
    - Vault Secrets is retired, Consul Dedicated is retired, and Vagrant is closing on 2026-12-31 [V].
    - HCP has no compute that could host Loams Cloud.
    - The tools stay BSL 1.1, and IBM is now the licensor [V].
11. **Compute for the try-out is the user's own devices** ("the ground"), joined outbound. Cloudflare Workers can host the light control plane; loams.dev already runs on a Worker.
    - Hosted fallbacks for users with no device to spare: Oracle Always Free (now **2 OCPU and 12 GB**, not 4 and 24) [V], Hetzner CX23 at €5.99 [V], and Cloudflare Containers on Workers Paid [V].
    - **Tailscale's free Personal plan is non-commercial** [V]. A company device must not depend on it.

## 2. TiDB X and TiDB Cloud, product by product

### 2.1 The architecture

| Claim | Status | Source |
|---|---|---|
| Object storage (such as S3) is "the single source of truth for all data"; local disks hold Raft logs and cache | [V] | [TiDB X architecture](https://docs.pingcap.com/tidbcloud/tidb-x-architecture/) |
| The SQL layer (TiDB server) is stateless; compute scales independently of storage; separate SQL compute groups give workload isolation | [V] | Same page |
| "In TiDB X, TiKV becomes stateless, but we retain the shard abstraction" | [V-claim] (the CTO's blog; the docs only say local state is mostly cache) | [Ed Huang, "TiDBX Origin and Architecture", 2025-12-16](https://www.pingcap.com/blog/tidbx-origins-architecture/) |
| "Virtual Clusters": logical clusters that share compute and storage, isolated through metadata and routing; tenants as key prefixes | [V-claim] | Same blog |
| Available in Starter, Essential, Premium and BYOC | [V] | Architecture page; [SCaiLE 2026 announcements, 2026-10-15](https://www.pingcap.com/blog/tidb-cloud-scaile-2026-product-announcements/) |
| PingCAP plans to open-source the TiDB X core "by the end of 2026" | [V] (a stated plan, not a release) | SCaiLE 2026 announcements |

### 2.2 The tiers

| Tier | Status (2026-10-23) | Price | Notes | Sources |
|---|---|---|---|---|
| **Starter** | GA | $0 base. Free per organization: 25 GiB row, 25 GiB columnar, 250M RUs a month across up to 5 free instances (each 5 GiB, 5 GiB and 50M RUs). Beyond that: from $0.10 per 1M RUs, $0.20 per GiB-month row, $0.05 per GiB-month columnar | Scales to zero when idle. More than 5 instances needs a card and a monthly spending limit. Default cap of 100 instances per organization (the Instance Capacity Plan raises it through Sales) | [Pricing](https://www.pingcap.com/tidb-cloud-pricing/); [limitations and quotas](https://docs.pingcap.com/tidbcloud/serverless-limitations/); [spending limit](https://docs.pingcap.com/tidbcloud/manage-serverless-spend-limit/); [release notes 2026](https://docs.pingcap.com/tidbcloud/tidb-cloud-release-notes/) (2026-06-02); [Instance Capacity Plan](https://www.pingcap.com/programs/agentic-ai-instance-capacity) |
| **Essential** | Public preview | "Compute starts at $400/month", with minimum capacity charges; storage, backup and egress extra. Essential's regional page lists $0.20–$0.28 per RCU per month with a 2,000-RCU minimum [V-claim] | Elastic Mode by default since 2026-10-13. Single-AZ for new instances since 2026-06-09. **No branches for new instances after 2026-07-14** | Pricing; release notes (Jun 9, Jun 30, Oct 13) |
| **Premium** | Pricing page: public preview. Announced GA "available to customers in November 2026" | "Compute starts at $1,800/month", with minimum capacity charges | Isolated resources, auto-scaling, private connectivity, CMEK, three zones, 99.99% SLA | Pricing; [Premium/Filesystem/PG announcement, 2026-10-15](https://www.pingcap.com/?p=35378) |
| **Dedicated** | GA | From $1,376/month (AWS, 4 vCPU nodes) | Classic TiDB, not TiDB X | Pricing |
| **BYOC** | Public preview (AWS) since 2026-10-15 | Contact sales; TiDB Cloud fees plus the customer's own cloud bill | "Designed for existing TiDB Self-Managed customers"; PingCAP operates it in the customer's account | Pricing; release notes (Oct 15); announcement |

**Starter limits that matter to Loams** ([limitations](https://docs.pingcap.com/tidbcloud/serverless-limitations/), [limited SQL features on TiDB X](https://docs.pingcap.com/tidbcloud/limited-sql-features-tidb-x/), [features](https://docs.pingcap.com/tidbcloud/features/), all [V]):

- **Transactions and connections:**
  - Transactions can't last longer than 30 minutes.
  - Connections open longer than 30 minutes may drop.
  - AWS public endpoints time out after 340 seconds of idle.
  - The default cap is 400 concurrent connections, or 5,000 with a spending limit above zero.
- **No changefeeds and no Data Migration** on Starter.
- **No PITR, no CMEK, no resource control, and no audit logs.**
- **Unsupported statements and settings:**
  - `CREATE RESOURCE GROUP` and placement policies;
  - `BACKUP`/`RESTORE`;
  - `IMPORT INTO` (Starter allows only `LOAD DATA LOCAL INFILE`);
  - `SELECT … INTO OUTFILE`;
  - the `time_to_live` table attribute;
  - most `information_schema` cluster and `STATEMENTS_SUMMARY` tables, which are Premium/BYOC only;
  - several partitioning variables.
  - The password policy is enforced and can't be relaxed.
- **What happens at quota:**
  - New connections are denied.
  - Existing connections are throttled.
  - Access comes back when the limit is raised or the month resets.
- **Vector search and full-text search:** in public preview on Starter, and not available on Essential.
- **Analytical processing:** marked available on all plans.

### 2.3 Filesystem (`ti fs`)

| Fact | Status | Source |
|---|---|---|
| Public preview since 2026-10-15; "features and interfaces are subject to change without notice"; no SLA during preview; retention policy after preview not final | [V] | [Release notes](https://docs.pingcap.com/tidbcloud/tidb-cloud-release-notes/); [quick start](https://docs.pingcap.com/tidbcloud-filesystem/filesystem-quick-start/); [FAQ](https://docs.pingcap.com/tidbcloud-filesystem/filesystem-faq/) |
| "You do not need to provision or manage a TiDB database" | [V] | FAQ |
| POSIX-compatible access when mounted; FUSE on Linux (`fuse3`, `/dev/fuse`); WebDAV by default on macOS, FUSE via macFUSE; **no mounts on Windows** | [V] | FAQ; [regions and limitations](https://docs.pingcap.com/tidbcloud-filesystem/filesystem-regions-and-limitations/) |
| Default FUSE buffers writes locally, so other runtimes may not see a change until `ti fs drain-file-system` or unmount; killing the mount process can lose unsynced writes; WebDAV has no drain | [V] | FAQ; regions and limitations |
| The `coding-agent` mount profile keeps dependencies, build output, caches and Git internals local | [V] | Regions and limitations; announcement |
| Several runtimes can use one file system. Concurrent writes to one file are not merged | [V] | FAQ; [sharing](https://docs.pingcap.com/tidbcloud-filesystem/filesystem-sharing/) |
| **Tokens:** an owner token (full access, non-expiring unless `--ttl`) and **scoped tokens** limited by path prefix and operations (`read`, `list`, `search`, `write`, `delete`) with a TTL. Revoking an owner token does not revoke its scoped tokens. Changes take about 10 s to propagate | [V] | [Tokens](https://docs.pingcap.com/tidbcloud-filesystem/manage-filesystem-tokens/) |
| **Layers** (a writable overlay on a base path), **checkpoints**, **forks** (child layers), rollback and commit. A commit can fail on base conflicts; no merge or rebase; layers are "not backups or authorization boundaries" | [V] | [Layers and checkpoints](https://docs.pingcap.com/tidbcloud-filesystem/filesystem-layers-checkpoints/) |
| Also: Git workspaces (`ti fs-git clone/hydrate/add-git-worktree`), journals (append-only), a secrets vault with grants and audit events, semantic search with your own OpenAI-compatible embedding key | [V] | [CLI reference index](https://docs.pingcap.com/ai/llms.txt) |
| SDKs (Drive9 client) for TypeScript, Go, Python, **Rust**, Kotlin and Swift. "Drive9 is the workspace engine" | [V] | FAQ |
| Regions: AWS us-east-1, us-west-2, ap-southeast-1; Alibaba ap-southeast-1; GCP us-east1; Azure centralus | [V] | Regions and limitations |
| **Price:** reads $0.04 and **writes $0.50 per 1,000**; database storage (small files) $0.30/GB-month; object storage (large files) $0.025/GB-month; egress $0.09/GB (us-east-1) | [V] | [Filesystem pricing](https://www.pingcap.com/tidb-cloud-filesystem-pricing-details/) |
| **Free:** a $5.00 monthly allowance per organization, shared across file systems and regions. With no card: 1 file system per region, 2,000 files, 2 GB, 500 MB per file. At the limit, writes stop and files stay readable | [V] | FAQ; pricing; [usage limits](https://docs.pingcap.com/tidbcloud-filesystem/manage-filesystem-limits/) |
| **With a card:** per-file-system limits on storage, file count and file size can be set, **in the console only**. Read, write and egress have no limit, and "there is no configurable spending limit" | [V] | Usage limits; pricing |
| Usage per file system through the API or CLI | [U]: the docs show only the console | Usage limits |

### 2.4 PostgreSQL-compatible Starter (preview)

| Fact | Status | Source |
|---|---|---|
| Public preview on AWS since 2026-10-15; open sign-up; "intended for development and evaluation"; no production SLA; not offered on Premium or BYOC | [V] | Release notes; announcement |
| PostgreSQL wire protocol v3 (Simple and Extended Query) | [V] | [FAQ](https://docs.pingcap.com/tidbcloud/tidb-cloud-faq/); [PostgreSQL compatibility](https://docs.pingcap.com/tidbcloud/postgresql-compatibility/) |
| Index types: B-tree, **GIN**, **HNSW** (vectors), unique, partial, expression, `CREATE INDEX CONCURRENTLY`. No GiST, Hash, SP-GiST, BRIN or IVFFlat | [V] | Compatibility |
| **Not supported:** table partitioning and inheritance; **FDW**, tablespaces, `CREATE RULE`; logical and streaming replication; true `SERIALIZABLE` (it falls back to repeatable read with a warning); large objects; procedural languages other than PL/pgSQL; **custom or third-party extensions** (only a predefined list) | [V] | Compatibility |
| `pg_cron` | [U]: not named; custom extensions are refused, and the predefined list was not read | Compatibility |
| Partial support: triggers (statement triggers run per row), PL/pgSQL with limits, sequences (limited `ALTER SEQUENCE`), deferrable FKs enforced immediately; default isolation read committed | [V] | Compatibility |
| Free quota: **10 free instances per organization, each 50 GiB and 500M RUs a month**; "excessive usage may face restrictions or suspension" | [V] | Limitations and quotas |

### 2.5 Branching, Data Service, Lake, Zero, API, MCP, AI toolkit

| Product | Status | Facts | Source |
|---|---|---|---|
| **Branching** | Public preview, **Starter only** in the docs; the features page still ticks Essential, but the release notes say new Essential instances lose it after 2026-07-14 | Copy-on-write from a point in time (24 hours back on free instances, 14 days with a spending limit); **5 branches per organization by default**; 10 GiB per branch free (100 GiB with a spending limit); not for instances over 100 GiB; usually a few minutes to create; branches have no auto-scaling | [Branch overview](https://docs.pingcap.com/tidbcloud/branch-overview); [features](https://docs.pingcap.com/tidbcloud/features/); release notes (Jun 30) |
| **Data Service** | Preview (one endpoint page still says beta) | "Available only for TiDB Cloud Starter hosted on AWS"; endpoints are web APIs that execute SQL; API-key auth | [Data Service overview](https://docs.pingcap.com/tidbcloud/data-service-overview/) |
| **Lake** | Public preview since 2026-06-30 | Warehouse with SQL, full-text, vector and geospatial search; Personal from $1.60/hour compute plus $23/TB-month storage; **no free quota listed**; its own MCP server (2026-08-11) and a Python UDF SDK (2026-08-18). Its feature text matches Databend's word for word, but no source confirms Lake is built on Databend [U] | Pricing; release notes; [Lake page](https://www.pingcap.com/tidb-cloud-lake/) |
| **TiDB Cloud Zero** | Public preview | A temporary MySQL-compatible database created with no sign-up, kept 30 days, which can be claimed into a real account | [Zero announcement](https://www.pingcap.com/blog/tidb-cloud-zero-public-preview/) |
| **TiDB Cloud API** | Public preview | v1beta1 manages Starter, Essential and Dedicated, **billing, Data Service and IAM**; v1beta2 manages Premium; HTTP Digest auth. Audit and rate limits are not described | [API overview](https://docs.pingcap.com/api/tidb-cloud-api-overview/) |
| **`ti` CLI** | Preview since 2026-09-15 | `ti db` (Starter instances, branches, SQL users, connection strings, run SQL) and `ti fs` | Release notes; [CLI index](https://docs.pingcap.com/ai/llms.txt) |
| **TiDB MCP Server** | Not labelled (inside the "TiDB for AI" preview section) | `uvx --from pytidb[mcp] tidb-mcp-server`; tools `show_databases`, `switch_database`, `show_tables`, `db_query`, `db_execute`, `db_create_user`, `db_remove_user`; **no cluster, IAM or billing tools** | [TiDB MCP Server](https://docs.pingcap.com/ai/tidb-mcp-server/) |
| **Serverless driver** (`@tidbcloud/serverless`) | Beta | SQL over HTTP to Starter and Essential, for runtimes without TCP such as Cloudflare Workers; interactive transactions experimental | [Serverless driver](https://docs.pingcap.com/tidbcloud/serverless-driver) (search result; the page body did not load); [Cloudflare guide](https://docs.pingcap.com/tidbcloud/integrate-tidbcloud-with-cloudflare/) |
| **AI SDK and Reasoning Engine** | Announced October 2025 as the "AI Developer Toolkit" with the MCP server | No 2026 documentation found for the Reasoning Engine | [TiDB X launch press release](https://www.pingcap.com/press-release/pingcap-launches-tidb-x-new-ai-capabilities/) [U on current status] |

**Compute or runtime products: none.** We searched for functions, sandboxes and runtimes, and found only Data Service (SQL endpoints), Lake's external UDFs and Zero (databases). The Kimi case study describes "one agent, one sandbox, one database" and says the agent "owns the hosting layer end-to-end", without naming who provides the sandboxes [V-claim]; nothing suggests TiDB Cloud runs them.

### 2.6 Credits

| Programme | What it says | Status | Source |
|---|---|---|---|
| TiDB AI Startup Program | "Up to $100,000 in TiDB Cloud credits". For pre-seed to Series B AI startups building agents, sandboxes or AI infrastructure on AWS or Alibaba Cloud, in the US, Southeast Asia, Japan or Europe. Products covered, expiry and review time are not stated | [V] | [Programme page](https://www.pingcap.com/tidb-ai-startup-program/) |
| Essential trial credits | About $300, expiring 45 days after issue | [U]: a search snippet only | — |
| "$500 startup credits" (owner's note) | Not found for TiDB Cloud. **HCP's trial credit is $500 for 6 months** (§3.1) | [U] | — |
| Do credits cover Filesystem or Lake? | Not stated anywhere | [U] | — |

Ostrium is not incorporated yet (memory `company-formation`), and the programme asks for a startup stage. Applying is the owner's action (Q1277).

### 2.7 The owner's notes, claim by claim

| Owner's claim | Finding |
|---|---|
| Data on object storage, storage and compute decoupled | **Verified** |
| "Even TiKV becomes stateless" | **Verified as PingCAP's claim** (CTO blog). The docs keep local Raft logs and cache |
| Starter, Essential and Premium all run on TiDB X | **Verified** (BYOC too) |
| A virtualization layer above SQL gives every agent an isolated multi-tenant database, scaling to zero | **Partly verified.** "Virtual Clusters" (CTO blog) and a "virtual database interface" (Kimi case study) exist as PingCAP's descriptions, and Starter scales to zero. No product page offers "a database per agent" as a feature. The customer-facing unit is a Starter instance or a branch, with an instance cap of 100 and a branch cap of 5 per organization by default |
| Manus has more than 1.5 million TiDB Cloud clusters | **Verified as PingCAP's claim** (SCaiLE 2026 posts). The Manus case study itself gives no cluster count |
| Online DDL, instant creation, forking, HTAP, vector and full-text search | **Mostly verified.** Branching is copy-on-write but takes "a few minutes", not instant. Vector and full-text search are preview on Starter only. Analytical processing is on all plans. Online DDL is TiDB's long-standing behaviour; nothing contradicts it |
| Filesystem: persistent, shared, mountable, POSIX when mounted, layered, owner and scoped tokens, no database needed, `ti fs` | **Verified.** Add: preview, no SLA, writes buffered locally, no Windows mounts, and no spending cap once a card is on file |
| Several runtimes can hold one filesystem; Moonshot (Kimi) uses more than a million | **Verified as PingCAP's claim** (announcements). The Kimi case study describes databases, not Filesystem |
| PostgreSQL (Starter, preview): pgwire v3, HNSW, GIN; no partitioning, FDW or custom extensions; no `pg_cron`; no SLA | **Verified**, except `pg_cron`, which is not named (custom extensions are refused) |
| TiDB Cloud Lake: separate warehouse with vector, full-text and geo search | **Verified** (preview, no free quota) |
| Control plane: MCP and API (clusters, IAM, billing, audit, Data Service) | **Corrected.** The API covers clusters, billing, IAM and Data Service; audit is not described. The MCP server is SQL-level only |
| Branch APIs on Starter and Essential | **Corrected.** Starter only; new Essential instances have had none since 2026-07-14 |
| TiDB AI SDK and Reasoning Engine | **Announced in 2025**; no current Reasoning Engine docs found |
| BYOC public preview | **Verified** (AWS, 2026-10-15; aimed at existing self-managed customers) |
| Premium in November 2026, with isolation, private connectivity and CMEK | **Verified** |
| TiDB X core open-sourced by the end of 2026 | **Verified as a stated plan** |
| Essential about $400–500 a month, Premium about $1,800 a month | **Verified as starting compute prices** ($400 and $1,800), with storage and the rest extra |
| $500 startup credits | **Not found** for TiDB; see §2.6 |

## 3. The HashiCorp free tier

### 3.1 HCP services today

| Service | Status | Free | Source |
|---|---|---|---|
| **HCP billing in general** | — | A new organization gets **$500 trial credits for 6 months**, no card needed. When they run out without a card, "your HCP services are terminated" (a hard stop) | [V] [HCP billing](https://developer.hashicorp.com/hcp/docs/hcp/admin/billing) |
| **HCP Terraform** | GA | **Free:** 500 managed resources, unlimited users, no Teams, **one concurrent run** ("1 remote, 1 agent"), SSO, Sentinel/OPA, run tasks, VCS, private registry. The legacy free plan ended 2026-03-31. What happens above 500 resources is [U] | [V] [Plans](https://developer.hashicorp.com/terraform/cloud-docs/overview); [IBM limits](https://www.ibm.com/support/pages/hcp-terraform-limits); [free tier blog](https://www.hashicorp.com/blog/continuing-hcp-terraform-s-enhanced-free-tier-experience) |
| HCP Vault Secrets | **Retired**: end of sale 2025-06-30, end of life by 2026-07-01 | — | [V] [IBM notice](https://www.ibm.com/support/pages/node/7263986) |
| HCP Vault Dedicated | GA | No free tier (Development tier billed hourly, 25 clients) | [V] [Tiers](https://developer.hashicorp.com/hcp/docs/vault/tiers-and-features) |
| HCP Vault Radar | GA | Free trial: 2 data sources, 100 resources | [V] [Tiers](https://developer.hashicorp.com/hcp/docs/vault-radar/get-started/product-tiers) |
| HCP Boundary | Active | No free allowance confirmed; billed per identity that authenticates in a month; needs a self-managed worker at each private site | [V] [Pricing definitions](https://developer.hashicorp.com/hcp/docs/hcp/admin/billing/pricing-definitions); [multi-hop workers](https://developer.hashicorp.com/boundary/docs/workers/multi-hop/hcp) |
| HCP Consul Dedicated | **Retired 2025-11-12** | — | [V] [Migrate](https://developer.hashicorp.com/hcp/docs/consul/migrate) |
| HCP Packer | Active | Billed per bucket-hour; deactivated without a payment method or credits | [V] [Registry](https://developer.hashicorp.com/hcp/docs/packer/manage-registry) |
| HCP Waypoint | Bundled with HCP Terraform Standard and Premium only | — | [V] [Pricing](https://www.hashicorp.com/en/pricing) |
| HCP Vagrant | **Closing**: no new boxes from 2026-10-01, end of operations 2026-12-31 | — | [V] [EOL](https://developer.hashicorp.com/hcp/docs/vagrant/hcp-vagrant-eol) |

**HCP has no service that runs a control plane or an application.** At most, HCP Terraform Free can hold infrastructure-as-code state and run plans for Ostrium's own Loams Cloud resources.

### 3.2 Licences

- **The move to BSL:** HashiCorp moved its products from MPL-2.0 to **BSL 1.1** on 2023-08-10 for "all future releases"; the APIs and SDKs stayed MPL [V] ([announcement](https://www.hashicorp.com/en/blog/hashicorp-adopts-business-source-license)).
- **The current licence files** name **International Business Machines Corporation (IBM)** as licensor, for Terraform ≥ 1.6.0, Vault ≥ 1.15.0, Boundary ≥ 0.14.0, Consul ≥ 1.17.0, Packer ≥ 1.10.0, Nomad ≥ 1.7.0 and Vagrant ≥ 2.4.3 [V] ([Terraform LICENSE](https://raw.githubusercontent.com/hashicorp/terraform/main/LICENSE)).
- **Change Date:** four years after each release, when the version converts to MPL-2.0. No version has converted yet; the first convert around late 2027.
- **The Additional Use Grant** forbids offering the software "to third parties on a hosted or embedded basis in order to compete with IBM Corp.'s paid version(s)". It also says "products that are not provided on a paid basis are not competitive" [V].
- **No relicensing back to open source was found** [V].
- **The forks:**
  - **OpenTofu** is MPL-2.0, v1.13.2 (2026-10-21), and a CNCF Sandbox project since 2025-04-23 [V] ([CNCF](https://www.cncf.io/projects/opentofu/)).
  - **OpenBao** is MPL-2.0, v2.7.1 (2026-10-01), and an OpenSSF sandbox project [V].
- **OpenTofu and HCP Terraform:** HCP Terraform runs only Terraform remotely [V] ([CLI runs](https://developer.hashicorp.com/terraform/cloud-docs/run/cli)). OpenTofu can use it through the `cloud` block or the `remote` backend for state and local runs, with a version check it warns about [V] ([OpenTofu remote backend](https://opentofu.org/docs/language/settings/backends/remote/)).
- **Terms for a free multi-tenant service:** the HashiCorp AUP forbids bypassing account limits, bulk account creation and excessive use [V] ([AUP](https://www.hashicorp.com/aup)). The HCP user agreement could not be read [U].

**D11 and D1574:** they govern code and dependencies. Loams never copies, ports or links BSL or FSL code. Using a hosted service for Ostrium's own operations is a different question; nothing in Loams' product may depend on it. Design 78 §13 records the ruling.

### 3.3 Earlier history

- **Design 64** (approved 2026-10-19) answered the owner's "I want all of the HashiCorp stack in Loams, in Rust". It takes over each product's role on Loams' own primitives, prefers OpenTofu (MPL) to Terraform (BSL), and never copies BSL code.
- **The HCP row in design 64:** its §1 table mapped "HCP" ("someone else runs it") to Ostrium services. D1323 superseded that: there are no services.
- **No decision adopted HCP as a dependency.** None of this conflicts with using HCP Terraform Free as an optional operations tool for Ostrium's own infrastructure. Design 78 keeps it optional and recommends OpenTofu with state on R2, so nothing depends on IBM's free tier staying free.

## 4. Compute options

Fit judgements ("runs Forgejo" and so on) are ours, from the published limits. They are not vendor claims.

### 4.1 The user's own devices (the primary option)

- **What runs:** a laptop, a desktop or a homelab runs the whole light compose stack (about 1 GB idle, design 70 §1.2) or Loams Desktop's per-user daemon (D784).
  - Studio agents, Forgejo, Plane and Zulip fit on any machine with 8 GB or more.
  - SigNoz on House wants a homelab-class machine.
- **Cost:** $0 to Ostrium.
- **Limits:**
  - Laptops sleep, and devices come and go.
  - Company laptops are managed by IT (design 78 §9).
  - Home uplinks are slow for serving.
- **Networking:**
  - Outbound HTTPS to Loams Cloud and to TiDB Cloud (MySQL over TLS on port 4000, or HTTPS) needs no inbound port.
  - Device-to-device traffic goes over the user's tailnet or their own Cloudflare Tunnel (design 69).

### 4.2 Cloudflare

| Service | Facts | Source |
|---|---|---|
| **Workers** | **Free:** 100,000 requests a day, 10 ms CPU per request. **Paid:** up to 5 minutes CPU (30 s default), 10M requests and 30M CPU-ms included. 128 MB memory on both | [V] [Limits](https://developers.cloudflare.com/workers/platform/limits/), [pricing](https://developers.cloudflare.com/workers/platform/pricing/) |
| **Containers and Sandbox SDK** | GA since 2026-04-13. **Workers Paid only** ($5/month). Instance types from lite (1/16 vCPU, 256 MiB, 2 GB) to standard-4 (4 vCPU, 12 GiB, 20 GB). **Included:** 375 vCPU-minutes, 25 GiB-hours and 200 GB-hours a month. **Ephemeral disk**, with snapshots of up to 20 GB kept for 30 days. Inbound is **HTTP only, through a Worker**. Scales to zero (`sleepAfter`, 10 minutes by default); cold starts of 1–3 s | [V] [GA changelog](https://developers.cloudflare.com/changelog/post/2026-04-13-containers-sandbox-ga/), [pricing](https://developers.cloudflare.com/containers/pricing/), [limits](https://developers.cloudflare.com/containers/platform-details/limits/), [architecture](https://developers.cloudflare.com/containers/platform-details/architecture/) |
| **Workers VPC** | Beta, free during beta, on the Free and Paid plans. VPC Networks (2026-04-14) bind a whole Tunnel or Mesh | [V] [Workers VPC](https://developers.cloudflare.com/workers-vpc/), [VPC Networks](https://developers.cloudflare.com/changelog/post/2026-04-14-vpc-networks/) |
| **Tunnel** | On all plans; HTTP, TCP and SSH; 1,000 tunnels per account. **$0 is widely reported but not on the docs page** [U] | [V] [Tunnel](https://developers.cloudflare.com/tunnel/), [account limits](https://developers.cloudflare.com/cloudflare-one/account-limits/) |

Design 70 §7 and design 71 §7.1 already map Loams onto Cloudflare. They are not repeated here.

### 4.3 Tailscale and Headscale

| Fact | Status | Source |
|---|---|---|
| Personal plan: $0, 6 users, unlimited user devices, 50 tagged resources, 1,000 ephemeral-node minutes a month; **"only suitable for non-commercial use"** | [V] | [Pricing](https://tailscale.com/pricing) |
| **A device is active on one tailnet at a time** (fast user switching); it cannot send packets on two tailnets at once | [V] | [Fast user switching](https://tailscale.com/kb/1225/fast-user-switching) |
| Sharing a node into another tailnet: the shared node is **quarantined** (it accepts connections but cannot dial out); tagged machines cannot be shared | [V] | [Sharing](https://tailscale.com/kb/1084/sharing) |
| Auth keys and OAuth clients mint tagged devices programmatically; OAuth "apps" (consent flow) are alpha | [V] | [Auth keys](https://tailscale.com/kb/1085/auth-keys), [OAuth clients](https://tailscale.com/kb/1215/oauth-clients) |
| `tsnet` embeds a node in a program on a userspace stack, with no TUN device or root, and accepts `ControlURL` (Headscale) | [V] | Design 69 §5 cites [tsnet](https://tailscale.com/kb/1244/tsnet) |
| Headscale v0.29.5 (2026-10-13), pre-1.0, BSD-3-Clause; **not supported behind Cloudflare's proxy or Tunnel**; needs public 443/tcp and 3478/udp | [V] | [Releases](https://github.com/juanfont/headscale/releases); [reverse proxy](https://headscale.net/stable/ref/integration/reverse-proxy/); D583 |

### 4.4 Free or cheap hosted compute (a user's own fallback ground)

| Option | Facts | Fit | Source |
|---|---|---|---|
| **Oracle Always Free** | Arm A1: **2 OCPU and 12 GB** a month (1,500 OCPU-hours, 9,000 GB-hours), plus 2 AMD micro instances, 200 GB block storage and 10 TB egress. **Idle instances are reclaimed after 7 days**; "out of host capacity" errors are common | The light stack and Studio agents fit; SigNoz is tight | [V] [Always Free](https://docs.oracle.com/en-us/iaas/Content/FreeTier/freetier_topic-Always_Free_Resources.htm) |
| Google Cloud | One e2-micro (memory not on the page; 1 GB [U]), 30 GB disk, 1 GB egress; a $300 credit for 90 days | **Too small** for the light stack (about 1 GB idle) | [V] [Free features](https://docs.cloud.google.com/free/docs/free-cloud-features) |
| AWS | New accounts since July 2025: $100 plus up to $100 in credits. The free plan **ends after 6 months, then the account closes** | A trial, not a ground | [V] [Free tier plans](https://docs.aws.amazon.com/awsaccountbilling/latest/aboutv2/free-tier-plans.html) |
| Hetzner | CX23 (2 vCPU, 4 GB) €5.99; CX33 (4 vCPU, 8 GB) €8.99; EU only. Every model showed "currently unavailable" when read [U] | The cheapest paid ground | [V] [Cost-optimised](https://www.hetzner.com/cloud/cost-optimized/) |
| Fly.io | No free allowance for new organizations (2 machine-hours or 7-day trial) | Paid only | [V] [Pricing](https://docs.fly.io/about/pricing) |
| Render, Railway | Render free sleeps after 15 minutes and has no disk; Railway Free gives $1 a month | No | [V] [Render](https://render.com/docs/free), [Railway](https://railway.com/pricing) |
| e2b, Daytona, Modal, Vercel Sandbox | Agent sandboxes: e2b $100 one-time (1-hour sessions); Daytona $200 credit; Modal $30 a month; Vercel Sandbox Hobby 5 CPU-hours and 45-minute sessions (non-commercial) | Agents only, not Factory apps | [V] [e2b](https://e2b.dev/pricing), [Daytona](https://www.daytona.io/pricing), [Modal](https://modal.com/pricing), [Vercel Sandbox](https://vercel.com/docs/vercel-sandbox/pricing) |

### 4.5 What each option can run

| Option | Studio agents | Forgejo | Zulip | Plane | SigNoz | Cost |
|---|---|---|---|---|---|---|
| User's laptop (16 GB) | Yes | Yes | Yes | Yes | Tight | $0 |
| User's homelab | Yes | Yes | Yes | Yes | Yes | $0 |
| Oracle Always Free A1 | Yes | Yes | Yes | Yes | Tight | $0 (reclaim risk) |
| Hetzner CX33 | Yes | Yes | Yes | Yes | Yes | €8.99/month |
| Cloudflare Containers (user's account) | Yes | With external state | Marginal | Marginal | No | $5/month plus usage |
| Workers | No | No | No | No | No | Free or $5 |
| TiDB Cloud | **No** | **No** | **No** | **No** | **No** | — |

## 5. Fit with Loams' parts

| Loams part | On TiDB Cloud? | Why |
|---|---|---|
| **Loams SQL** (§47: adapted TiDB on Loams' TiKV, one keyspace per database or branch) | **Only as plain MySQL.** Starter exposes the SQL wire and nothing below it; keyspaces are TiDB Cloud's internal mechanism, not an API | No TiKV or PD endpoint, no keyspace API, no placement or resource groups. A Loams SQL database maps to a Starter instance, or to a database inside one, and a branch to a TiDB Cloud branch (5 per organization) |
| **Loams Postgres** (Neon fork, §51) | **No** (the PG preview is a different engine) | The PG preview is a trial target with no SLA and no extensions. The Neon fork stays the product |
| **Live** (`loams-kv` on redb or TiKV) | **No** | Needs transactional KV, which Starter does not expose |
| **Metastore** | **No** | Same; a TiDB metastore backend was superseded by D124 and D260 |
| **Durable** (Resonate on SQLite or TiKV) | **No**, not today | A MySQL-backed Resonate store would be new work (design 78 Q1268) |
| **House** (chDB, then the Rust engine, §75) | **Keep our own** | Lake is a different, paid warehouse with no free quota; the owner keeps House |
| **Git** (create-only PUTs on a bucket, §36) | **No bucket API.** Filesystem holds working trees, not Git truth | Filesystem's object tier is internal; whether its SDK offers create-only writes is [U] |
| **Agent workspaces** (Studio, §53) | **Yes: Filesystem** | Layers per run, scoped tokens per device, `coding-agent` profile, Git workspaces |
| **Metering** (§63) | **Partly** | TiDB's billing API gives organization-level cost; per-tenant use needs Loams' own counters |
| **Identity** (§76) | **No** for Zitadel (it needs Postgres 14–18); **maybe** for OpenFGA's MySQL datastore | OpenFGA v1.22 supports `mysql`; whether it runs on TiDB is [U] |

## 6. Multi-tenancy within TiDB Cloud's free tier

- **Instances:**
  - 5 free per organization.
  - More need a card and a spending limit.
  - The default cap is 100 per organization; more go through Sales.
  - So per-user instances do not scale past a handful on the free tier.
- **Branches:** 5 per organization, so not per user.
- **Databases inside one instance:**
  - No documented limit was found on the number of databases or tables [U].
  - Atlassian runs more than three million tables on 16 TiDB clusters [V-claim].
  - Isolation is MySQL grants (one SQL user per tenant database).
  - **No resource groups on Starter**, so one tenant can use up the instance's RUs. Fairness must be enforced in front of the instance (design 78 D1717).
- **The spending limit** is per instance and hard. It is the right cap for each shared instance.
- **Filesystem:**
  - Without a card: one per region per organization.
  - With a card: per-file-system storage and file-count limits (console only).
  - No cap on writes, reads or egress.
  - Scoped tokens give per-tenant path prefixes.
- **The organization's free totals** (25 GiB, 250M RUs, the $5 Filesystem allowance) are shared by every tenant. A free try-out on Ostrium's organization therefore needs small per-user quotas, a cohort limit, or users bringing their own TiDB Cloud organization (design 78's BYOG path).

## 7. Sources not already linked

- [TiDB Cloud Release Notes in 2026](https://docs.pingcap.com/tidbcloud/tidb-cloud-release-notes/) (read 2026-10-23; entries from Jan 15 to Oct 20).
- [Manus case study](https://www.pingcap.com/case-study/manus-agentic-ai-database-tidb/); [Kimi case study](https://www.pingcap.com/case-study/kimi-2-6-agent-hosting-platform-tidb-cloud/).
- [IBM completes acquisition of HashiCorp](https://newsroom.ibm.com/2025-02-27-ibm-completes-acquisition-of-hashicorp,-creates-comprehensive,-end-to-end-hybrid-cloud-platform) (2025-02-27).
- Existing Loams research: [`pingcap-tikv-ecosystem.md`](pingcap-tikv-ecosystem.md) (2026-10-21) for the open-source repos; `~/Documents/Ostriumlabs/pitch/research/cloudflare-byoc.md` (2026-10-21) for Cloudflare.
