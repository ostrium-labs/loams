# 03 — Cloudflare as a Deployment Target, and the Startup Credits

> **Imported on 2026-10-19.** Written in the former private `ostrium-labs/loams-platform` repository; open source (Apache-2.0) in the monorepo since the owner's order of 2026-10-19. "This repository", "proprietary" and "private" below describe that repository and the open-core boundary of the time, which no longer holds; see [the platform docs index](../README.md).

> **Moved from operon on 2026-10-02.** This was operon `docs/design/35-cloudflare-target.md` on operon PR #179 (unmerged; removed there in commit d26531d) under the owner's ruling of 2026-10-02 (operon §38 D440). The Cloudflare target is now a **commercial component**. The `CloudflareRunner`, the Workers and Durable Object crates, `DoSqliteFs` and `R2Fs`, the Tail Worker usage path and the credits plan live here. Doc 04 builds the hosted Loams Cloud on top of it.
>
> **Renumbering.** Decisions: D380 → PD20, D383 → PD21, D384 → PD22, D385 → PD23, D386 → PD24, D387 → PD25. Questions: Q380–Q383 → Q-CF-1–Q-CF-4, Q396–Q399 → Q-CF-5–Q-CF-8, plus Q-CF-9 (was Q373, the Workers runner before the auth plan). **D381** (no Loam S3 service, the `r2` provider) and **D382** (the `Fs` trait with `NativeFs` and `MemFs`) stayed in operon, in operon §36 §17. This document adds only the Cloudflare backends of `Fs`. §2's statement that the Cloudflare crates are Apache-2.0 in operon is **superseded**: they are proprietary here (operon D440). The `CloudflareRunner` implements operon's `Runner` trait (operon §24 §16, D375) as `RunnerKind::External("cloudflare")`.

Status: **Approved** (owner defaults, 2026-10-02: "do suggested for all") · 2026-10-01. Source: §13 "Cloudflare deployment target and startup credits" and the related open questions in §15 of the owner's draft "Loam Serverless Runtime — Consolidated Plan" (2026-09-30, `chatdump.md` lines 814–866 and 937–946). The owner asked on 2026-10-01 to fold that draft into the design docs, the decision log and the plans. This document says which Loam workloads run on Cloudflare's hosted platform and how, defines the `Fs` trait and the `CloudflareRunner`, records the platform limits and the credits programme as checked on 2026-10-01, and plans the spike that decides what ships. Loam Git, which is the largest workload here, is [operon §36](../../design/36-loams-git.md). The `Runner` trait itself is defined in **doc 02** (the protocol gateway and standards, written on another branch at the same time, which also amends operon §24 and operon §27 for it); this document implements it and does not redefine it.

Decisions **PD20–PD25**; open questions **Q-CF-1–Q-CF-9** (see the note above; operon §36 keeps D381, D382, D388–D399 and Q384–Q395). Plan: [CF1](../plans/2026-10-01-cf1-cloudflare-spike.md).

Markers: **(source)** means read on 2026-10-01 at the URL in §12. **(verify)** means the spike checks it first. **(estimate)** means computed, not measured. **(draft)** means the figure comes from the owner's draft and was not re-checked.

---

## 1. Summary

