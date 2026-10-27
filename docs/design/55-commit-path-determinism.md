# 55 — Commit-Path Determinism: What Loams Takes from "Loam Io"

> **Updated 2026-10-19 ([D1119](13-decision-log.md)): Loams has no open core.** Where this document places metering, billing, the hosted control plane, commercial APIs or other components in the private `loams-platform` repository, or calls something private for open-core reasons, read that as superseded: everything is Apache-2.0, and the metering, billing and control-plane code is moving into the open repo now (design §63, Loams Meter). Ostrium Labs offers services (infrastructure efficiency, SLAs, support), not a paid edition. The rest of the design is unchanged.

Status: **Approved direction** (owner, 2026-10-15) · details **Proposed** · 2026-10-15. Source: the external proposal "Loam Io", which the owner brought on 2026-10-15, the controller's review of it, and the owner's ruling on that review (§1.1), which is binding. The pilot runs **after v0.1.0-preview.3** (the upgrades release, §54). Decisions **D946–D962** and open questions **Q835–Q842** are declared in the [decision log](13-decision-log.md). Rows that restate the owner's direction are *Approved*; rows that add detail to it are *Proposed* until the owner confirms them. Plan: [DT1](../plans/2026-10-15-dt1-commit-path-determinism.md). **No code is written by this document.**