| # | Decision | Status |
|---|---|---|
| PD20 | **Cloudflare is a deployment target for Loam's edge services, not a second architecture.** Git's front end and per-repository sequencers (operon §36), the crates mirror, the build-cache endpoint and thin function runners can run on Workers, Durable Objects and R2. The engine's data plane (query, log, the hot tier, TiKV, Resonate) stays on Kubernetes runners. The `wasm32-unknown-emscripten` preview is a **spike (CF1), not a foundation** | Approved (owner defaults, 2026-10-02) |
| D381 | **No Loam-built S3 service.** The draft's "Rust S3-compatible service" is **RustFS** where Loam runs its own storage (D61, D178) and **R2** on Cloudflare, which already speaks the S3 API. `operon-store` stays the client. Tenants are isolated by **vended, prefix-scoped credentials** (`ObjectStoreProvider::issue_credentials`, operon §25 §5), which on R2 are temporary credentials with `prefixes` and `object-read-only` or `object-read-write`, up to 7 days. D178's provider list gains `r2` | In operon (authoritative there) |
| D382 | **An `Fs` trait with three backends**: `DoSqliteFs` (a Durable Object's SQLite storage: hot data and metadata), `R2Fs` (large immutable blobs) and `NativeFs` (local disk on Kubernetes and Lambda runners). `Fs` holds **warm, rebuildable state and scratch only**. Durable truth stays in the object store behind conditional writes (D1), so **Durable Object storage is a cache and a lease, never the source of truth** | In operon (authoritative there) |
| PD21 | **`CloudflareRunner` implements doc 02's `Runner` trait in two modes**: **thin** (workers-rs on `wasm32-unknown-unknown`; stable; the default; validates and calls Loam's and Resonate's HTTP APIs, never TiKV) and **native** (`wasm32-unknown-emscripten`; experimental; behind the off-by-default feature `cf-native`, on pinned patch sets). The native fallback is always the same Rust core on Cloudflare Containers or Kubernetes | Approved (owner defaults, 2026-10-02) |
| PD22 | **Tenant code on Cloudflare runs in the tenant's own Cloudflare account first** (BYO account). Untrusted tenant code on Loam's account runs only in a **Workers for Platforms dispatch namespace in untrusted mode**, never in a Loam Worker; on Loam's account only that way (Q-CF-3, answered 2026-10-02 by PD33) | Approved (owner defaults, 2026-10-02) |
| PD23 | **Usage on Cloudflare goes through operon §27's hooks.** A Tail Worker turns each invocation's trace event (`CPUTimeMs`, `WallTimeMs`) into a `loam.meter.v1.Invocation` with `cpu_estimated = false` and sends `HostReport`s over HTTPS to a configured consumer (off by default), since there is no node socket. Billing stays in `loam-platform` (D190, D202). Refines D201 | Approved (owner defaults, 2026-10-02) |
| PD24 | **Heavy CPU never runs in a Worker.** Pack indexing and generation beyond small pushes, repacking, compilation and large compression run in Cloudflare Containers or on Loam's Kubernetes runners | Approved (owner defaults, 2026-10-02) |
| PD25 | **The credits plan.** Apply to Cloudflare for Startups **after CF1 shows that at least one workload ships on Cloudflare**, so the one-year validity is not spent on the spike; plan usage to the 12-month window. The tiers on Cloudflare's page today are **$10K, $100K and $350K**, not the draft's four; **R2 is capped at $10K**, which at beta scale is not binding (§8) | Approved (owner defaults, 2026-10-02) · (owner decision for the timing) |

## 2. Context and scope

- `loams.dev` is registered on Cloudflare (bought by the owner; ruling of 2026-10-01: the domain is `loams.dev`, packages are `loams` on crates.io, PyPI and npm (`@loams`), Go modules are `loams.dev/...`, and Java is deferred). The Next.js landing page and the React SPA, and the console at `console.loams.dev`, are hosted there by the private `loam-cloud` repository; they are out of scope here (D220).
- The owner's goal (draft §13.1): run the Loam runtime on Cloudflare with a filesystem, an S3-compatible service, Git and a Rust package cache, and apply for startup credits.
- **What is open source and what is not** (D220, as amended by operon D440 on 2026-10-02). The `Fs` trait with `NativeFs` and `MemFs` is Apache-2.0 in operon (operon §36 §17). Everything Cloudflare-specific here is **proprietary**: the `DoSqliteFs` and `R2Fs` backends, the `CloudflareRunner`, the Worker and Durable Object crates for Git, the cache and the mirror, `loam-tail`, Loam Cloud's own account, its `wrangler` configuration, credentials and fleet automation. Customers get them through the hosted service or Cloudflare BYOC (doc 04 PD36), not as open-source self-hosting.

**How this relates to operon §24.** operon §24 runs **open-source workerd** on Loam's own nodes, under gVisor, one process per tenant (D171). This document targets **Cloudflare's hosted Workers**. A tenant's `fetch`-contract bundle (D181), built by a framework's Cloudflare adapter, is the same artifact for both: T0 on Loam's workerd, or the `CloudflareRunner`. On hosted Cloudflare, Durable Objects, R2, KV and Queues are Cloudflare's own bindings, so operon §24 §2.2's non-goal (no Loam implementation of Cloudflare's product bindings) is unchanged.

## 3. Mapping Loam workloads onto Cloudflare

| Workload | On Cloudflare | Why | Fallback |
|---|---|---|---|
| **Git front end** (Smart HTTP routing, auth, ref advertisement) | Worker (thin) | Stateless, I/O-bound; R2 egress is free for clones | The `gateway` role on Kubernetes |
| **Git sequencer** per repository (operon §36 §4.4) | Durable Object named by `(ns, repo_id)`; state in `DoSqliteFs` | Single-threaded, single-instance per name: the natural sequencer and isolation unit. Correctness comes from R2's create-only PUTs, not the Durable Object (D382, operon §36 D389) | Rendezvous owner under a lease (D75) |
| **Git receive and upload** (pack indexing, pack assembly) | Worker for small pushes and whole-pack reuse; Container for the rest | 128 MB per isolate; `gix-pack` builds for `wasm32-unknown-unknown` (gitoxide CI); measured in CF1 | Kubernetes worker |
| **Git compaction** (`git repack`) | Container, scheduled by the Durable Object's alarm | CPU-heavy, needs the `git` binary (operon §36 D396) | Kubernetes worker task |
| **Crates mirror** (operon §36 §9) | Worker + R2 + the Cache API | Read-through, content-addressed; no index database needed (operon §36 D399) | The `gateway` role |
| **Build cache** (operon §36 §8) | Direct: sccache → R2 S3 API with temporary credentials. Gateway: a Worker serving the WebDAV subset over the R2 binding | R2 temporary credentials scope by prefix and read-only permission | RustFS or S3 |
| **S3 for tenants** | R2 itself, reached with vended credentials (D381) | Already S3-compatible | RustFS |
| **Thin function runner** | Worker per function (BYO account) or a Workers for Platforms user Worker (PD22) | The `fetch` contract runs unchanged | T0 workerd (operon §24) |
| **`http-port` functions** | Container | Workers do not listen on ports | T2 gVisor (operon §24) |
| **Static sites** | Workers static assets | — | Object store + edge (operon §24 §4.3) |
| **Go vanity imports** (`loams.dev/...`) | A Worker (or Pages route) on `loams.dev` answering `?go-get=1` with the `go-import` (and `go-source`) meta tags that point at the module's repository | The owner's ruling of 2026-10-01: Go module paths are `loams.dev/...`. A static answer per module path; no state | Any static host for `loams.dev` |
| **Event mirror and async events** | A Queue whose consumer calls `ProduceCloudEvents` (D270) | The streams gRPC protocol stays the narrow waist (draft §4.1) | Direct `ProduceCloudEvents` |
| **Retrieval engine, log, hot tier, TiKV, Resonate, Live** | **Not on Cloudflare** | Threads, NVMe, gRPC servers, multi-GB memory; `tikv-client` (tonic, tokio) does not build for `wasm32-unknown-unknown` | Kubernetes |

The draft's per-tenant Durable Object "for coordination and metadata" in front of R2 is not needed for S3 (D381); per-repository Durable Objects are kept, for Git (operon §36).

## 4. The `Fs` trait (D382)

### 4.1 Why a trait

Loam code that must run both in Workers and natively needs files for spill buffers (a pack arriving on a push), local caches (pack indexes, scope caches, operon §36 §5.4) and small mutable metadata (a sequencer's idempotency index). In thin mode (`wasm32-unknown-unknown`) there is no `std::fs`. In native mode (`wasm32-unknown-emscripten`) `std::fs` exists through `-sNODERAWFS`, forwarded to `node:fs`, and `worker-fs-mount` can mount a `durable-object-fs` backend under it. The trait gives one async interface over all three, and CF1 measures `std::fs`-over-`worker-fs-mount` against `DoSqliteFs` directly.

### 4.2 The trait

`crates/operon-fs` (Apache-2.0, no Cloudflare dependency):

```rust
/// Relative, '/'-separated, no empty, '.' or '..' segments, at most 1,024 bytes (R2's key limit).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FsPath(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode { Overwrite, CreateNew }

#[derive(Clone, Debug)]
pub struct FsMeta { pub len: u64, pub modified_unix_ms: i64 }

#[derive(Clone, Copy, Debug)]
pub struct FsCaps {
    pub append: bool,          // DoSqliteFs, NativeFs
    pub atomic_rename: bool,   // DoSqliteFs (one transaction), NativeFs (rename(2))
    pub max_file: u64,         // DoSqliteFs: 1 GiB by default; R2Fs: 5 GiB single PUT; NativeFs: disk
    pub survives_restart: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("not found: {0}")] NotFound(FsPath),
    #[error("already exists: {0}")] AlreadyExists(FsPath),
    #[error("unsupported on this backend: {0}")] Unsupported(&'static str),
    #[error("too large: {0} bytes")] TooLarge(u64),
    #[error("backend: {0}")] Backend(String),
}

#[cfg_attr(not(target_family = "wasm"), async_trait::async_trait)]
#[cfg_attr(target_family = "wasm", async_trait::async_trait(?Send))]   // Workers futures are !Send
pub trait Fs: fmt::Debug {
    fn caps(&self) -> FsCaps;
    async fn read(&self, path: &FsPath) -> Result<Bytes, FsError>;
    async fn read_range(&self, path: &FsPath, range: Range<u64>) -> Result<Bytes, FsError>;
    /// `CreateNew` fails with `AlreadyExists`; a returned write is visible to every later read.
    async fn write(&self, path: &FsPath, data: Bytes, mode: WriteMode) -> Result<(), FsError>;
    /// Returns the new length. `Unsupported` on R2Fs.
    async fn append(&self, path: &FsPath, data: Bytes) -> Result<u64, FsError>;
    /// Atomic replace of `to`. `Unsupported` on R2Fs.
    async fn rename(&self, from: &FsPath, to: &FsPath) -> Result<(), FsError>;
    async fn remove(&self, path: &FsPath) -> Result<(), FsError>;   // NotFound is Ok
    async fn stat(&self, path: &FsPath) -> Result<Option<FsMeta>, FsError>;
    /// Paths under `dir`, sorted, strictly after `after`, at most `limit`.
    async fn list(&self, dir: &FsPath, after: Option<&FsPath>, limit: usize)
        -> Result<Vec<(FsPath, FsMeta)>, FsError>;
}
```

### 4.3 Backends

| Backend | Crate | Storage | Notes |
|---|---|---|---|
| `NativeFs` | `operon-fs` | `tokio::fs` under a root directory; `rename(2)`; fsync of file and directory when `survives_restart` | Kubernetes, Lambda (`/tmp`, not surviving), laptops |
| `MemFs` | `operon-fs` | in memory | tests, and the conformance suite's reference |
| `DoSqliteFs` | `deploy/cloudflare/operon-fs-cloudflare` | Tables `fs_files(path TEXT PRIMARY KEY, len INTEGER, modified_unix_ms INTEGER, chunks INTEGER)` and `fs_chunks(path TEXT, idx INTEGER, data BLOB, PRIMARY KEY (path, idx))`; 1 MiB chunks (rows may be up to 2 MB); every write, append and rename in one storage transaction | 10 GB per Durable Object; 100 KB statements; 100 columns per table (§6). Survives restarts, but D382 still treats it as rebuildable |
| `R2Fs` | `deploy/cloudflare/operon-fs-cloudflare` | Objects under a prefix through the R2 binding; `CreateNew` with `onlyIf`/`If-None-Match` | No append, no rename; immutable blobs |

One conformance suite (`operon-fs::conformance`) runs against every backend, with capability flags selecting the append and rename cases: read-after-write, `CreateNew` races, range reads at boundaries, list pagination, rename atomicity under a crash (Native, DoSqlite) and the limits. On Cloudflare it runs inside a test Worker in `wrangler dev` (local `workerd`, which has Durable Object SQLite and an R2 simulator) and once against a real account in CF1.

## 5. The `CloudflareRunner` (PD21, PD22)

### 5.1 What it implements

The `Runner` trait is doc 02's. Its method set is fixed there; the `CloudflareRunner` maps it onto these Cloudflare operations:

| Runner concern | Cloudflare operation |
|---|---|
| Deploy a function version | Upload the bundle and its bindings through the Workers scripts API (or a Workers for Platforms user Worker), with a `compatibility_date` from the manifest |
| Route | A Workers route or custom domain; or a dispatch from Loam's dispatch Worker (PD22) |
| Invoke async events | A Queue consumer bound to the function |
| Durable waits | The Resonate HTTP gateway (D173, D261), never a Durable Object of the tenant's |
| Limits | CPU limit per invocation (Paid: up to 5 min, default 30 s) and subrequest limits in the script settings; custom limits from the dispatch Worker for user Workers |
| Usage | The Tail Worker of PD23 |
| Retire | Delete the script version and its routes |
| Capabilities reported | `contracts: [fetch, static]` (thin), `+ http-port` with Containers; `memory_mb: 128`; `tcp_in: false` |

### 5.2 Thin mode (default)

- **Toolchain:** workers-rs 0.8.7 (Apache-2.0, 2026-09-25) on `wasm32-unknown-unknown`, built with `worker-build`; or the tenant's JavaScript bundle unchanged.
- **What runs:** request validation, protocol conversion and calls to Loam's APIs and Resonate's HTTP gateway over `fetch`. Loam's own Workers (Git front end, mirror, cache) use `Fs`'s Cloudflare backends and `operon-store` only where `object_store` builds for the target (CF1 checks; otherwise the R2 binding through `R2Fs` and a small `Store` adapter).
- **Never:** `tikv-client`, tonic servers, threads or blocking I/O (draft §2 item 5 stands).

### 5.3 Native mode (`cf-native`, experimental)

What the Cloudflare blog announced on 2026-09-28 ("Supporting native Rust in Workers with the new Emscripten target for wasm-bindgen"; "the first public experimental preview") (source):

- `worker-build --emscripten` (and `--tokio`) builds Rust for `wasm32-unknown-emscripten`, with examples `emscripten`, `emscripten-tokio` and `emscripten-tcp` in workers-rs.
- `-sNODERAWFS` bridges file-system calls to `node:fs`. `worker-fs-mount` (Dan Lapid) mounts a `node:fs`-compatible backend; its `durable-object-fs` backend "stores files as rows in the Durable Object's SQLite storage", written synchronously and committed with the object's transaction.
- `-sNODERAWSOCKETS` gives epoll, TCP, UDP and Unix sockets over `node:net`. **General inbound TCP is "upcoming"**; the Minecraft demonstration accepts players' connections into a Durable Object, within the experimental preview. Neither changes PD20's placement of the data plane, whose blockers are threads, memory and local NVMe.
- **Tokio** runs either through JSPI (which needs thread-local context swaps during stack switches) or through a proposed `LocalEventLoop` that replaces parking with a host-driven wake; `LocalEventLoop::block_on` panics where a runtime would park. The first Tokio patch for the target is upstream; further patch sets are in review; `libc`, `socket2` and `mio` needed patches (mostly adding the target to platform gates). The examples depend on these patches directly. The post links no PRs and states no size, memory or performance figures, and it does not say whether JSPI is enabled in Workers (Q-CF-1).
- The demonstration is the Pumpkin Minecraft server (Rust, Tokio) in a Durable Object.

So native mode would let gitoxide's on-disk code, Tokio services and socket clients run in a Durable Object. Loam treats it as experimental until the patches are upstream: the feature is off by default, its patch sets are pinned in `deploy/cloudflare/patches/` with their upstream PR links, and every workload in native mode has a thin-mode or Container fallback. Whether `tikv-client` could now run with sockets is re-spiked in CF1 but nothing depends on it (draft §13.4).

### 5.4 Tenancy (PD22)

Hosted Workers isolates are Cloudflare's security boundary; Loam adds none of its own. Two models:

1. **BYO account (first).** The runner deploys to a tenant's own Cloudflare account with the tenant's API token. Isolation, billing and limits are Cloudflare's, per account. This fits BYOC (D64).
2. **Loam's account (later, Q-CF-3).** Tenant code runs as user Workers in a Workers for Platforms dispatch namespace, which runs them in untrusted mode with isolated caches, behind a Loam dispatch Worker that sets custom CPU and subrequest limits per plan. Workers for Platforms is a separate plan ($25/month, 20M requests and 60M CPU-ms included, $0.02 per extra script beyond 1,000, per Cloudflare's pricing page; verify).