This document **amends D28** ([§12](12-roadmap-testing-risks.md) §2 item 1, the engine's seeded simulation in `crates/loams-sim`) and extends **D313** ([§31](31-loams-router-and-verification.md) §7.1 and §13, bit-exact simulation of sans-I/O machines with `loams-detsim`) from the router's control plane to the engine's commit paths (§5). It builds on D314 (shared checkers), the `FaultyStore` of [M0.1](../plans/2026-09-23-m0.1-storage-primitives.md) and the open-core rule that nothing billing-grade lives in this repository (D541, D552).

Markers: **(verify)** means not checked against a primary source; the plan task that depends on the fact checks it first. **(estimate)** means computed or judged, not measured. **(target)** is a number this document sets as a gate.

---

## 1. Summary

### 1.1 The owner's direction (2026-10-15, binding)

1. **"Loam Io" is not adopted as written.** It conflicts with D28 and D313, it is too large (about a thousand ambient call sites, and dependencies that cannot be made bit-exact), and its Workers premise is weak: the engine does not run on Workers; only the console does.
2. **Adopt five parts, scoped narrowly:**
   1. **Clock and Entropy injection, with scoped lint bans, on the commit-path crates only** (manifest commits, leases, journals, Live's commit journal, Graph ownership), moving that code toward sans-I/O and reusing `loams-detsim`.
   2. **Cancel-safety contracts** (`cancel-safe` / `cancel-unsafe, use cancel().await` / `atomic-commit`) on commit and recovery paths, plus a **cancel-at-every-await sweep** on the WAL and manifest commits.
   3. **Stronger `FaultyStore` models:** stale list-after-put, torn unsynced disk writes on a simulated crash, per-key versions and conditional-write races.
   4. **A `TaskSet` helper** (fail-fast, and first-ok for hedged reads) over tokio's `JoinSet`, plus a **`spawn_concurrent`** that fails loudly, for liveness loops.
   5. **CPU-time against wall-time accounting** for the optimizer. No billing-grade metering in the open repository.
3. **Skip:** the engine-wide `HashMap` ban, a Workers backend, io_uring, and threading `&Io` through every function.
4. **Pilot first** (2026-10-15): `loams-log`'s WAL and manifest commit path, after v0.1.0-preview.3. A **measured gate** (did the pilot find a bug, or prove a property the current simulation cannot?) decides whether the rollout to the other commit-path crates goes ahead.

### 1.2 Decisions

| # | Decision | Status | § |
|---|---|---|---|
| D946 | **"Loam Io" is not adopted as written.** Its capability handle threaded through every engine function, its engine-wide bit-exact backend (`loam-io-sim`), its tokio and Workers backends and its engine-wide lint bans conflict with D28 and D313, would touch about 800 production call sites across 60 crates (§3.2), and rest on a premise (the engine on Workers) that does not hold. Five parts are adopted, narrowly (D947–D951) | Approved (owner) | §3, §4 |
| D947 | **Commit-path determinism; amends D28.** Code on the commit paths (manifest commits, leases, journals, Live's commit journal, Graph ownership) reads time and randomness only through an injected `Clock` and `Entropy`, moves its decisions into sans-I/O machines, and is simulated bit-exactly with `loams-detsim` (D313's tier 1, extended from the router's control plane to these machines). Lint bans hold the line in those modules only. The rest of the engine stays under D28's seeded simulation | Approved (owner) | §5, §6 |
| D948 | **Cancel-safety contracts on commit and recovery paths.** Every async function there declares `cancel-safe`, `cancel-unsafe, use cancel().await` or `atomic-commit`; the WAL commit and the log's segment swap (its manifest commit) get a cancel-at-every-await sweep | Approved (owner) | §7 |
| D949 | **Stronger `FaultyStore` models:** stale list-after-put, torn unsynced disk writes on a simulated crash, per-key versions and conditional-write races | Approved (owner) | §8 |
| D950 | **`TaskSet` and `spawn_concurrent`:** a helper over tokio's `JoinSet` with fail-fast and first-ok (hedged reads) policies, and a spawn for liveness loops that fails loudly when the loop cannot run or ends unexpectedly | Approved (owner) | §9 |
| D951 | **CPU-time and wall-time accounting for the optimizer**, as generic observability; nothing in it is billing-grade or a source of truth for a charge (D541, D552) | Approved (owner) | §10 |
| D952 | **Not adopted:** the engine-wide `HashMap` ban, a Workers backend, io_uring through this proposal, and threading `&Io` through every function | Approved (owner) | §4 |
| D953 | **Pilot first, after v0.1.0-preview.3:** `loams-log`'s WAL commit and segment swap. A measured gate (D958) decides whether the rollout (D962) goes ahead | Approved (owner) | §11 |
| D954 | **The seam.** `loams_common::det` holds `Clock` (`now_ms`, wall; `mono`, monotonic), `Mono`, `Entropy`, `SystemClock` and `ManualClock`, with no new dependency. A new crate `loams-task` holds `TokioClock` (monotonic time from `tokio::time::Instant`, so paused test time drives it) and `OsEntropy`. `Machine`, `Ctx`, `Millis`, `TraceSink` and `SpecEvent` move from `loams-sqlrouter` to `loams_common::machine` and are re-exported there unchanged (Q836). `loams_meta::Clock` and `loams_git::Clock` become adapters during the rollout. Wall time that the metastore checks (ULID times, `created_at_ms`, freshness) keeps coming from `MetaStore::now_ms` | Proposed | §6.1 |
| D955 | **The pilot's machines in `loams-log`:** `writer::policy::Buffer` (when to flush), `commit::CommitRetry` (attempt, back off, classify the outcome), `fence::partition` (which appends a broken fence fails) and `segmenter::swap::Decision` (swap, skip, delete or leave the segment). The tokio driver executes their outputs; `select!` on the commit path is `biased;`; WAL and segment ULIDs take their random part from `Entropy`. In the rollout one `Retry` machine replaces the hand-written deadline-and-backoff loops (the writer's, `MetaClient`'s two, the state machine's snapshot upload) | Proposed | §6.2 |
| D956 | **Contract syntax and the sweep.** A `# Cancel safety` doc section whose first word is the tag, checked by `scripts/ci/cancel-tags.py` on the modules listed in `scripts/ci/commit-path.toml`. `loams_detsim::cancel::{CancelAfter, sweep}` drops a future at its k-th suspension for every k, runs recovery, and checks that the system is in one of the end states the tag allows | Proposed | §7 |
| D957 | **Model shapes.** `FaultyStore::set_list_view(ListView::{Fresh, Stale { lag }, Moving})`; `Fault::Race(RaceWrite)` applies a competing write before a conditional PUT, so the 412 or 409 is real; `FaultyStore::history(path)` records per-key versions; `loams_store::disk::TornDisk` (feature `torn-disk`) is a `redb` storage backend that keeps unsynced writes apart and drops or tears them on `crash(seed)`, which `loams-meta` uses for its Raft log through a test-only backend hook (Q838) | Proposed | §8 |
| D958 | **The gate.** Go when the pilot (a) finds at least one defect the current suites missed, or (b) proves at least two of P1–P4 (§11.3), and in either case stays inside the cost budget (§11.4). Otherwise the pilot's code stays, the rollout does not start, and D947's scope shrinks to `loams-log`. The owner rules on the gate report (Q835) | Proposed | §11.4 |
| D959 | **Lint scoping.** A scoped crate's `clippy.toml` lists the ambient methods under `disallowed-methods` and repeats the workspace's settings. A crate whose commit path is a few modules sets `#![allow(clippy::disallowed_methods)]` at its root and `#![deny(clippy::disallowed_methods)]` in those modules. Driver sites carry `#[expect(clippy::disallowed_methods, reason = "…")]`, counted by a ratchet in `Repo: checks` | Proposed | §6.4 |
| D960 | **`loams-task`** (new crate: tokio, tokio-util, tracing): `TaskSet::{fail_fast, first_ok}`, `hedge`, and `spawn_concurrent`, which returns a `Supervised` handle with `stop().await`. An unexpected exit is logged at error, counted (`loams_task_unexpected_exit_total{task}`) and handed to the owner's `on_exit`; with `debug_assertions` it also panics (Q840) | Proposed | §9 |
| D961 | **Accounting shape.** `loams_task::cpu::{Accounted, accounted_blocking}` read the thread's CPU clock (`rustix`, `CLOCK_THREAD_CPUTIME_ID`) around each poll or closure; Loams' DataFusion operators report `cpu_compute` beside `elapsed_compute`; OTel histograms are labelled by operator kind only, with no namespace or tenant label; the optimizer uses them first in an offline calibration report of its static thresholds, not for online tuning (Q839) | Proposed | §10 |
| D962 | **Rollout order after a go:** collection, link and hot-tier manifest commits; `loams-meta`'s client retries and lease commands; `loams-worker`'s leases; Graph ownership; Git's ref log; Live's commit journal; the safekeeper's NVMe journal | Proposed | §11.5 |

### 1.3 What it looks like

```
                 loams-log commit path (pilot)                        test harnesses
 ┌──────────────────────────────────────────────────────────┐
 │ sans-I/O machines (no clock, no RNG, no I/O, no spawn)   │   loams-detsim (D313, RT1):
 │   Buffer · CommitRetry · fence::partition · swap::Decision│◄─ one ChaCha8Rng, simulated time,
 │        ▲ inputs: results, timers, Ctx{now, rng, trace}   │   network and store models,
 │        │ outputs: Put, CommitWal, SwapSegment, SleepUntil │   trace hash, shrinking      → P1, P2
 ├────────┼─────────────────────────────────────────────────┤
 │ tokio driver: executes outputs, feeds results back        │   loams_detsim::cancel::sweep:
 │   Clock (TokioClock) · Entropy (OsEntropy) injected       │◄─ drop at the k-th suspension,
 │   flush task via spawn_concurrent (D960)                  │   recover, check end states  → P3
 └────────┬───────────────────────────────┬─────────────────┘
          │ Store (object_store)          │ MetaStore (openraft + redb)
          ▼                               ▼
   FaultyStore: list views, races,   TornDisk (redb backend): unsynced
   per-key history (D957)            writes dropped or torn on crash → P4
                                     loams-sim (D28): seeded, real I/O, unchanged
```

---

## 2. The proposal

### 2.1 What "Loam Io" proposed

The proposal adapts Zig 0.16's `std.Io` to Rust (verify: Zig 0.16 passes an `Io` interface explicitly, like an allocator, with `async`, which may run a call eagerly, and `concurrent`, which fails with `error.ConcurrencyUnavailable` when the implementation cannot run it concurrently; futures are awaited or cancelled explicitly). For Loams it proposed:

1. **A capability handle in every engine function:** `Clock`, `Executor`, `BlobStore`, `Disk`, `Net` and `Entropy`, passed as `&Io`.
2. **Backends:** a deterministic `loam-io-sim`, a tokio backend, and a Cloudflare Workers backend.
3. **Structured tasks:** `Task` and `TaskSet` with `spawn` (may run inline) against `spawn_concurrent` (must run concurrently).
4. **Cancel-safety contracts** on async functions.
5. **Enforcement:** clippy bans on ambient clocks, spawns, randomness and `HashMap` iteration, engine-wide.

### 2.2 What is right about it

The diagnosis is sound. Ambient time, ambient randomness and implicit cancellation are where distributed storage bugs hide, and a simulation is only as good as its control over them. TigerBeetle and FoundationDB get their results from exactly this discipline. What does not fit is the scope: Loams is built on libraries that do their own I/O, and D28 and D313 already decided where bit-exact simulation pays (§3.1).

---

## 3. Why not as written

### 3.1 It contradicts D28 and D313

- **D28** (2026-09-24, [§12](12-roadmap-testing-risks.md) §2 item 1): the engine's simulation is **seeded, not bit-exact**, because openraft, redb and `object_store` do real I/O and spawn blocking work; bit-exact replay would need a madsim port of every dependency, and a seed sweep on every pull request finds the same classes of bugs.
- **D313** (2026-10-01, [§31](31-loams-router-and-verification.md) §13): bit-exact simulation is for **sans-I/O machines only**, driven by `loams-detsim`; madsim and turmoil are not used. §31 §13.1 keeps the engine in tier 2.

"Loam Io" would make the whole engine tier 1. That reverses both decisions for every crate, including those whose behaviour lives in third-party code (§3.3).

### 3.2 It is too large

Ambient call sites in production code, measured on `dev` at `f1283fec` (2026-10-15) over the 60 crates under `crates/` (`loams-wal-decoder` included; `bench/git-wal` excluded): every `.rs` file under `src/`, with `#[cfg(test)] mod … { … }` blocks and `//` comment lines removed. The method is DT1 Task 0's script (`scripts/ci/ambient-inventory.py`), which the plan checks in.

| Call | Production sites | Raw `grep` under `crates/*/src` (inline tests included) | Raw `grep` under `crates/` (all tests) |
|---|---|---|---|
| `Instant::now` | 353 (plus 103 `.elapsed()`) | 407 | 883 |
| `tokio::time::sleep` | 173 (plus 28 `sleep_until`, 224 `timeout`/`interval`) | 226 | 620 |
| `tokio::spawn` | 203 (plus 86 `spawn_blocking`) | 245 | 472 |
| `SystemTime::now` | 41 | 55 | 88 |
| `thread::spawn` | 5 (plus `thread::Builder` sites, which Task 0 counts) | 12 | 39 |
| Ulid/Uuid/`rand` ambient randomness | 95 | — | — |
| **Total of the first five rows** | **775** | **945** | **2 102** |

The controller's review quoted 575, 304, 289, 92 and 13 (about 1 270); those figures do not reproduce at `f1283fec` with either method above (they fall between the production and the all-tests counts), so this document uses its own. The conclusion is the same: threading `&Io` through the engine means changing on the order of a thousand sites, plus every signature between them.

### 3.3 The dependencies are not deterministic

The engine's heavy dependencies do their own I/O, spawn their own work and read their own clocks. Counts of `tokio::spawn`, `spawn_blocking`, `thread::spawn`/`thread::Builder`, `Instant::now` and `SystemTime::now` in each crate's `src/` (cargo registry sources, the versions in `Cargo.lock`; tests inside `src/` included, so these are upper bounds):

| Dependency (version) | Spawns | Clock reads | Notes |
|---|---|---|---|
| `lance` 12.0.0 | 62 `tokio::spawn`, 16 `thread::spawn`, 8 `spawn_blocking` | 55 `Instant::now`, 4 `SystemTime::now` | Collections' storage |
| `object_store` 0.14.2 | 3 `tokio::spawn`, 27 `spawn_blocking`, 2 `thread::spawn` | 28 `Instant::now`, 4 `SystemTime::now` | Every bucket access |
| `openraft` 0.10.0-alpha.34 | 4 `spawn_blocking`, 1 `thread::spawn`, plus its runtime's spawns (feature `tokio-rt`) | 3 `Instant::now`, 3 `SystemTime::now` | The embedded metastore |
| `tantivy` 0.26.2 | 5 `thread::Builder`, 2 `thread::spawn`, 4 `spawn_blocking` | — | Text search |
| `datafusion-common-runtime` 54.1.0 | 12 `spawn_blocking` | — | Query execution |
| `foyer` 0.22.6 | — | 8 `Instant::now` | The range cache |
| `redb` 4.3.0 | 14 `thread::spawn` | — | Real file I/O and `fsync` |

None of them takes an `Io` handle. A bit-exact engine would need forks of all of them or a madsim-style runtime replacement, which D28 and D313 already rejected.

### 3.4 The Workers premise does not hold

The engine does not run on Cloudflare Workers. Only the console does, as static assets on Workers Free (`web/apps/console/wrangler.jsonc`; [console deployment plan](../plans/2026-10-02-console-cloudflare-deploy.md): "Workers Free static assets only; no … Worker request code"). The only crate that targets `wasm32` is `loams-specview`, a frontend. A Workers backend for the engine has no user.

---

## 4. Adopted, rejected and why

| Proposal item | Verdict | Scope here | Why |
|---|---|---|---|
| Clock and Entropy in the capability handle | **Adopted, narrowly** (D947, D954) | Commit-path modules only (§5.2), injected at construction, not passed per call | That is where a wrong clock or a colliding id corrupts state, and where D313's sans-I/O style already works (`loams-sqlrouter`) |
| `Executor`, `BlobStore`, `Disk`, `Net` in the handle | **Rejected** (D952) | — | The machines have no I/O at all; their drivers already go through `Store` and `MetaStore`, which are the seams the models replace. A second abstraction over them adds nothing |
| `&Io` through every function | **Rejected** (D952) | — | §3.2, §3.3 |
| `loam-io-sim`, engine-wide | **Replaced** by `loams-detsim` for machines (D313) and `loams-sim` for the engine (D28) | §5 | One bit-exact simulator, not two |
| tokio backend | **Not needed** | — | Production stays tokio; only the machines are runtime-free |
| Workers backend | **Rejected** (D952) | — | §3.4 |
| io_uring | **Rejected here** (D952) | — | Not a determinism question. The safekeeper's journal already has an io_uring engine (feature `compio`, `crates/loams-safekeeper/src/journal/mod.rs`) where it pays |
| `Task`/`TaskSet`, `spawn` and `spawn_concurrent` | **Adopted** (D950, D960) | A helper crate, used where it fixes something | Liveness loops that die silently are a real failure mode (§11.2 H1) |
| Cancel-safety contracts | **Adopted** (D948, D956) | Commit and recovery paths | Tokio drops futures at any `.await`; on a commit path that is a correctness question |
| Engine-wide clippy bans | **Adopted, narrowly** (D959) | The commit-path modules | An engine-wide ban would be 775 `#[expect]`s |
| `HashMap` ban | **Rejected** (D952) | — | Iteration order matters only where it reaches a trace or a decision. Inside machine modules the review rule is `BTreeMap`/`BTreeSet` (the writer already uses them), and the trace-hash test (P1) catches a violation |
| CPU and wall time | **Adopted** (D951, D961) | Query operators and blocking work | Separates CPU-bound from I/O-bound work for the optimizer and EXPLAIN; not billing |
| Stronger fault models | **Adopted** (D949, D957) | `FaultyStore` and a `redb` backend | The current models cannot express stale listings, real races or lost unsynced writes (§8) |

---

## 5. Relation to D28 and D313

### 5.1 The amendment

D28 said the engine's simulation is seeded, not bit-exact. **D947 amends it:** the *decisions* on the engine's commit paths (when to flush, when to retry, how to classify an outcome, which appends a fence fails, whether a written segment may be deleted) move into sans-I/O machines that read time and randomness only from their `Ctx`, and those machines are simulated bit-exactly by `loams-detsim`, exactly as D313 does for the router. Everything else D28 covers stays as it is: `loams-sim` keeps running the whole engine (metastore, log, links, collections) seeded, with real openraft, redb and object-store I/O, on one current-thread runtime, with the same checks.

§31 §13.1's tiers become:

| Tier | What runs | Deterministic | Where |
|---|---|---|---|
| **1. Machine DST** (D313, **extended by D947**) | The router's machines (`loams-sqlrouter`), and the engine's commit-path machines (`loams-log` first), against models | Bit-exact: same seed, same trace hash | `crates/loams-detsim`; scenarios in each crate's `tests/sim/` |
| **1b. Commit-path sweeps** (new, D948, D949) | The real commit code (driver plus machines) over a single-node metastore and `FaultyStore`: cancel sweeps, torn-disk crashes, stale listings, races | Seeded; the sweep's cancel points are enumerated, not sampled | Each crate's `tests/` |
| **2. Engine simulation** (D28, unchanged) | Metastore, log, links, collections on one runtime with real I/O | Seeded, not bit-exact | `crates/loams-sim` |
| **3. Real-system nemesis** (D315) | Processes | No | `crates/loams-nemesis` (RT5) |

### 5.2 What "commit path" covers

The owner's list, mapped to code at `f1283fec`:

| Owner's term | Code | Commit point |
|---|---|---|
| Journals (the log's WAL) | `crates/loams-log/src/writer.rs` (`write_and_commit`, `commit`) | `MetaStore::commit_wal` after a create-only PUT of the WAL object |
| Manifest commits (the log) | `crates/loams-log/src/segmenter.rs` (the swap in `SegmenterSource`'s task) | `MetaStore::swap_segment` after a create-only PUT of the segment |
| Manifest commits (collections, links, hot tier) | `crates/loams-collection/src/{target.rs,index_build.rs,maintenance/merge.rs,maintenance/compaction.rs}`, `crates/loams-link/src/counter.rs`, `crates/loams-hot/src/build.rs` | Manifest PUT, then the fenced pointer CAS (failpoints `*.after_manifest_put`, `*.after_cas`) |
| Leases | `crates/loams-meta/src/client.rs` (retry loops), the lease commands in `crates/loams-meta/src/state/`; `crates/loams-worker/src/{worker.rs,runner.rs}` | Lease commands through Raft; the worker's renew loop |
| Graph ownership | `crates/loams-graph/src/owner.rs`, `crates/loams-graph/src/durable/{append.rs,lane.rs,mod.rs}` | The owner lease and fenced appends (D744) |
| Journals (Git) | `crates/loams-git/src/{sequencer.rs,owner.rs,retry.rs,wal.rs,reflog.rs}` | The create-only PUT of the next ref-log segment (D389) |
| Live's commit journal | `crates/loams-live/src/journal.rs` | The journal entry and head in the mutation's TiKV transaction |
| Journals (the safekeeper) | `crates/loams-safekeeper/src/journal/{mod.rs,segments.rs}` | `fdatasync` or `RWF_DSYNC` of a flush unit |

The pilot is the first two rows. The rest is the rollout (D962), after the gate.

### 5.3 What does not change

`loams-sim`'s checks, its PR budget (32 seeds × 300 steps) and its nightly budget (1 000 seeds) in `ci.yml`'s `sim` job; the kill -9 crash gate (`crates/loams/tests/crash.rs`); the failpoints (`fail` crate, feature `failpoints`); the metastore conformance suite and fault matrix. D313's rejection of madsim and turmoil stands, and so does D314: checkers live in `loams-detsim::checkers`.

---

## 6. Clock and Entropy injection (D947, D954, D955, D959)

### 6.1 The seam

Three clock seams exist today, and none is monotonic:

- `loams_meta::Clock` (`crates/loams-meta/src/clock.rs`): `now_ms() -> u64`, with `SystemClock` and `ManualClock`; `MetaConfig::clock` and `MetaClient` take it, and `MetaStore::now_ms` exposes the proposer's clock.
- `loams_git::Clock` (`crates/loams-git/src/clock.rs`): `now_unix_ms() -> i64`, with `ManualClock` in `crates/loams-git/src/testing.rs`.
- `loams_sqlrouter::machine::Ctx` (`crates/loams-sqlrouter/src/machine.rs`): `now: Millis`, `rng: &mut dyn rand_core::Rng`, `trace: &mut dyn TraceSink`, for machines.

D954 adds one shared seam without new dependencies in `loams-common`:

```rust
// loams_common::det (std only)
pub struct Mono(pub u64);                         // ns since the clock's origin
pub trait Clock: Send + Sync + std::fmt::Debug {
    fn now_ms(&self) -> u64;                      // wall, ms since the Unix epoch
    fn mono(&self) -> Mono;                       // monotonic: deadlines, intervals, backoff
}
pub trait Entropy: Send + Sync + std::fmt::Debug {
    fn fill(&self, dest: &mut [u8]);
    fn u128(&self) -> u128 { /* from fill */ }
}
pub struct SystemClock;                           // std::time
pub struct ManualClock;                           // set and advance both clocks by hand

// loams_common::machine — moved from loams-sqlrouter, re-exported there unchanged
pub struct Millis(pub u64);
pub struct Ctx<'a> { pub now: Millis, pub rng: &'a mut dyn rand_core::Rng, pub trace: &'a mut dyn TraceSink }
pub trait Machine { type Input; type Output; fn on(&mut self, ctx: &mut Ctx<'_>, input: Self::Input) -> Vec<Self::Output>; }

// loams_task (new crate, tokio)
pub struct TokioClock;   // mono() from tokio::time::Instant: paused test time moves it
pub struct OsEntropy;    // rand's thread RNG
```

Rules:
- **Machines** read time and randomness only from `Ctx`. `loams-common` gains `rand_core` 0.10, which `loams-sqlrouter` already uses; that keeps `loams-sqlrouter`'s kernel rule (`kernel_crate_has_no_io_dependencies`) true.
- **Drivers** hold an `Arc<dyn Clock>` and an `Arc<dyn Entropy>`, injected when the component is built (`LogWriter::start_with(…, LogIo { clock, entropy })`; `LogWriter::start` keeps its signature and uses `TokioClock` and `OsEntropy`).
- **Wall time the metastore checks stays the metastore's.** ULID times, `created_at_ms` and freshness stamps keep coming from `MetaStore::now_ms`, as today (`writer.rs:378`, `:645`; `segmenter.rs:176`, `:328`, `:340`), because the metastore compares them with its own clock (D59's bounded skew). `Clock::now_ms` is for everything else.
- **Moving `Ctx`** also lets `loams-detsim` depend on `loams-common` instead of `loams-sqlrouter` for `TraceSink` (RT1 Task 1 names `loams_sqlrouter::TraceSink` in `NodeCx`).

### 6.2 The pilot's machines

`crates/loams-log/src/writer.rs` mixes four decisions with its I/O. Each becomes a machine or a pure function; the driver keeps the tokio code:

| Machine | Today | Inputs | Outputs | Properties checked |
|---|---|---|---|---|
| `writer::policy::Buffer` | `next()` and the requeue in `run()` (`writer.rs:503–585`), reading `Instant::now` at `:412`, `:522`, `:573` | Append buffered, flush requested, flush finished (with requeued appends), close, timer | `Flush(job)`, `WaitUntil(Mono)`, `Wait`, `Exit` | No buffered append waits longer than `flush_interval` unless a flush is in flight; byte accounting returns to zero; requeued appends go out before later ones; nothing is answered twice |
| `commit::CommitRetry` | `commit()` (`writer.rs:722–785`), reading `Instant::now` at `:729`, `:775` and sleeping at `:782` | Start, attempt result (`Committed`, `Rejected`, `ClockSkew`, `Retryable`, `Fatal`, each with `earlier_unknown`), timer | `CommitWal`, `SleepUntil(Mono)`, `Done(Result<…, FlushFailure>)` | A rejection or skew is reported as definite only when no earlier attempt's outcome was unknown; one attempt in flight at most; no attempt after the deadline |
| `fence::partition` | The `Fenced` arm of `flush()` (`writer.rs:602–629`) | The flush's appends, the refused lease, its current epoch (or none) | Failed appends, requeued appends | Only appends under the refused lease at a stale epoch fail; every other append is requeued, in order; an unreadable lease fails every append under it |
| `segmenter::swap::Decision` | The tail of the segmenter's attempt (`segmenter.rs:326–392`) | Written-at time, now, cancelled, swap result with `earlier_unknown` | `Swap`, `Skip { delete }`, `Fenced { delete }`, `Unknown` | A segment is deleted only when no swap of it can have been applied |

In the rollout, `CommitRetry` generalises into one `Retry` machine for the other deadline-and-backoff loops (`crates/loams-meta/src/client.rs:241–293` and `:395–432`, `crates/loams-meta/src/state_machine.rs:310–342`).

The driver changes are small: `tokio::select!` at `writer.rs:547` becomes `biased;` (an unbiased `select!` draws from tokio's own RNG); the ULIDs at `writer.rs:645` and `segmenter.rs:329` take their random part from `Entropy` instead of `Ulid::generate()`; the flush task (`writer.rs:260`) is started with `spawn_concurrent` once `loams-task` has it (§9).

Injected entropy also makes rare events forceable: two writers with the same `node_id` and a seeded `Entropy` that repeats itself produce the same WAL path, so the create-only PUT race (§8.2) can be tested on purpose rather than hoped for.

### 6.3 What the pilot does not change

The reader's long-poll (`reader.rs:159`, `:186`), retention's and GC's run intervals (`retention.rs:195`, `:211`; `gc.rs:329`, `:345`) are not commit decisions, but the crate-level ban (§6.4) covers them, so the pilot routes them through the injected `Clock` too. They stay in the driver; no machine is extracted for them.

### 6.4 Scoped lint bans (D959)

`loams-sqlrouter` already does this (`crates/loams-sqlrouter/clippy.toml`). Clippy reads `clippy.toml` from the crate's directory, and a crate-level file replaces the workspace's, so each scoped crate's file repeats the workspace settings (`allow-unwrap-in-tests`, `allow-expect-in-tests`) and adds:

```toml
disallowed-methods = [
  { path = "std::time::Instant::now",       reason = "use the injected Clock (§55 §6)" },
  { path = "std::time::Instant::elapsed",   reason = "use the injected Clock" },
  { path = "std::time::SystemTime::now",    reason = "use the injected Clock" },
  { path = "tokio::time::Instant::now",     reason = "use the injected Clock" },
  { path = "tokio::time::sleep",            reason = "machines return SleepUntil; drivers carry #[expect]" },
  { path = "tokio::time::sleep_until",      reason = "as above" },
  { path = "tokio::task::spawn",            reason = "use loams_task::spawn_concurrent or TaskSet" },
  { path = "std::thread::spawn",            reason = "as above" },
  { path = "ulid::Ulid::new",               reason = "take the random part from Entropy" },
  { path = "ulid::Ulid::generate",          reason = "as above" },
  { path = "rand::rng",                     reason = "use the injected Entropy", allow-invalid = true },
  { path = "rand::thread_rng",              reason = "as above", allow-invalid = true },
]
```

Whether `tokio::spawn` must be named as `tokio::task::spawn` (its definition) is checked in DT1 Task 3 (verify).

- **Whole crate:** `loams-log` (pilot). Its integration tests (`tests/it/main.rs`) carry `#![allow(clippy::disallowed_methods)]`.
- **Some modules:** a crate whose commit path is a few modules (`loams-graph`, `loams-live`, `loams-meta`, `loams-safekeeper`) gets the same `clippy.toml`, `#![allow(clippy::disallowed_methods)]` at its root and `#![deny(clippy::disallowed_methods)]` in the commit-path modules. CI already runs clippy with `-D warnings` (`Rust: fmt and clippy`), so nothing new runs.
- **Drivers** keep their few ambient calls with `#[expect(clippy::disallowed_methods, reason = "driver: executes SleepUntil")]`. `scripts/ci/ambient-ratchet.py` counts those per scoped module against `scripts/ci/commit-path.toml` and fails when a count rises or a listed module loses its `deny`.

---

## 7. Cancel-safety contracts (D948, D956)

### 7.1 The three tags

Tokio cancels a future by dropping it at whichever `.await` it is suspended in: a `select!` branch losing, a `timeout`, a client disconnecting, a task aborted. On a commit path that can leave half an effect.

| Tag | Meaning | A caller may |
|---|---|---|
| `cancel-safe` | Dropping it at any suspension leaves no effect that a retry does not repair (reads; idempotent writes keyed by the caller) | Use it in `select!`, `timeout`, hedging |
| `cancel-unsafe, use cancel().await` | Dropping it can leave a partial effect or detached work; it must be stopped through the named method, which awaits cleanup (`LogWriter::shutdown`, a worker task's `ctx.cancel` followed by awaiting the task) | Never drop it while running; call the named method |
| `atomic-commit` | It has exactly one commit point. Dropped before it: nothing committed (any object written is an orphan that GC reclaims). After it: committed. At it: unknown, and the named recovery decides (a retry of the same object id within `WAL_COMMIT_WINDOW_MS`, a linearizable read) | Drop it only if it handles "unknown" through the named recovery |

### 7.2 Syntax and the checker

Doc comments, not macros:

```rust
/// Writes `pending` as one WAL object and commits it.
///
/// # Cancel safety
///
/// atomic-commit: the commit point is `MetaStore::commit_wal`. Dropped before it, the WAL
/// object is an orphan (GC pass 2). Dropped during it, the outcome is unknown; a retry with the
/// same path within `WAL_COMMIT_WINDOW_MS` returns the first commit's offsets or is refused.
async fn write_and_commit(…)
```

`scripts/ci/cancel-tags.py` (with `--self-test`) reads `scripts/ci/commit-path.toml`, finds every `async fn` (and every `fn` returning `impl Future`) in the listed modules, and fails if one lacks a `# Cancel safety` section whose first word is `cancel-safe`, `cancel-unsafe,` or `atomic-commit:`. It runs in `Repo: checks`.

### 7.3 The sweep

`loams_detsim::cancel` (std only, no tokio):

```rust
pub struct CancelAfter<F> { /* the future, and how many Pending results remain */ }
pub enum Cancelled<T> { Completed(T), DroppedAt(u32) }
pub fn sweep<S, R>(points: Range<u32>, setup: impl FnMut() -> S, run: impl FnMut(&mut S, u32) -> R) -> SweepReport;
```

`CancelAfter(fut, k)` polls the inner future and drops it when it returns `Pending` for the k-th time, so each k is one suspension point actually reached. A sweep first runs uncancelled to count the points (K), then for every k in 1..=K builds a fresh system, runs the operation under `CancelAfter`, runs recovery (a new writer or segmenter over the same metastore and store; GC with the clocks advanced past its bounds), and checks the end state against the tag. The points come from real I/O (a single-node metastore on openraft and redb, a `FaultyStore` over `object_store`'s in-memory store), so K can differ between runs; the report lists the points reached, and the test requires every point of the uncancelled run to be covered.

Sweeps in the pilot:

| Operation | Tag | Allowed end states after recovery |
|---|---|---|
| `write_and_commit` (one flush) | atomic-commit | Nothing committed and the WAL object, if written, reclaimed by GC; or committed once, every partition of the object together, at dense offsets |
| `commit` (the retry loop) | atomic-commit | As above; a retry of the same path never commits twice |
| The segmenter's put-and-swap (`Segmenter::run_once`) | atomic-commit | The partition index names the old chunks or the new segment, never both or neither; fetch returns the same records; an unreferenced segment is reclaimed |
| `LogWriter::append_many` (caller side) | atomic-commit (detached) | The batches commit or not, all partitions together; the caller has no answer. A blind retry duplicates the records, which the doc now says |
| `LogWriter::shutdown` | cancel-unsafe, use cancel().await | n/a: the sweep checks that dropping it neither loses a buffered append nor hangs a later `flush` |

### 7.4 The tags for `loams-log`

| Function | Tag |
|---|---|
| `LogWriter::{append, append_fenced, append_many}` | atomic-commit (detached: the flush task commits after the caller is gone) |
| `LogWriter::flush` | cancel-safe (it only waits) |
| `LogWriter::shutdown` | cancel-unsafe, use cancel().await (it is itself the cancel) |
| `write_and_commit`, `commit` | atomic-commit |
| The segmenter's attempt; `Segmenter::run_once` | atomic-commit |
| `LogReader::fetch` | cancel-safe |
| Retention's `apply`; `Retention::run_once` | atomic-commit per partition (each `trim_partition` is fenced and independent) |
| GC's passes; `Gc::run_once` | cancel-safe (deletes are idempotent; forgetting is fenced and follows the deletes) |

---

## 8. `FaultyStore` models (D949, D957)

### 8.1 What exists

`crates/loams-store/src/fault.rs`: `FaultyStore` wraps an `ObjectStore` and injects queued or seeded random faults per `Op` (`Put`, `PutCreate`, `PutIfMatch`, `PutMultipart`, `Copy`, `Get`, `Delete`, `List`): `Error`, `ErrorAfterApply` (a lost acknowledgement), `Precondition` (a synthesised 412, or 409 after writing an absent object) and `Delay`, with a `ChaCha8Rng` seed. Listing faults are errors and delays only. There is no model of what a listing *shows*, no record of versions per key, no real competing writer, and no disk. `crates/loams-kv/src/embedded/faulty.rs` (feature `faults`) is the one disk fault model: a `redb` storage backend that fails the next n syncs.

### 8.2 The new models

| Model | API | What it represents | Pilot use |
|---|---|---|---|
| **Stale listing** | `set_list_view(ListView::Stale { lag })` | A listing omits objects created by the last `lag` mutations and still shows objects deleted by them. S3 has been strongly consistent for list-after-write since 2020; other S3-compatible stores may not be (verify per backend, Q841) | GC's passes list before they decide (`gc.rs` pass 1 forgets a retired prefix once a listing finds it empty; passes 2 and 3 list `wal/` and `ns/<ns>/streams/`) |
| **Moving listing** | `set_list_view(ListView::Moving)` | A paginated listing is not a snapshot on any provider: each page reflects the store at a different moment | Same; `Store::list_page` |
| **Per-key versions** | `history(path) -> Vec<VersionRecord { seq, etag, len, writer }>` | Which write produced which version, so a test asserts who won | All |
| **Conditional-write race** | `inject(op, Fault::Race(RaceWrite::{Create(bytes), Overwrite(bytes)}))` | A competing writer lands between the caller's read and its conditional PUT; the 412 or 409 the caller gets is real and the competing version is visible, unlike `Precondition`, which only pretends | The WAL's create-only PUT under a forced ULID collision (§6.2) |
| **Torn unsynced writes** | `loams_store::disk::TornDisk` (feature `torn-disk`, optional `redb` dependency): a `redb::StorageBackend` that keeps a durable image and an overlay of writes since the last `sync_data`; `crash(seed)` drops, keeps or tears each unsynced write at a 512-byte boundary | Power loss. **kill -9 cannot find a missing `fsync`**: the process dies but the page cache survives, so the crash gate (`abort()` and SIGKILL of `loams dev`) never loses an unsynced write | `loams-meta`'s Raft log (`crates/loams-meta/src/db.rs` opens `meta.redb`; `log_store.rs` acknowledges an append after `txn.commit()`), through a `test-util` hook `MetaConfig::db_backend` (Q838) |

`TornDisk` generalises `loams-kv`'s backend; DT1 moves that one onto it later only if it is a strict superset (it adds held committers and panics, which stay in `loams-kv`).

---

## 9. `TaskSet` and `spawn_concurrent` (D950, D960)

In `loams-task` (new; tokio, tokio-util, tracing):

```rust
pub struct TaskSet<T, E> { /* JoinSet, policy, names */ }
impl<T: Send + 'static, E: Send + 'static> TaskSet<T, E> {
    pub fn fail_fast() -> Self;           // join: every Ok in spawn order, or the first Err; the rest aborted
    pub fn first_ok() -> Self;            // first: the first Ok; the rest aborted; all Errs if none
    pub fn spawn(&mut self, name: &'static str, fut: impl Future<Output = Result<T, E>> + Send + 'static);
    pub async fn join(self) -> Result<Vec<T>, TaskSetError<E>>;
    pub async fn first(self) -> Result<T, TaskSetError<E>>;
}
pub enum TaskSetError<E> { Failed { name: &'static str, error: E }, Panicked { name: &'static str, message: String }, AllFailed(Vec<(&'static str, E)>) }
pub async fn hedge<T, E, F, Fut>(after: Duration, make: F) -> Result<T, E>;   // make(0) now, make(1) after `after` if no answer; first Ok wins
pub fn spawn_concurrent<E>(name: &'static str, make: impl FnOnce(CancellationToken) -> Fut, on_exit: impl FnOnce(Exit<E>) + Send + 'static) -> Result<Supervised, NoRuntime>;
pub struct Supervised; impl Supervised { pub async fn stop(self) -> Result<(), Exit<E>>; } // Drop without stop: abort, warn
```

- **`spawn` in a `TaskSet`** is structured: the set owns the tasks, dropping it aborts them, and a panic becomes `TaskSetError::Panicked` rather than a lost `JoinError`. Hedged reads use `first_ok` or `hedge`, and only on operations tagged `cancel-safe` (§7): the loser is dropped.
- **`spawn_concurrent`** is for loops that must keep running beside their owner (the log writer's flush task, lease renewals, heartbeats, watchers). It refuses to start without a runtime (`NoRuntime`, an error rather than a panic). If the loop returns or panics before `stop()` was called, that is an unexpected exit: logged at error with the task's name, counted in `loams_task_unexpected_exit_total{task}`, and passed to `on_exit`, which must make the owner fail visibly (for the writer: answer every waiter with an error and refuse new appends). With `debug_assertions` it then panics (Q840).
- **Zig's distinction, translated:** Zig's `async` may run a call inline and `concurrent` must run it concurrently or fail. Tokio always runs a spawned task concurrently on a live runtime; what goes wrong in Loams is a loop that stops without anyone noticing. So "fails loudly" here means at start (no runtime) and at an unexpected end.

Today eight non-test files under `crates/*/src` use `JoinSet` directly (in `loams-worker`, `loams-safekeeper`, `loams-pg-control`, `loams-sqlgate`, `loams-sim` and three agentd crates). They move only when a task touches them for another reason; there is no sweep.

---

## 10. CPU time against wall time (D951, D961)

Loams' own DataFusion operators already record `elapsed_compute` (`crates/loams-query/src/exec/{ann,sparse,tantivy_search,filter_bitmap}.rs`), which is wall time inside the operator, and the ANN strategy uses static thresholds (§05: brute force under `max(1 000, full_scan_threshold_kb · 1024 / (4 · dim))` rows, prefilter under 10 % of rows, otherwise post-filter). Wall time cannot tell a CPU-bound operator from one waiting on the bucket.

- **Measurement.** `loams_task::cpu::Accounted<F>` reads the polling thread's CPU clock before and after each `poll` (`rustix::time::clock_gettime(ClockId::ThreadCPUTime)`; the workspace forbids `unsafe`, and `rustix` 1 is already a workspace dependency, which gains its `time` feature) and adds the difference to a shared counter. Tokio tasks move between threads, but each poll runs on one, so per-poll deltas add up correctly. `accounted_blocking(f)` does the same around a `spawn_blocking` closure.
- **Gaps, stated.** CPU spent on threads Loams does not own (Lance's CPU pool, Tantivy's merge threads, DataFusion's spawned tasks outside the wrapper) is not attributed. Benchmarks bound it with the process's own CPU time (`getrusage(RUSAGE_SELF)` through `rustix`) around single-query runs.
- **Where it shows.** A `cpu_compute` metric beside `elapsed_compute` in each Loams operator (`EXPLAIN ANALYZE`); the slow log gains `cpu_ms` and `wall_ms` per query; OTel histograms `loams_query_operator_cpu_seconds` and `loams_query_operator_wall_seconds`, labelled by operator kind only.
- **What the optimizer does with it, first.** An offline calibration report (`loams-query`'s benchmark harness) that compares the static thresholds with measured CPU and wall cost per strategy, and proposes new defaults through ordinary review. Online tuning from these numbers is not in DT1.
- **Not billing.** These are generic observability under open-core's rules (D541, D552): no namespace or tenant label, no export path other than OTel and EXPLAIN, and nothing in them is a source of truth for a charge. `loams-platform` may read them as hints, like any other metric.
- **Cost.** One `clock_gettime` per poll costs a syscall on Linux (the thread CPU clock is not served by the vDSO; about 100–300 ns, estimate). Task 19 measures it; Q839 decides whether it is always on.

---

## 11. The pilot and its gate (D953, D958, D962)

### 11.1 Scope

After v0.1.0-preview.3 ships. `loams-log` only: the seam (§6.1), the four machines and their `loams-detsim` scenarios (§6.2), the crate-level lint ban (§6.4), the cancel tags and sweeps (§7), and the models the pilot needs (§8.2: stale and moving listings, per-key versions and races in `loams-store`; `TornDisk` with `loams-meta`'s hook). It needs `loams-detsim`'s scheduler, network model and fault points, which RT1 Tasks 1–2 specify and which **do not exist yet** (§12; Q837).

### 11.2 Hypotheses from reading the code

Read on `dev` at `f1283fec`; none has been run. The pilot tests each one and reports it as confirmed or refuted.

- **H1, the flush task dies silently.** If the writer's flush task (`writer.rs:260`, `run` at `:537`) ends by panicking, the waiters of the flush in flight are dropped and `flush()` maps the dropped channel to success (`rx.await.unwrap_or(Ok(()))`, `:459`; `shutdown()` at `:479`), so a caller is told that a failed flush succeeded. Later `append`s are buffered into a `State` that nothing drains and wait forever, and later `flush()` calls do too, because only a clean exit sets `stopped` (`:510`). What would panic is a bug elsewhere (an arithmetic overflow in debug builds, a panicking `MetaStore` or `Store` implementation); the point is that the writer does not fail visibly when it happens. Tests: `flush_task_panic_fails_waiters_not_ok`, `append_after_flush_task_panic_fails_fast`.
- **H2, a stale listing leaks a retired prefix.** GC pass 1 forgets a retired prefix once a listing of it comes back empty (`gc.rs`, the pass over retired paths before `:460`). Under a stale or moving listing, an object a straggling writer put under that prefix can be missed; pass 3 lists only `ns/<ns>/streams/` and registered roots' prefixes, so the object may never be reclaimed. A leak, not a loss. Test: `gc_reclaims_late_object_under_retired_prefix_with_stale_list`.
- **H3, dropping an append is not documented.** A caller that drops `append_many` after its batches are buffered gets no answer, and the batches commit anyway (`writer.rs:394–437`); a retry duplicates them. That is correct behaviour for `atomic-commit` with a detached commit, but nothing in the API says so. Not a defect; the tag documents it.

### 11.3 Properties the current simulation cannot show

| # | Property | How it is shown | Why `loams-sim` cannot |
|---|---|---|---|
| P1 | **Bit-exact replay** of the WAL commit decisions: the `wal_commit` scenario (writer machines, a model of `commit_wal` with its window, pruning and unknown outcomes, a store model, network drops and resets) gives the same trace hash twice for each of 10 000 seeds, and a failing seed shrinks | `loams-detsim`, `crates/loams-log/tests/sim/` | D28: a failing seed "may not replay exactly" |
| P2 | **Exhaustive outcome classification:** every sequence of up to six attempt results (five kinds, each with or without `earlier_unknown`) and every deadline position gives a correctly classified result; no attempt after the deadline; one in flight at most (about 10⁶ cases, estimate) | `CommitRetry` as a pure machine, enumerated | Random faults reach a small, uneven part of the space |
| P3 | **Cancel safety at every suspension point** of `write_and_commit`, `commit`, the segment swap and `append_many`: each ends in an allowed state (§7.3), with every point of the uncancelled run covered | `loams_detsim::cancel::sweep` over the real code | It never drops a commit future mid-flight |
| P4 | **No acknowledged append is lost to a power cut:** 1 000 seeded crashes of a single-node metastore whose `meta.redb` drops or tears every unsynced write lose no acknowledged append, and the node reopens or refuses to start with a corruption error, never serving a shorter log silently | `TornDisk` | It isolates metastore nodes but never loses their unsynced writes; the crash gate's kill -9 cannot either (§8.2) |

### 11.4 The gate (D958)

Measured at the end of the pilot (DT1 Task 8), written up as the plan's gate report, and ruled on by the owner (Q835):

- **Found a defect:** at least one test that fails on `dev` as of the pilot's start, showing lost or duplicated acknowledged data, a hang, a false success or a leak beyond GC's bounds, which `loams-sim` (PR and nightly), the crash gate and `loams-log`'s suites with failpoints did not catch (for nightly, no failure on it in the 30 nights before). H1 and H2 count if confirmed.
- **Or proved properties:** at least two of P1–P4 hold at the stated sizes.
- **And within budget (target):** C1, production lines changed in `loams-log` ≤ 800, tests excluded; C2, append throughput and p99 append latency within 2 % of Task 0's baseline (`writer_baseline`, median of five runs); C3, at most 3 minutes added to pull-request CI; C4, no new third-party crate beyond those RT1 already names for `loams-detsim`.
- **Go:** the rollout (§11.5) starts, task by task. **No go:** the pilot's code stays (it is small and tested), D947's scope is recorded as `loams-log` only, and DT1 Tasks 10–16 are not run. Tasks 18 (`TaskSet`) and 19 (accounting) do not depend on the gate.

### 11.5 Rollout order (D962)

| Order | Crate and modules | Why this order | Ambient sites in those files at `f1283fec` (lines matching) |
|---|---|---|---|
| 1 | `loams-collection` (`target.rs`, `index_build.rs`, `maintenance/{merge,compaction}.rs`), `loams-link` (`counter.rs`), `loams-hot` (`build.rs`) | The same put-manifest-then-CAS shape as the segment swap; one shared decision machine | 3 + 2 + 4 |
| 2 | `loams-meta` (`client.rs` retries, lease commands, `clock.rs`) | The source of `earlier_unknown` for every commit; `Retry` replaces its loops | 6 (`client.rs`) + 5 (`state_machine.rs`) |
| 3 | `loams-worker` (`worker.rs`, `runner.rs`) | Lease renewal loops are `spawn_concurrent`'s main users | 4 |
| 4 | `loams-graph` (`owner.rs`, `durable/*`) | Ownership is a lease plus fenced appends on the log (D744) | 3 + 4 |
| 5 | `loams-git` (`sequencer.rs`, `owner.rs`, `retry.rs`) | Already has a `Clock`; its ref log is a create-only-PUT journal | 8 + 10 + 2 |
| 6 | `loams-live` (`journal.rs`) | No ambient sites today (it uses TiKV timestamps); tags and sweeps against TiKV, nightly | 0 |
| 7 | `loams-safekeeper` (`journal/{mod,segments}.rs`) | Its own on-disk format with `fdatasync`; a torn-write model for its files, if it has a file seam (assessed first) | 5 + 2 |

---

## 12. Where things are today (checked on `dev` at `f1283fec`, 2026-10-15)

**`loams-log`'s ambient sites** (production code; Task 0 re-checks them):

| File:line | Call | Role |
|---|---|---|
| `writer.rs:260` | `tokio::spawn(run(…))` | Starts the flush task |
| `writer.rs:412`, `:573` | `Instant::now` (`get_or_insert_with`) | When the buffer opened |
| `writer.rs:522` | `Instant::now` | Is a flush due |
| `writer.rs:547–549` | `tokio::select!` (unbiased) with `sleep_until` | The flush task waits |
| `writer.rs:645` | `Ulid::generate()` | Random part of the WAL object's ULID |
| `writer.rs:729`, `:775` | `Instant::now` | Commit retry deadline |
| `writer.rs:782` | `tokio::time::sleep` | Commit backoff |
| `segmenter.rs:329` | `Ulid::generate()` | Random part of the segment's ULID |
| `reader.rs:159`, `:186` (`select!` at `:190`) | `Instant::now`, `sleep_until` | Long-poll deadline |
| `retention.rs:195`, `:211` | `.elapsed()`, `Instant::now` | Run interval |
| `gc.rs:329`, `:345` | `.elapsed()`, `Instant::now` | Run interval |

Sixteen sites in all: 8 `Instant::now`, 2 `.elapsed()`, 1 `sleep`, 2 `sleep_until`, 1 `tokio::spawn`, 2 `Ulid::generate`. Wall time already comes from `MetaStore::now_ms` at seven sites. The crate uses `BTreeMap`/`BTreeSet`, not `HashMap`, on the write path. Its failpoints are `wal.after_put`, `wal.after_commit`, `seg.after_put`, `seg.after_swap`, `retention.after_trim` and `gc.after_delete`.

**`loams-detsim` does not exist.** RT1 ([plan](../plans/2026-10-01-rt1-postgres-slice-and-sim.md)) is "In progress (2026-10-03: Task 0 reconciliation; Tasks 1–11 remain)"; its Tasks 1–2 specify the crate (scheduler, simulated time, `ChaCha8Rng`, trace hash, network model, fault points, swarm runs, shrinking), and no `crates/loams-detsim` is on `dev`. What exists of D313 is RT0's seam: `loams_sqlrouter::{machine, trace}` with its `clippy.toml` bans.

**`loams-sim`** (`crates/loams-sim/src/sim.rs`): a current-thread runtime, three metastore nodes over the in-process `Router` with redb in temp directories, log writers, a reader, a collection, a worker (segmenter, retention, links, GC), a `FaultyStore::random` store, meta-node isolation, worker restarts and store fault bursts; checks linearizability (WGL from `loams-meta-conformance`), exactly-once reads of acknowledged appends, the link's counters, metastore invariants, missing objects and the collection's fold. CI's `Engine: simulation` job runs 32 seeds × 300 steps on pull requests and 1 000 seeds nightly. It also holds `elle`, the list-append checker.

**The crash gate** (`crates/loams/tests/crash.rs`): `loams dev` as a child process, `abort()` at each named failpoint and a random-time SIGKILL loop, then restart and verify. It runs on the embedded metastore and, nightly, on TiKV.

**Prior art in the repository:** `loams-sqlrouter`'s machine seam and `clippy.toml`; `loams_meta::{Clock, ManualClock}`; `loams_git::Clock` and its `ManualClock`; `loams-kv`'s faulty `redb` backend; `fail` failpoints in `loams-log`, `loams-collection`, `loams-link`, `loams-hot`, `loams-meta` and `loams-graph`.

---

## 13. Reconciliation with earlier designs

| Earlier | What it says | How §55 fits |
|---|---|---|
| D28, §12 §2 item 1 | The engine's simulation is seeded, not bit-exact | **Amended by D947** for commit-path decisions, which move into machines simulated bit-exactly; the seeded engine simulation is unchanged (§5) |
| D313, §31 §7.1, §13 | Bit-exact DST for sans-I/O machines via `loams-detsim`; madsim and turmoil not used; the engine stays in tier 2 | **Extended**, not changed: tier 1 now also holds the engine's commit-path machines; tier 1b is new (§5.1). The seam types move to `loams-common` (D954) |
| D314 | Checkers live in `loams-detsim::checkers` | Kept; the pilot's checkers (exactly-once at offsets, outcome classification) go there |
| RT1 Tasks 1–2 | Build `loams-detsim` | DT1 depends on them; if they have not merged when DT1 starts, DT1 Task 2 builds them to RT1's spec (Q837) |
| §12 §2 items 2–3 | `FaultyStore` fault matrix; kill -9 at failpoints | Kept; §8's models add what the matrix cannot express, and `TornDisk` covers what kill -9 cannot |
| D541, D552, open-core | No billing-grade metering in this repository | CPU and wall accounting is generic observability with no tenant label (§10) |
| §05 (ANN pre- and post-filter selection) | Static thresholds | Unchanged until a calibration report proposes new values through review (§10) |
| §54 (D923) | Upgrades ship in preview.3 | DT1 starts after it; no format changes here |

---

## 14. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | The pilot finds nothing and proves little; effort is spent | Medium | Low | The gate (§11.4) stops the rollout; the pilot is eight pull requests (DT1 Tasks 0–7) |
| 2 | Machines drift from the driver that runs them (the simulated logic is not the shipped logic) | Medium | High | The driver contains no decisions; the same machine type runs in production and simulation; the sweeps (P3) run the real driver |
| 3 | The cancel sweep is flaky because suspension points vary with real I/O | Medium | Medium | Coverage is reported per point; the test requires the uncancelled run's points, not a fixed K; failures print the point and seed |
| 4 | `TornDisk` reports a loss that is redb's own (or openraft's) behaviour | Low | High | Reproduce with a minimal redb program before blaming Loams; report upstream; the gate counts it as a defect found either way |
| 5 | Moving `Ctx` to `loams-common` disturbs RT1, which is in progress | Low | Medium | Re-exports keep every path; RT1's tests run in DT1 Task 1 |
| 6 | Lint bans annoy contributors outside the commit path | Low | Low | Bans are per module; the reason names the alternative |
| 7 | Thread-CPU reads cost too much on hot operators | Medium | Low | Measured in Task 19; sampled or opt-in (Q839) |
| 8 | Someone reads the CPU metrics as billing data | Low | Medium | No tenant label; open-core's wording in the metric's help text; `loams-platform` keeps its own sources |

---

## 15. Open questions (Q835–Q842)

| # | Question | Default | Owner | Needed by |
|---|---|---|---|---|
| Q835 | The gate's thresholds (D958): one defect found, or two of P1–P4 proved, inside the cost budget? | Yes | Owner | DT1 Task 8 |
| Q836 | Move `Machine`, `Ctx`, `Millis`, `TraceSink` and `SpecEvent` from `loams-sqlrouter` to `loams_common::machine`, or keep a second copy in `loams-log`? | Move them, re-exported unchanged | Eng | DT1 Task 1 |
| Q837 | If RT1 Tasks 1–2 have not merged when DT1 starts, does DT1 build them to RT1's spec? | Yes, and RT1's status records it | Eng | DT1 Task 2 |
| Q838 | The torn-disk tests need a `test-util` hook in `loams-meta` (`MetaConfig::db_backend`). Acceptable in a production crate? | Yes, behind `test-util` only | Eng | DT1 Task 6 |
| Q839 | Thread-CPU accounting costs a syscall per poll. Always on, or behind a setting? | Always on for Loams' DataFusion operators and blocking closures if Task 19 measures under 1 % overhead; otherwise behind `LOAMS_CPU_ACCOUNTING=1` | Eng | DT1 Task 19 |
| Q840 | On an unexpected exit of a `spawn_concurrent` loop, should release builds abort the process (fail-stop) instead of failing the owner? | No: release builds fail the owner visibly (the writer answers every waiter with an error and refuses new appends); debug builds panic | Owner | DT1 Task 18 |
| Q841 | Should the stale listing model also keep showing deleted objects, which S3 no longer does but other S3-compatible stores might? | Yes; paginated listings are not snapshots on any provider anyway | Eng | DT1 Task 5 |
| Q842 | Seed budgets for the pilot's scenarios | PR: 200 seeds per `loams-log` scenario, 50 torn-disk crashes; nightly: 100 000 and 1 000 | Eng | DT1 Task 4 |

---

## 16. Sources

**Loams (read on `dev` at `f1283fec`, 2026-10-15):** design §05, §12 §2, §13 (D28, D313, D314, D744), §31 §7.1 and §13, §54; plans RT1, M0.4, UP1 (format), the console deployment plan; `crates/loams-log/src/{lib,writer,segmenter,reader,retention,gc}.rs`; `crates/loams-store/src/{lib,fault,store}.rs`; `crates/loams-sim/src/{lib,sim}.rs` and `Cargo.toml`; `crates/loams-sqlrouter/{clippy.toml,Cargo.toml,src/machine.rs}`; `crates/loams-meta/src/{clock,db,log_store,client,state_machine}.rs`; `crates/loams-git/src/clock.rs`; `crates/loams-kv/src/embedded/faulty.rs`; `crates/loams-common/src/meta/store.rs`; `crates/loams/tests/crash.rs`; `crates/loams-query/src/exec/*.rs`; `.github/workflows/ci.yml` (`sim`, `crash-gate`, clippy, `repo-checks`); workspace `Cargo.toml` and `clippy.toml`; `web/apps/console/wrangler.jsonc`.

**Third-party:** cargo registry sources of `lance` 12.0.0, `object_store` 0.14.2, `openraft` 0.10.0-alpha.34, `tantivy` 0.26.2, `datafusion-common-runtime` 54.1.0, `foyer` 0.22.6, `redb` 4.3.0 (counts in §3.3). To verify in DT1: Zig 0.16's `std.Io` semantics (§2.1); clippy's path for `tokio::spawn` in `disallowed-methods`; `rustix` 1's `time::clock_gettime(ClockId::ThreadCPUTime)`; `redb` 4's `StorageBackend` contract for `sync_data`; list-after-write consistency of RustFS and the S3-compatible stores the `Store` conformance suite covers.