## 6. Constraints (checked 2026-10-01)

| Limit | Value | Source (last updated) | Consequence |
|---|---|---|---|
| Memory per isolate (Workers and Durable Objects) | **128 MB** | Workers limits (2026-09-05) | gitoxide in a Worker only for small packs; spill to `Fs`; CF1 measures |
| CPU per HTTP request | Free 10 ms; Paid **default 30 s, up to 5 min** | Workers limits | Repack and big pack generation go to Containers (PD24) |
| CPU per Cron Trigger | Paid 30 s (interval < 1 h) or 15 min (≥ 1 h) | Workers limits | Sweepers in Containers or on Kubernetes |
| Worker size | 64 MiB uncompressed (verify: older pages showed a compressed limit) | Workers limits | Native-mode binaries measured in CF1 |
| Startup | global scope within 1 s | Workers limits | Cold start measured in CF1 |
| Subrequests | Free 50; Paid 10,000 per request | Workers limits | Range-read fan-out per fetch is bounded |
| Outgoing connections waiting for headers | 6 at once | Workers limits | Pipelines R2 reads through the binding, not many sockets |
| Threads | none: Durable Objects are "single-threaded and cooperatively multi-tasked" | Durable Objects concepts (2026-07-15) | One sequencer per object is natural; no parallel pack work |
| Durable Object throughput | soft limit **1,000 requests/s** per object | Durable Objects limits (2026-06-01) | Above operon §36's 30 pushes/s target by a wide margin; reads go to the front-end Worker |
| Durable Object SQLite | **10 GB** per object; rows, strings and BLOBs ≤ **2 MB**; statements ≤ 100 KB; 100 columns per table; unlimited per account on Paid | Durable Objects limits | 1 MiB chunks in `DoSqliteFs` |
| R2 objects | ≤ 5 TiB; single PUT ≤ 5 GiB; multipart ≤ 10,000 parts; keys ≤ 1,024 bytes | R2 limits (2026-06-08) | `.lpk` above 5 GiB is multipart |
| **R2 writes to one key** | **1 per second** | R2 limits | Rules out a CAS'd head pointer for Git (operon §36 D389) |
| R2 conditional writes | `If-Match` and `If-None-Match` on PutObject (S3 API); `onlyIf` / `R2Conditional` and conditional headers (except `If-Range`) on the binding, where a failed condition returns `null` | R2 S3 API and Workers API reference (2026-07-31) | The commit protocol works over the binding and the S3 API |
| R2 consistency | strong read-after-write, metadata, delete and listing; IAM changes up to a minute | R2 consistency | operon §36's linearizable reads hold |
| Containers | instance types `lite` (1/16 vCPU, 256 MiB, 2 GB) to `standard-4` (4 vCPU, 12 GiB, 20 GB); per account 1,500 vCPU and 6 TiB memory concurrently | Containers limits (2026-09-30) | Repack and pack generation fit `standard-2` or larger. GA status not confirmed (Q-CF-2) |
| D1 | 10 GB per database (Paid), not raisable | D1 limits (2026-04-21) | Not used (operon §36 D399) |

**Prices** (Standard plan; read 2026-10-01):

| Product | Included | Over |
|---|---|---|
| Workers | 10M requests and 30M CPU-ms a month (with the $5 plan) | $0.30 per million requests; $0.02 per million CPU-ms (no charge for wall time) |
| Durable Objects | 1M requests; 400,000 GB-s; 25B rows read; 50M rows written; 5 GB-month | $0.15 per million requests; $12.50 per million GB-s; $0.001 per million rows read; $1.00 per million rows written; $0.20 per GB-month (page updated 2026-09-30) |
| R2 | 10 GB-month; 1M Class A; 10M Class B | $0.015 per GB-month; $4.50 per million Class A; $0.36 per million Class B; **no egress fee** (page updated 2026-10-01) |
| Containers | 25 GiB-h memory, 375 vCPU-min, 200 GB-h disk (with the $5 plan) | $0.0000025 per GiB-s; $0.000020 per vCPU-s; $0.00000007 per GB-s; egress $0.025/GB in North America and Europe after 1 TB |

Whether a SQLite-backed Durable Object that only waits for R2 is billed duration for that time is not stated on the pricing page (Q-CF-4). operon §24 §9's CPU-time comparison stands: Cloudflare's CPU price is $0.072 per CPU-hour.

## 7. Usage and metering (PD23)

- Each Loam Worker and each runner-deployed function has a Tail Worker (`loam-tail`). Its trace events carry `CPUTimeMs` and `WallTimeMs` at the top level (Cloudflare changelog, 2025-04-09; Workers Trace Events fields).
- `loam-tail` maps an event to a `loam.meter.v1.Invocation` (operon §27 §3.3): `org` and `namespace` from the script's tags, `function` and `version` from the script name and version id, `cpu_usec = CPUTimeMs × 1000`, `cpu_estimated = false` (Cloudflare measured it), `fuel = 0`.
- It batches them into `HostReport`s (`host_id` = the Tail Worker's deployment id; `seq` from a Durable Object counter) and POSTs them to the configured consumer URL, resending until acknowledged, within the Durable Object's storage bound. With no consumer configured nothing is sent.
- Git, cache and mirror families (operon §36 §11) are exported by the Workers as OTLP metrics to a configured collector; a Worker has no scrape endpoint.

## 8. Credits plan (PD25)

**Cloudflare for Startups** (cloudflare.com/forstartups, read 2026-10-01; no date on the page):

| Tier | Credits |
|---|---|
| Tier 3 | $10K |
| Tier 2 | $100K |
| Tier 1 | $350K |

- **Validity:** "Credits are valid for one year or until fully consumed, whichever comes first"; no extensions.
- **Caps:** R2 "covered up to a $10,000 cap"; Workers AI $2.5K, $10K or $50K by tier. Credits cover "compute, storage, delivery, and AI services"; no per-product list (Containers, Durable Objects, D1) was found (Q-CF-5).
- **The draft's figures are an older structure.** The four tiers ($5K, $25K, $100K, $250K) and the "R2 and Cache Reserve" cap appear in an older Cloudflare blog post and in search snippets; the current page shows three tiers and names R2 only.

**Is the R2 cap binding? (estimate)** $10K of R2 is about 667,000 GB-months at $0.015, or about 55 TB held for twelve months, or 2.2 billion Class A operations. Loam's beta (Git, caches, the mirror) is far below that, so the cap does not limit the storage plan at beta scale; it matters only for large build caches at a $100K+ tier. Workers CPU at $0.02 per million CPU-ms is similarly far from $10K at beta load.

**Plan.**

1. Run CF1 on the Workers Paid plan ($5/month; Containers need it) and the free allowances. CF1's own spend is small (estimate: under $50).
2. If CF1 says "ship" for at least one workload, apply for the credits.
3. Plan usage so the credit burn fits twelve months: the Git and mirror beta first, the build cache second (largest storage), thin functions third.
4. Keep the workloads portable (PD20): when the credits end, each workload has a Kubernetes fallback (§3).

## 9. The spike (CF1)

[CF1](../plans/2026-10-01-cf1-cloudflare-spike.md) runs the draft's six steps, with measurements:

| # | Step | Measures |
|---|---|---|
| 1 | Build and deploy workers-rs's `emscripten` and `emscripten-tokio` examples | Wasm size (raw and gzip), deploy time, cold and warm start, CPU-ms per request, JSPI vs `LocalEventLoop` (Q-CF-1) |
| 2 | `Fs` with `DoSqliteFs` and `R2Fs`; the conformance suite; `std::fs` over `worker-fs-mount` in native mode | Latency per operation and size (4 KiB, 1 MiB, 64 MiB), rows read and written, $ per 1,000 operations |
| 3 | Clone and fetch a small repository with gitoxide inside a Durable Object (native mode), and index a pushed pack with `gix-pack` (thin mode) | Peak linear memory (`memory_size` × 64 KiB), CPU-ms, wall time, the largest pack that fits in 128 MB |
| 4 | S3 PUT and GET to R2 through `operon-store` (S3 API) and through the binding; the conditional-write race of operon §36 §4.2; one key written twice a second | Latency p50/p99, 412 behaviour, the per-key limit's error |
| 5 | Serve a crate download through a mirror prototype | Cold (upstream) and warm (R2, Cache API) latency, Class A and B operations |
| 6 | The same operations on the native runners (k3d with `NativeFs` and RustFS) | The comparison table; the "ships on Workers" decision per workload |

**Decision rule** (per workload): ship on Workers if peak memory stays under 64 MB (half the isolate), p99 CPU per interactive operation under 1 s, cost per operation within 2× the native runner's at beta volume (estimate), and thin mode needs no unmerged patch. Native mode ships only when its Tokio, `libc`, `socket2` and `mio` patches are upstream, or the owner accepts pinned forks (Q-CF-7).

## 10. Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | The Emscripten preview changes or stalls | Thin mode is the default; native mode is a feature with fallbacks (PD21) |
| 2 | Pinned Tokio, `libc`, `mio` and `socket2` patch sets drift from upstream | Pin by commit in `deploy/cloudflare/patches/`, each with its upstream PR; rebase monthly; drop each on merge |
| 3 | 128 MB per isolate is too small for Git work | Size gates route big pushes and fetches to Containers (PD24); CF1 sets the thresholds |
| 4 | Lock-in to Durable Objects | Durable Objects are a cache and a sequencer placement only (D382); the same core runs with a lease on Kubernetes |
| 5 | The one-year credits window ends | Apply after CF1 (PD25); portable workloads |
| 6 | Hosted Workers features drift from open-source workerd, so a bundle works in one and not the other | operon §24's Q-RT-3 support matrix covers both targets |
| 7 | Multi-tenant abuse on Loam's account | BYO account first; Workers for Platforms with custom limits later (PD22) |

## 11. Conflicts with existing decisions, and how they are resolved

| Earlier | The draft or this document | Resolution |
|---|---|---|
| D1: object storage is the only durable source of truth | The draft: a Durable Object per tenant "for coordination and metadata", indexes "in a Durable Object or D1" | D382: Durable Object storage is a cache and a lease; truth stays in R2 behind conditional writes |
| D61, D178: RustFS is the self-hosted object store | The draft: "a Rust S3-API service fronting R2" | D381: no Loam S3 service; RustFS self-hosted, R2 on Cloudflare; `r2` joins D178's providers |
| D190, D200–D202: metering hooks only | The draft §13.3: Durable Objects as "a natural metering unit for CPU-time billing" | PD23: Tail Worker events through operon §27's `HostReport`; billing in `loam-platform` |
| D201: `HostReport` on a node socket | No node on Cloudflare | PD23 refines D201: HTTPS delivery from a Tail Worker, same messages and ack rule |
| operon §24 §4.2, Q-RT-4: Durable Objects in clusters | Loam's own services use Durable Objects on Cloudflare | Narrows Q-RT-4: Loam's own services need no Durable Objects in clusters (a rendezvous owner and lease replace them); Q-RT-4 remains for tenant Durable Objects |
| operon §24 Q-RT-3: hosted-only features | The `CloudflareRunner` targets hosted Workers | Q-RT-3 stays for Loam's workerd; the same bundle runs on hosted Workers unchanged |
| D184: Envoy is the edge | On Cloudflare, Cloudflare is the edge | No conflict: Envoy is the edge of Loam's own clusters |
| The draft §2 item 5 and §13.4: `tikv-client` might run with sockets | — | Re-spiked in CF1; nothing depends on it (PD21) |
| The draft §2, §3, §4.3: Resonate on TiDB | The thin runner calls Resonate's HTTP gateway | D260, D261: Resonate's store is TiKV; the runner does not care which |
| The draft §13.6: four tiers, $5K–$250K; R2 and Cache Reserve capped | Cloudflare's page on 2026-10-01 | PD25: three tiers, $10K/$100K/$350K; R2 capped at $10K |
| The draft §13.4: "128 MB, as far as I know" | Verified | 128 MB on both plans (Workers limits, 2026-09-05) |

## 12. Open questions

| # | Question | Owner | Needed by |
|---|---|---|---|
| Q-CF-1 | Does hosted Workers enable JSPI, so the Tokio JSPI path works, or only `LocalEventLoop`? CF1 Task 2 measures it, and thin mode uses `LocalEventLoop` until JSPI is confirmed | Eng | CF1 Task 2 |
| Q-CF-2 | ~~Are Cloudflare Containers generally available, and what is their cold start for a `git repack` image~~ Answered 2026-10-02 by the owner: the plan's default — not an owner decision; CF1 Task 6 measures GA status and cold start; until then repack routes to Loam's Kubernetes runners (PD24) | Eng | Resolved |
| Q-CF-3 | ~~Run tenants' untrusted code on Loam's own Cloudflare account through Workers for Platforms, or only in tenants' accounts~~ Answered 2026-10-02 by the owner: the recommended default — yes, on Loam's own account only in a Workers for Platforms dispatch namespace in untrusted mode (PD22, PD33) | Founder | Resolved |
| Q-CF-4 | ~~Is a SQLite-backed Durable Object billed duration while it awaits an R2 PUT, and what does a sequencer cost per million pushes~~ Answered 2026-10-02 by the owner: the plan's default — not an owner decision; CF1 Task 8 measures it and records the cost per million pushes | Eng | Resolved |
| Q-CF-5 | Which products the startup credits cover (Containers, Durable Objects, Queues), beyond "compute, storage, delivery, and AI". Owner action pending (2026-10-02): confirm with Cloudflare, when applying, which products the credits cover | Owner action | Before applying |
| Q-CF-7 | ~~Ship native mode on pinned patch sets before upstream merges them, or wait~~ Answered 2026-10-02 by the owner: the recommended default — wait: thin mode ships, native mode only when its Tokio, `libc`, `socket2` and `mio` patches are upstream (§9's decision rule); why: no patch fork to carry | Founder | Resolved |
| Q-CF-8 | ~~Does `object_store` (with its `aws` feature) build and work on `wasm32-unknown-unknown` inside a Worker, or does thin mode need an R2-binding `Store` adapter~~ Answered 2026-10-02 by the owner: the plan's default — not an owner decision; CF1 Task 5 checks it; if `object_store` fails in a Worker, thin mode uses an R2-binding `Store` adapter | Eng | Resolved |

## 12a. Birthday Week 2026 betas (added 2026-10-02, verified 2026-10-02)

Owner direction of 2026-10-02: "search the latest beta features in the Cloudflare blog and use them." Open adapters are in operon §42; this section is the hosted side. Every row was read on Cloudflare's own page on the date above; **beta** means Cloudflare calls it beta, and none of it may be on a customer's critical path until it is GA or the exit is tested.

| Feature (source) | Status | Use in the Cloudflare target | Decision |
|---|---|---|---|
| **Containers: faster starts** ([blog 2026-09-30](https://blog.cloudflare.com/faster-agent-sandboxes/)) | Median start 648 ms (p95 910 ms, p99 1,129 ms), down from just over 4 s. GA product; the numbers are Cloudflare's own | Revises "cold start 1 to 3 s" in doc 04 §2. Still not the query path, now plausible for per-request tenant `http-port` functions | PD73 |
| **`durable_object` scheduling policy** (same post) | **Public beta**: pick the runtime image and instance type at start, per Durable Object, instead of one application per combination | Hosted Functions (doc 04 PD33): one Container application, the instance type chosen per function from the plan | PD73 |
| **Filesystem snapshots** (`ctx.container.snapshotContainer`, `start({ containerSnapshot })`) | **Public beta**. Doc 04 recorded snapshots up to 20 GB kept 30 days; the post adds the API for pause and resume | Factory agent sandboxes (operon §39 SF4): a coding workspace pauses between turns and resumes from its snapshot; compute billing stops once the container has slept, but snapshot-storage pricing is unconfirmed (Q-CF-10) | PD74 |
| **`cloudflare/debian-trixie` image** (same post) | Debian Trixie Slim with Node.js 24.20.0 LTS; saves writing a Dockerfile | Base for sandboxes that only need git, a shell and Node; the factory still pins its own images for Rust and Python | PD74 |
| **`Container` and `Sandbox` classes deprecated** (same post) | Maintained **through 2026-12-31**, no updates after; migrate to `this.ctx.container` | CF1 and `CloudflareRunner` native mode (§5.3) must target `ctx.container` from the start. No code of ours uses the old classes yet (checked 2026-10-02) | PD75 |
| **Dynamic Workers** ([blog 2026-03-24](https://blog.cloudflare.com/dynamic-workers/)) | **Open beta**, Workers Paid. `env.LOADER.load()` starts a Worker from code at runtime; $0.002 per unique Worker loaded per day, waived in beta | The Cloudflare-target tier for untrusted agent-generated code (operon §24): millisecond start, V8 isolate, no container. It complements Workers for Platforms (PD33): WfP for tenant-deployed functions, Dynamic Workers for code generated at run time | PD76 |
| **Durable Object Facets** ([blog 2026-04-13](https://blog.cloudflare.com/durable-object-facets-dynamic-workers/)) | **Open beta**, Workers Paid; each facet has its own SQLite database inside the supervisor Durable Object | A database per generated app (the Software Factory's generated apps, doc 05); the supervisor Durable Object meters and logs before forwarding, which is exactly where `loam-usage` hooks in | PD76 |
| **Emscripten target for Rust Workers** ([blog 2026-09-28](https://blog.cloudflare.com/rust-workers-emscripten-target/)) | **First public experimental preview.** Tokio runs through JSPI or a `LocalEventLoop` runtime; single-threaded; some patches are not yet upstream; TCP and UDP sockets need `-sNODERAWSOCKETS` | Confirms `cf-native` mode (§5.3) is feasible and gives it a Tokio path; CF1 pins the patchset and treats it as experimental | PD77 |
| **Artifacts** ([blog 2026-10-01](https://blog.cloudflare.com/next-git-platform-on-cloudflare/), [docs](https://developers.cloudflare.com/artifacts/)) | **Open beta**, Workers Paid; $0.15 per 1,000 operations over 10,000 a month, $0.50 per GB-month over 1 GB; 1 GB per repository, 32 MB per blob, 1 TB per account, US or EU jurisdiction per namespace. Billing starts 2026-10-14 (docs) or 2026-10-15 (blog) | Hosted repositories for agent workspaces and Loams Git mirrors (doc 04 PD78). Not a source of truth: the bucket WAL is (operon §36, §42 D561) | PD78 |

**Decisions**

| # | Decision | Status |
|---|---|---|
| PD73 | **Hosted Functions on Containers use the `durable_object` scheduling policy** (beta) with faster starts, as a burst and edge option only; the Knative default stays | Proposed |
| PD74 | **Factory agent sandboxes on the Cloudflare target use Container filesystem snapshots** (beta) for pause and resume, behind the open `WorkspaceSnapshot` seam (operon §42 D574). The exit is a branch plus a volume snapshot on k3s | Proposed |
| PD75 | **Nothing is built on the deprecated `Container` and `Sandbox` classes**: CF1 and the native runner use `this.ctx.container` | Proposed |
| PD76 | **Untrusted agent code on the Cloudflare target runs in Dynamic Workers; generated apps get a database through Durable Object Facets** (both beta). The open tier is workerd under gVisor | Proposed |
| PD77 | **Rust on Workers via Emscripten is experimental**; CF1 gets a Tokio task and pins the upstream patchset | Proposed |
| PD78 | **Artifacts is a hosted repository service for agent workspaces and mirrors**, with one namespace per org (jurisdiction from the org's region), tokens issued through the credential broker, never a WAL or ref store | Proposed |

**Owner actions** (only the owner can do these): confirm the Workers Paid subscription is active (Containers, Dynamic Workers, Facets and Artifacts all need it; Artifacts billing starts 2026-10-14); read the Artifacts, Dynamic Workers and Containers snapshot pricing pages before any paid use.

**Open questions**

| # | Question | Owner | Needed by |
|---|---|---|---|
| Q-CF-10 | Pricing of paused container snapshots and of snapshot storage (not stated in the 2026-09-30 post) | Eng | CF1 |
| Q-CF-11 | Dynamic Workers pricing after the beta waiver, and any CPU-time ceiling per loaded Worker | Eng | Before the factory beta |
| Q-CF-12 | Does Artifacts offer atomic multi-ref updates, and which Git protocol versions? (Not stated) | Eng | Hosted mirror |

## 13. Sources

Read on 2026-10-01 unless noted.

- The owner's draft "Loam Serverless Runtime — Consolidated Plan" (2026-09-30), §2, §4.4, §13 and §15 (`chatdump.md`).
- Repository: operon §01, §15, §18 §5.3 (D75), operon §24 (§2.2, §4, §9, Q-RT-3, Q-RT-4), operon §25 §5 (`ObjectStoreProvider`), operon §27 (D201), `docs/open-core.md` (D220), `crates/operon-store`.
- Cloudflare blog: "Supporting native Rust in Workers with the new Emscripten target for wasm-bindgen", https://blog.cloudflare.com/rust-workers-emscripten-target/ (2026-09-28); workers-rs https://github.com/cloudflare/workers-rs (Apache-2.0, v0.8.7, 2026-09-25; `examples/emscripten`, `examples/emscripten-tokio`, `examples/emscripten-tcp`; PR #1061); `worker-fs-mount` https://github.com/danlapid/worker-fs-mount; the Pumpkin demo https://github.com/danlapid/rust-workers-minecraft.
- Cloudflare docs: Workers limits https://developers.cloudflare.com/workers/platform/limits/ (2026-09-05); Workers pricing https://developers.cloudflare.com/workers/platform/pricing/ (2026-08-28); Durable Objects pricing (2026-09-30), limits (2026-06-01) and "What are Durable Objects" (2026-07-15); R2 S3 API (2026-07-31), Workers API reference (2026-07-31), limits (2026-06-08), pricing (2026-10-01), consistency, tokens (2026-10-01) and the API `POST /accounts/{account_id}/r2/temp-access-credentials`; Containers limits (2026-09-30) and pricing; D1 limits (2026-04-21); Workers for Platforms pricing and "How Workers for Platforms works"; changelog "CPU time and Wall time now published for Workers Invocations" (2025-04-09) and Workers Trace Events fields.
- Cloudflare for Startups https://www.cloudflare.com/forstartups/ (no date); the older programme in https://blog.cloudflare.com/th-th/startup-program-250k-credits.
- gitoxide `.github/workflows/ci.yml` (`wasm` job), read on `main`.
