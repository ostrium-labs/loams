# GT1 — Loams Git in Production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact names, paths, constants, messages or defaults, use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file. The code is not pre-written in this plan (M0.3 Ruling 1).
>
> **Status: Planned** (2026-10-10). Track GT, design [§36](../design/36-loams-git.md) (D388–D399, D381, D382; amended by D411 and D415; Q384–Q395 all resolved on 2026-10-02). §36 has no separate production design, so this plan carries the production rulings §36 leaves open (the public API, the repository catalog, ownership, operations), each marked as a ruling and listed under "Open questions" where the owner must confirm it.
>
> **This plan supersedes the three 2026-10-01 plans** [GT1](2026-10-01-gt1-wal-git-core.md), [GT2](2026-10-01-gt2-smart-http.md) and [GT3](2026-10-01-gt3-build-cache-and-mirror.md). None of them started: `dev` at `1dc6e8a3` has no `loams-git`, `loams-git-remote`, `loams-buildcache`, `loams-registry` or `loams-fs` crate and no `proto/loams/git/`. Their tasks, test names and rulings are folded in here and keep their names, so reviews that cite them still apply. Mapping: old GT1 → GT1a and GT1b; old GT2 → GT1d and GT1e; old GT3 → GT1f. GT1c and GT1g are new (production). §36's GT4 (`loams-vfs`, the `Materializer`) and GT5 (NVMe replicas, gossip, any-node writes) stay out of scope. Task 0 marks the old plans superseded in `docs/plans/README.md`; this plan changed no other file when it was written.
>
> **Schedule.** D411 keeps track GT in §15's W1 slot, after M3. M3 has no plan on `dev`. Whether this production plan starts now or waits is GT-Q1, an owner decision, needed before Task 1.

**Goal:** Loams Git GA. Bucket-native Git hosting for agent fleets, with:
- the §36 §4 WAL core: create-only segments as the commit point, checkpoints, one-object packs, a per-repository sequencer with group commit, fencing, unknown-outcome resolution and idempotency, O(1) forks;
- `git-remote-loams`: serverless `loams::<store-url>` access straight to the bucket, then `stateless-connect` and `loams://` addresses for partial clone;
- repositories as a service: a `loams.repos.v1` Connect API, a catalog of names, sequencer ownership by rendezvous and lease, forwarding, the `_git` event stream;
- Smart HTTP for stock clients: protocol v2 upload-pack on range reads, v0/v1 receive-pack with streaming verification, §15 W1's client matrix;
- compaction with stock `git repack`, pack GC across fork families, integrity scrubs;
- the sccache build cache (gateway and direct paths, trust classes) and the crates.io mirror;
- production operations: §36 §11's metric families, traces, dashboards and alerts, quotas, a failure table with deterministic simulation and a chaos soak, a restore drill, a security review with fuzzing, a performance gate, single-node mode, CLI and docs.

The exit is "Exit criteria for production" at the end of this plan, with the owning tasks.

**Architecture** (§36 §4–§7):
- **`crates/loams-git`** is the portable core: formats, the four traits (`WalStore`, `RefLog`, `BlobStore`, and from GT4 `Materializer`), the state machine, `BucketRefLog`, the object databases and the transport-free protocol module. It depends on `loams-store`, `loams-cloudevents` and gitoxide crates only, and has no listener.
- **`crates/loams-git-remote`** builds the binary `git-remote-loams`. It links `loams-git` and runs `git` subcommands as processes.
- **The `loams` binary** gains the off-by-default feature `git`: `RepoService` (`loams.repos.v1`), the Smart HTTP routes and the per-repository sequencers, all in the `gateway` role, plus the `git-compact` and `git-gc` worker tasks in the `worker` role. The features `buildcache` and `registry` (also off by default) mount `loams-buildcache` and `loams-registry` in the `gateway` role.
- **Correctness never depends on one sequencer.** The rendezvous owner of a repository runs its sequencer under the lease `task/git-seq/<ns>/<repo_id>`. A wrong owner, a zombie after a lease loss, or a direct-writing helper is fenced by the create-only segment PUT (§36 §4.2). The lease only keeps the common case on one node so that group commit works.
- **Object storage is the source of truth** (D1). Pack caches, the H1 range cache (`loams-cache::RangeCache`), compaction mirrors and spill files are rebuildable caches in an `Fs` (§36 §17).
- **Stock `git` is a process, never a library** (D394, D396): in the helper, the compaction worker and the tests.

**Tech Stack:**
- Rust 1.97 (workspace `rust-version`), edition 2024, workspace lints.
- gitoxide crates from one release train, at least 14 days old (Task 0 records the exact versions; the `gix` 0.88.0 train of 2026-09-25 qualifies on 2026-10-10): `gix-hash`, `gix-validate`, `gix-object`, `gix-pack`, `gix-packetline` (async), `gix-commitgraph`, `gix-features`, with `default-features = false` and only `sha1`. MIT OR Apache-2.0.
- Workspace crates already on `dev`: `loams-store` (`put_if_absent`, `put_if_match`, `get_range`, `head`, `list`, `FaultyStore` with `Fault::ErrorAfterApply` on `Op::PutCreate`), `loams-cloudevents`, `loams-stream-grpc` (the `io.cloudevents.v1.CloudEventBatch` codegen and `ProduceCloudEvents`, PR #171 merged), `loams-meta-conformance` (`linearizability::check`, `CasRegisterModel`), `loams-worker` (`Task`, `TaskSource`, `TaskKey::lease()` = `task/<key>`), `loams-hot::placement` (`PlacementKey`, `rendezvous_score`), `loams-cache::RangeCache`, `loams-sim` (seeded harness, `elle`), `loams-proto` (connectrpc 0.9 and buffa 0.9).
- Workspace dependencies: `prost` 0.14 and `prost-build` 0.14 (the on-disk formats), `crc32c` 0.6, `sha2` 0.11, `axum` 0.8, `reqwest` 0.13, `object_store` 0.14, `tokio`, `bytes`, `futures`, `async-trait`, `thiserror`, `tracing`, `proptest`.
- System and test tools, run as processes and never linked: `git` (the CI image's version and the newest release, both ≥ 2.45), the `gix` CLI, `pygit2` (libgit2) from a `uv` venv, JGit's CLI jar, `sccache` v0.18.0, `cargo`, `rustfs/rustfs:1.0.x` (D61), `cargo-fuzz` (nightly, CI only), `buf`.

**Spec:**
- [§36](../design/36-loams-git.md) (all), D388–D399, D411, D415 and Q384–Q395 in the [decision log](../design/13-decision-log.md); D561 and D562 (proposed) only as the reason the bucket WAL stays the default.
- [§15](../design/15-agent-workspaces.md) §3 (amended by D388–D390), §6, §7, §8, §11, W1's client matrix.
- [§02](../design/02-stream-engine.md) §7.4 (D270), [§03](../design/03-storage-formats.md) §6–§7 (format versions, GC grace and claims), [§04](../design/04-hot-tier.md) (H1), [§09](../design/09-links-and-workers.md) (worker tasks), [§18](../design/18-metastore-backends-and-router.md) §5.3 (D75), [§27](../design/27-usage-hooks.md), [§44](../design/44-unified-api-and-sdks.md) §4–§8 (API rules, the catalogue).
- As built (`dev` at `1dc6e8a3`): `crates/loams-store/src/{store.rs,fault.rs,error.rs}`, `crates/loams-cloudevents/src/lib.rs`, `crates/loams-stream-grpc/{src/events.rs,proto/io/cloudevents/v1/cloudevents.proto}`, `crates/loams-meta-conformance/src/linearizability.rs`, `crates/loams-worker/src/lib.rs`, `crates/loams-hot/src/placement.rs`, `crates/loams-cache/src/range_cache.rs`, `crates/loams/src/{main.rs,server.rs,api/connect.rs,api/internal.rs}`, `crates/loams-proto/build.rs`.

## Global Constraints

- **Worktree and branches.** Work in `~/Documents/Ostriumlabs/loams-wt/gt1-loams-git`. One branch per milestone: `feat/gt1a-wal-core`, `feat/gt1b-helper`, `feat/gt1c-repo-service`, `feat/gt1d-smart-http`, `feat/gt1e-compaction`, `feat/gt1f-cache-mirror` and `feat/gt1g-ops`, each based on `dev`, with stacked PRs targeting `dev` (the 2026-10-01 plans said `main`; the repository integrates on `dev`). Use `git commit -s` (DCO). Commit areas: `git`, `store`, `fs`, `events`, `proto`, `api`, `worker`, `cache`, `registry`, `bench`, `ci`, `deploy`, `docs`.
- **Rust builds** use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR` and never build in `/tmp`. One cargo build at a time on the build machine (D127; jobs and linker from `~/.cargo/config.toml`). Build the touched crates (`cargo test -p loams-git`), and `cargo test -p loams --features git` only before a commit that touches `loams`. Never `--workspace --all-features` locally. RustFS, the client matrix and the chaos soak run in CI, or locally only when no cargo build is running. Stop and report if `/home` has under 8 GB free.
- **The default build does not change.** `cargo tree -p loams -e normal` on default features must not list `gix-pack`, `loams-git`, `loams-buildcache` or `loams-registry` (Task 14 test `default_features_exclude_git`).
- **Never link copyleft code** (D11, D394, D396). `git2`, `libgit2-sys` and any GPL crate are refused by `scripts/ci/git-licence.sh` (Task 41). `git` runs as an unmodified separate process.
- **Object storage is the source of truth.** No state outside the bucket is read for correctness. Every local cache is keyed by content or by a sequence number and may be deleted at any time; a test in each task that adds a cache deletes it mid-run.
- **One limits module.** Every constant this plan names lives in `loams_git::limits` (or `loams_buildcache::limits`, `loams_registry::limits`) with a doc comment naming its §36 section, and appears in the limits table (D88). Each limit has a test at the limit and one past it.
- **AP0 API rules** (§44) for `loams.repos.v1`: every mutation has `idempotency_key = 15`; reads are `NO_SIDE_EFFECTS`; errors are `loams.errors.v1` with reasons registered in `docs/api/reasons.md`; pagination is `page_size`/`page_token`; watch streams send a snapshot, then changes, then a heartbeat every 15 s.
- **Tenant from the credential** (D182, §36 §10). The namespace comes from the token, never from the URL alone; a URL naming another namespace answers `404` with no detail.
- **Loopback only until the unified auth plan** (D111, Q389, MT1 / D451). Every new listener refuses a non-loopback address with `<listener> listen on <addr>: only loopback addresses are served until the unified auth plan (D111)`. Authorization goes through the `GitAuthorizer` seam (Task 18).
- **No billing** (D393, D548, D550). No field, metric, table or endpoint named `plan`, `price`, `invoice`, `credit`, `meter`, `billable` or `usage`. `scripts/ci/no-metering.sh` covers every new crate (Task 37). Usage leaves only as §36 §11's metric families and the `_git` stream.
- **Secrets.** Tokens, vended credentials and upstream keys are `Secret<T>` whose `Debug` and `Display` print `[redacted]`. None appears in a log line, an error, a WAL event, a CloudEvent, a metric label or a report-status line.
- **Pins.** Exact versions for every new crate, image and tool, at least 14 days old (`cargo info`, the registry date). Record each pin in the task's commit message.
- **Docs and code stay in step.** A change to a §36 format, constant or rule updates §36 in the same PR, marked "As built (GT1x)".

## Rulings made while writing this plan

Rulings 1–8 are the 2026-10-01 GT1 plan's, 9–14 GT2's, 15–19 GT3's, carried verbatim in substance. Rulings 20–30 are new.

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| 1 | Content-named blobs over 64 MiB use an unconditional multipart PUT; under 64 MiB, `put_if_absent`. On RustFS, blobs over 64 MiB are refused (`BlobError::TooLarge`) until Task 6 confirms atomic multipart completion (Q385) | Two writers of one content name write identical bytes | A 64 MiB push limit on RustFS until confirmed |
| 2 | The serverless helper is its own sequencer per `git push` process, fenced by the segment PUT | No server in GT1b; the protocol is safe for any number of writers | Many direct pushers to one hot repository retry on 412; the server path group-commits them |
| 3 | The helper starts with `fetch`/`push`/`option`; `stateless-connect` arrives in Task 25 | It needs v2 upload-pack | No partial clone through `loams::` until GT1d |
| 4 | Helper fetch downloads whole missing packs and splits each `.lpk` into `pack-<checksum>.pack`/`.idx` with no `index-pack` run | Packs are immutable and content-named; the stored idx was verified at push | Over-fetching after repacks until Task 25 |
| 5 | Fast-forward checks for helper pushes run in the pushing process (`git merge-base --is-ancestor`) | The helper has the local graph; the sequencer checks only ids | A modified helper with bucket credentials can force-push; direct bucket writers are trusted with their credentials, and the server path enforces both |
| 6 | Helper idempotency keys are `sha256(principal ‖ pack checksum ‖ sorted updates)`; `principal` is the credential subject; `git config user.email` only under `LOAMS_GIT_UNAUTHENTICATED_TESTS=1` | §36 §4.5; a caller-chosen email would let credential holders spoof one identity | None |
| 7 | **Changed for production:** a serverless `loams::` push to a missing repository fails with `fatal: repository '<address>' not found` unless `LOAMS_GIT_CREATE_ON_PUSH=1`; repositories are created by `CreateRepo` (Task 15) or `loams_git::create_repo` | A typo must not create a repository once a catalog exists | One extra call for scripts; the variable keeps the old behaviour |
| 8 | SHA-1 only. The object format is a field in every message and checkpoint; SHA-256 is refused with `object format sha256 is not supported yet (Q388)` | gitoxide's SHA-256 parity is open | An additive change later |
| 9 | Upload-pack v2 only unless Task 0 finds a matrix client without v2; a request without `Git-Protocol: version=2` gets `400` with `loams: git protocol v2 is required (git ≥ 2.26; set protocol.version=2)` (Task 0 confirms the release) | D394, Q387 | Old clients fail; Task 27 adds v0/v1 |
| 10 | v2 `ls-refs` is served from `snapshot(Latest)`; `server-option=loams-at-least=<seq>` selects `AtLeast` | Linearizable reads after a push, on any node | One GET per `ls-refs` |
| 11 | Fetch packs reuse stored deltas whose base is sent or common, else send the object whole; no new delta search | gitoxide cannot encode deltas yet | Larger packs; Task 23 records the ratio against `git upload-pack` |
| 12 | A non-atomic push is one `RefTxn` per ref, committed concurrently, so they share a segment and each ref reports its own status | git's per-ref semantics with one code path | None |
| 13 | Compaction mirrors repositories on the worker's disk at `<data_dir>/git-mirror/<ns>/<repo_id>.git`, an LRU cache | `git repack` needs a local repository | Worker disk use |
| 14 | `packfile-uris` is off at GA | It needs per-provider pre-signed URLs and client opt-in | Large clones stream through the gateway |
| 15 | The gateway cache path is WebDAV, not an S3 facade | sccache has a WebDAV backend; SigV4 for one client is much more code | The subset grows if Task 0 finds more (Q394) |
| 16 | Approximate LRU by refresh-on-hit after `refresh_after` (7 d) | No per-read index | Weekly-only hits may expire early |
| 17 | Per-repository cache keys, no cross-repository dedup (§36 §8.3) | Poisoning and side channels | Lower hit rates across forks; `read_from` grants a parent's `trusted/` |
| 18 | The mirror stores crates once, in `ns/_public/packages/crates/`, and filters per namespace at serve time | §15 principle 5 | A hidden crate is stored but never served to that tenant |
| 19 | The mirror's `config.json` says `auth-required: true` | Per-namespace policy; cargo sends `Authorization` on every request | Old cargo fails; Task 0 records the minimum |
| 20 | **The public API is `loams.repos.v1`** (`RepoService`), generated by `loams-proto` with connectrpc and buffa. **`loams.git.v1` stays the on-disk format package** (§36 §4.3, D390, D415: its `dataschema` URNs are stored in segments), generated once by `loams-git` with `prost`, and is never exposed on the API | One package's Rust types are generated exactly once (`loams-proto/build.rs`'s rule); the core must not depend on connectrpc so it stays portable to `wasm32` | A package rename before GA if the owner prefers `loams.git.v1` for the API (GT-Q2) |
| 21 | **The repository catalog is in the bucket**, not the metastore: `ns/<ns>/repos/_names/<name>.json` = `{repo_id, state, created_unix_ms, deleted_unix_ms?}`, created with `put_if_absent` and changed with `put_if_match` (ETag CAS). `repo_id` is `r` + a lowercase 26-character ULID, immutable; bucket paths use `repo_id`, so delete-and-recreate and renames never alias data | D1; the core stays portable; `MetaStore` (`crates/loams-common/src/meta/store.rs`) has no generic records and adding a resource kind there touches every backend and the conformance suite | Listing many repositories is a LIST, not an index scan; GT-Q3 |
| 22 | **`<ns>` in object paths is the numeric `loams_common::NamespaceId`**, as collections use (`ns/1/collections/…`); URLs and the API carry the namespace **name**, resolved with `MetaStore::namespace_by_name`. §36's `NamespaceId(String)` becomes a re-export of `loams_common::NamespaceId` | One namespace identity in the bucket | A §36 §4.1 wording fix only |
| 23 | **The CloudEvents protobuf codec moves into `loams-cloudevents`** behind a feature `protobuf` (prost only); `loams-stream-grpc` re-exports it | `loams-git` must not depend on tonic (`loams-stream-grpc`'s codegen is `tonic-prost`), and the type must be generated once | A small refactor of a merged crate |
| 24 | **`loams-fs` is built minimally** (the §36 §17.2 trait, `NativeFs`, `MemFs`, the conformance suite) in Task 2, because spill files and caches need it and it does not exist on `dev` | §36 §17, D382 | None |
| 25 | **`loams-store` gains three additive methods** in Task 1: `put_multipart`, `copy` and `list_page(prefix, after, limit)`; today's `list` collects a whole prefix into memory | Large packs, refresh-on-hit and paged GC listings need them | None; additive |
| 26 | **`PlacementKey` gains `ResourceKind::Repo`** with `PlacementKey::repo(ns, repo_id_hash)`, hashed with the existing non-collection rule (kind byte appended) | D75's one router; `ResourceKind` is `#[non_exhaustive]` | None |
| 27 | **Forwarding to the sequencer owner is an internal HTTP route** beside `crates/loams/src/api/internal.rs`'s: `POST /internal/v1/git/commit` with a postcard-encoded `ForwardedTxn`; a 2 s timeout falls back to a local, fenced sequencer (counted) | The internal API is plain HTTP today; fencing makes the fallback safe | Contention on a hot repository during an owner outage |
| 28 | **Idle sequencers are evicted** after `seq_idle_evict` (10 min); the next push loads hint, checkpoint and tail | §36 §4.4 "idle repositories keep no sequencer" | One cold load per idle repository |
| 29 | **Usage hooks are Prometheus text on the gateway's `/metrics`** unless Task 0 finds a §27 exporter on `dev` (none found at `1dc6e8a3`); per-repository labels are never used, only `org` and `namespace` | §36 §11; cardinality | A port to OTLP later (Q-UH-1) |
| 30 | **GA is gated on MT1** (the unified auth plan) for non-loopback serving; everything else in this plan can finish before it | D111; a git host that only serves loopback is a single-node product | GT-Q6 |

---

## Review Focus

1. **No acknowledged push is lost, and none is applied twice.** Tests: Task 7 `append_is_exclusive_across_writers`, `retry_after_lost_ack_is_committed`; Task 10 `lost_ack_resolves_to_committed`, `fenced_group_revalidates`, `four_sequencers_under_faults_linearizable`; Task 12 `concurrent_pushes_never_lose_an_acked_push`; Task 39 `sim_10k_seeds_no_lost_ack`.
2. **Fencing holds with many sequencers, zombies and direct writers.** Tests: Task 10 `two_sequencers_one_store_linearizable`; Task 16 `zombie_owner_is_fenced`, `helper_and_server_race_one_repo`.
3. **Formats are exact and versioned.** Tests: Task 5 `golden_segment_v1`, `golden_checkpoint_v1`, `corrupt_crc_is_error`, `n_minus_one_is_read`.
4. **Readers never see a partial state or a mixed pack set.** Tests: Task 10 `snapshot_is_a_prefix_of_commits`; Task 11 `checkpoint_plus_replay_equals_state`; Task 28 `readers_see_old_or_new_pack_set`.
5. **A push is acknowledged only after its pack and segment are durable, and malformed or disconnected packs never reach the WAL.** Tests: Task 24 `ack_after_segment_commit`, `malformed_pack_is_refused`, `missing_base_is_refused`, `connectivity_gap_is_refused`; Task 41 `fuzz_pack_receive` (no panic, no WAL write).
6. **Compaction and GC never lose an object**, including across forks. Tests: Task 28 `repack_preserves_every_reachable_object`, `concurrent_push_during_compaction_keeps_new_pack`; Task 29 `pack_gc_respects_grace_and_forks`, `fork_family_reachability_keeps_parent_packs`; Task 30 `scrub_detects_missing_pack`.
7. **A tenant reads or writes another tenant's repository**, or a token writes outside its branch prefix. Expected: never. Tests: Task 18 `namespace_from_token_not_url`, `branch_prefix_scope_refuses_push`, `other_namespace_is_404`.
8. **Cache poisoning.** Tests: Task 31 `untrusted_put_to_trusted_is_403`, `class_comes_from_token_not_path`; Task 33 `untrusted_key_cannot_write_or_read_outside_prefix`; Task 34 `untrusted_ci_cannot_write_trusted`.
9. **Supply chain through the mirror.** Tests: Task 35 `crate_with_wrong_cksum_is_refused_and_not_stored`, `quarantined_version_is_hidden`, `denied_crate_is_404`.
10. **Unbounded memory or disk.** Tests: Task 23 `large_clone_bounded_rss`; Task 24 `two_gib_push_bounded_rss`; Task 2 `spill_dir_removed_on_drop`.
11. **Copyleft code or the default build gains Git.** Tests: Task 41 `no_libgit2_in_lockfile`; Task 14 `default_features_exclude_git`.
12. **A secret leaks.** Tests: Task 18 `token_never_logged`; Task 33 `vended_secret_not_in_recipe_debug`.

---

## File structure

```
proto/loams/git/v1/wal.proto                                  Task 5 (§36 §4.3 verbatim; prost, on-disk only)
proto/loams/repos/v1/repos.proto                              Task 14 (public API; loams-proto)
crates/loams-store/src/{store.rs,fault.rs}                    Task 1 (put_multipart, copy, list_page)
crates/loams-fs/                                              Task 2
  src/{lib.rs,path.rs,native.rs,mem.rs,conformance.rs}  tests/{native.rs,mem.rs}
crates/loams-cloudevents/{build.rs,src/protobuf.rs}           Task 3 (feature protobuf)
crates/loams-stream-grpc/{build.rs,src/lib.rs}                Task 3 (re-export)
crates/loams-git/                                             Tasks 4–11, 16–17, 19–24, 28–30
  Cargo.toml  build.rs
  src/{lib.rs,limits.rs,ids.rs,paths.rs,error.rs,catalog.rs,
       format/{mod.rs,segment.rs,checkpoint.rs,lpk.rs,event.rs},
       blob.rs,wal.rs,state.rs,odb/{mod.rs,pack_cache.rs,range.rs},reflog.rs,sequencer.rs,
       checkpointer.rs,fork.rs,gc.rs,pack_gc.rs,scrub.rs,owner.rs,mirror_events.rs,commit_graph.rs,compact.rs,
       protocol/{mod.rs,pktline.rs,advertise.rs,lsrefs.rs,objectinfo.rs,fetch.rs,negotiate.rs,
                 assemble.rs,filter.rs,shallow.rs,receive.rs,verify.rs,report.rs,v0.rs},
       metrics.rs,testing.rs}
  tests/{format.rs,blob.rs,wal.rs,state.rs,odb.rs,reflog.rs,linearizable.rs,checkpoint.rs,fork.rs,gc.rs,
         catalog.rs,owner.rs,events.rs,protocol.rs,lsrefs.rs,fetch.rs,receive.rs,compact.rs,pack_gc.rs,
         scrub.rs,differential.rs,sim.rs}
  tests/golden/{segment_v1.lgw,checkpoint_v1.lgc,lpk_v1_footer.bin}
  fuzz/{Cargo.toml,fuzz_targets/{pktline.rs,receive_pack.rs,segment.rs,checkpoint.rs}}   Task 41
crates/loams-git-remote/                                      Tasks 12, 25
  src/{main.rs,protocol.rs,url.rs,fetch.rs,push.rs,local.rs,connect.rs}  tests/{helper.rs,concurrent.rs,faults.rs}
crates/loams-buildcache/                                      Tasks 31–34
  src/{lib.rs,limits.rs,keys.rs,trust.rs,tokens.rs,webdav.rs,refresh.rs,sweep.rs,metrics.rs,direct.rs}
  tests/{webdav.rs,trust.rs,sweep.rs,direct.rs,sccache_e2e.rs}
crates/loams-registry/                                        Tasks 35–36
  src/{lib.rs,limits.rs,index.rs,crates.rs,policy.rs,upstream.rs,audit.rs,metrics.rs}
  tests/{index.rs,crates.rs,policy.rs,cargo_e2e.rs,fixtures/…}
crates/loams-hot/src/placement.rs                             Task 16 (ResourceKind::Repo)
crates/loams/Cargo.toml                                       features git, buildcache, registry
crates/loams/src/api/{repos.rs,git_http.rs,git_internal.rs,buildcache.rs,registry.rs}
crates/loams/src/api/connect.rs                               Task 14 (CATALOGUE row)
crates/loams/src/{main.rs,server.rs}                          --git-listen, --cache-listen, --registry-listen, dev --git
crates/loams/tests/git/{main.rs,http.rs,matrix.rs,repos.rs,e2e.rs}
crates/loams-sim/                                             Task 39 (git workload, if the harness needs a hook)
bench/git-wal/                                                Task 13, 42
bench/fixtures/sccache-ws/                                    Task 34
scripts/git/{rustfs.sh,bench.sh,matrix.sh,pygit2_probe.py,jgit_probe.sh,gix_probe.sh,restore-drill.sh,chaos.sh}
scripts/ci/{git-licence.sh,no-metering.sh (extended)}
deploy/loams-git-dev/compose.yaml                             Task 43 (loams dev --git + RustFS)
deploy/observability/loams-git/{dashboards/,alerts.yaml}      Task 37
docs/security/loams-git-threat-model.md                       Task 41
docs/runbooks/loams-git/                                      Task 45
docs/guides/{git.md,build-cache.md,crates-mirror.md}          Tasks 34, 36, 45
docs/api/{reasons.md,route-map.md}                            Task 14
.github/workflows/{gt1.yml,gt1-e2e.yml,gt1-nightly.yml}
.github/workflow-templates/loams-sccache.yml                  Task 34
docs/design/36-loams-git.md  docs/plans/README.md  CHANGELOG.md
```

## Shared contracts (all tasks use these names)

### Object layout (§36 §4.1, Rulings 21–22)

```
ns/<ns_id>/repos/_names/<name>.json           catalog record (Ruling 21); put_if_absent / put_if_match
ns/<ns_id>/repos/<repo_id>/
  head                                        hint {seq, checkpoint, written_unix_ms}; ≤ 1 write/s
  wal/<seq:020>.lgw                           create-only segments; the PUT is the commit point (D389)
  checkpoints/<seq:020>.lgc                   create-only full state after seq
  packs/<checksum:hex>.lpk                    create-only: pack ‖ idx ‖ 32-byte footer (D390)
  midx/<seq:020>.midx                         compaction output
  commit-graph/<seq:020>.graph                compaction output
  gc/claims/<ulid>.json                       GC claims (§03 §7)
ns/<ns_id>/cache/sccache/<repo>/{trusted,scratch/<principal>}/<key>      Tasks 31–33
ns/_public/packages/crates/{index/<path>,sha256/<cksum>}                 Task 35
```

### Traits (`loams-git`; §36 §5 verbatim)

`WalStore`, `RefLog`, `BlobStore` and their types (`Seq`, `WalBatch`, `WalSegment`, `Hint`, `Appended`, `Expect`, `RefUpdate`, `RefTxn`, `Receipt`, `RejectReason`, `RefError`, `ReadAt`, `RefSnapshot`, `CommittedTxn`, `BlobId`, `Put`) are exactly §36 §5.1–§5.3. `Materializer` (§36 §5.4) is declared, unimplemented, for GT4. Additions:

```rust
pub trait Clock: Send + Sync + fmt::Debug { fn now_unix_ms(&self) -> i64; }

/// Object lookup shared by the pack-cache Odb (Task 9) and RangeOdb (Task 21).
#[async_trait]
pub trait ObjectDb: Send + Sync + fmt::Debug {
    async fn contains(&self, oid: &ObjectId) -> Result<bool, OdbError>;
    async fn read(&self, oid: &ObjectId) -> Result<(gix_object::Kind, Bytes), OdbError>;   // delta-resolved
    async fn missing_from_closure(&self, tips: &[ObjectId], extra: Option<&dyn ObjectDb>)
        -> Result<Vec<ObjectId>, OdbError>;                                               // connectivity
}

/// Who may do what (Task 18). The MT1 implementation replaces `LoopbackDevAuthorizer`.
#[async_trait]
pub trait GitAuthorizer: Send + Sync + fmt::Debug {
    async fn resolve(&self, credential: &Secret<String>) -> Result<GitGrant, AuthError>;
}
pub struct GitGrant {
    pub org: String, pub namespace: NamespaceId, pub principal: Principal,
    pub repos: RepoMatch,                      // All | Names(Vec<String>)
    pub access: GitAccess,                     // Read | Write | Admin
    pub branch_prefixes: Vec<String>,          // empty = every ref; else writes only under these
    pub cones: Vec<RepoPath>,                  // GT4 write admission; carried, unchecked until GT4
    pub cache_class: Option<TrustClass>, pub registry_read: bool,
}

pub struct RepoRecord { pub name: String, pub repo_id: RepoId, pub state: RepoLifecycle, pub created_unix_ms: i64,
                        pub deleted_unix_ms: Option<i64>, pub etag: String }   // RepoLifecycle: Active | Deleting
#[async_trait]
pub trait RepoCatalog: Send + Sync + fmt::Debug {
    async fn create(&self, ns: NamespaceId, name: &str, format: pb::ObjectFormat) -> Result<RepoRecord, CatalogError>;
    async fn get(&self, ns: NamespaceId, name: &str) -> Result<Option<RepoRecord>, CatalogError>;
    async fn list(&self, ns: NamespaceId, after: Option<&str>, limit: usize) -> Result<Vec<RepoRecord>, CatalogError>;
    async fn mark_deleting(&self, ns: NamespaceId, name: &str, etag: &str) -> Result<RepoRecord, CatalogError>;
    async fn purge(&self, ns: NamespaceId, name: &str, etag: &str) -> Result<(), CatalogError>;
}
```

### Public API (`loams.repos.v1`, Task 14 writes it; this is the contract)

```proto
syntax = "proto3";
package loams.repos.v1;

service RepoService {
  rpc CreateRepo(CreateRepoRequest) returns (CreateRepoResponse);
  rpc GetRepo(GetRepoRequest) returns (GetRepoResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ListRepos(ListReposRequest) returns (ListReposResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ForkRepo(ForkRepoRequest) returns (ForkRepoResponse);                       // one PUT (§36 §4.7)
  rpc DeleteRepo(DeleteRepoRequest) returns (DeleteRepoResponse);                 // -> Operation
  rpc UpdateRepoConfig(UpdateRepoConfigRequest) returns (UpdateRepoConfigResponse);   // HEAD symref, protections -> ConfigChange
  rpc ListRefs(ListRefsRequest) returns (ListRefsResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc CompactRepo(CompactRepoRequest) returns (CompactRepoResponse);             // -> Operation
  rpc WatchRepo(WatchRepoRequest) returns (stream WatchRepoResponse);            // CommittedTxn from a seq
}
// Every mutating request has `string idempotency_key = 15;`.
// Repo names: [a-z0-9][a-z0-9._-]{0,99}, not ending in ".git"; ids "r" + 26-char lowercase ULID.
// Reads take `string consistency_token = 14;` ("git:<repo_id>:<seq>", §36 §4.6) and answer one.
// Repo: name, repo_id, object_format, default_branch, protections[], fork_parent {repo, seq}, seq, created_at, state.
```

Reasons (registered in `docs/api/reasons.md`): `repo_not_found` (not_found), `repo_already_exists` (already_exists), `repo_deleting` (failed_precondition), `ref_stale` (aborted), `ref_protected` (failed_precondition), `out_of_scope` (permission_denied), `object_format_unsupported` (unimplemented), `fork_too_deep` (failed_precondition), `quota_exceeded` (resource_exhausted), `sequencer_unavailable` (unavailable).

### Smart HTTP routes (§36 §6.1; Tasks 19–27)

`GET /git/<ns-name>/<repo>.git/info/refs?service=git-upload-pack|git-receive-pack`, `POST /git/<ns-name>/<repo>.git/git-upload-pack`, `POST /git/<ns-name>/<repo>.git/git-receive-pack`, on `--git-listen` (loopback). Capability strings exactly as §36 §6.1. Report-status reasons: `fetch first`, `non-fast-forward`, `protected`, `already exists`, `stale info`, `missing necessary objects`, `out of scope`, `quota exceeded`.

### Limits (`loams_git::limits`)

| Constant | Value | § |
|---|---|---|
| `MAX_GROUP_TXNS` | 64 | §36 §4.4 |
| `MAX_SEGMENT_BYTES` | 1 MiB | §36 §4.3 |
| `IDEMPOTENCY_WINDOW` | 1 h | §36 §4.5 |
| `CHECKPOINT_EVERY_SEGMENTS` / `CHECKPOINT_EVERY_BYTES` | 256 / 8 MiB | §36 §7 |
| `MAX_REFS_PER_TXN` / `MAX_REFS_PER_PUSH` | 4096 / 4096 | §36 §5.2, §6.3 |
| `EXACTLY_RETENTION` | 24 h | §36 §4.6 |
| `GC_GRACE` | 1 h | §03 §7 |
| `MAX_PACK_BYTES` | 2 GiB | §36 §6.3 |
| `MULTIPART_THRESHOLD` | 64 MiB | Ruling 1 |
| `MAX_FORK_DEPTH` | 8 | Task 9 |
| `MAX_FENCE_RETRIES` | 16 | Task 10 |
| `MAX_WANTS`, `MAX_HAVES_PER_ROUND`, `MAX_NEGOTIATION_ROUNDS` | 65 536, 256, 64 | Task 22 |
| `RECEIVE_IDLE_TIMEOUT`, `UPLOAD_DEADLINE` | 60 s, 30 min | Tasks 23–24 |
| `SEQ_IDLE_EVICT`, `FORWARD_TIMEOUT`, `HINT_EVERY` | 10 min, 2 s, 1 s | Rulings 27–28 |

---

## Execution order

1. Task 0, after the owner answers GT-Q1.
2. **GT1a** (Tasks 1–11). Tasks 1, 2 and 3 are independent and come first.
3. **GT1b** (12–13) after GT1a. Its gate (Task 13) is the first exit gate and records Q384's numbers.
4. **GT1c** (14–18) after GT1a; it can run beside GT1b.
5. **GT1d** (19–27) after GT1c Task 16. Task 27 only if Task 0 finds a matrix client without v2.
6. **GT1e** (28–30) after GT1d Task 24.
7. **GT1f** (31–36) needs only Task 4 (ids) and Task 1 (`copy`, `list_page`); it can run beside GT1a–GT1e on the one-build machine's spare slots.
8. **GT1g** (37–45) after GT1e; Task 41's fuzz targets start as soon as their parsers exist (Tasks 5, 19, 24). The non-loopback part of Task 18 and the GA gate wait for MT1 (Ruling 30).

---

### Task 0: Reconcile with the code as built

**Files:** this plan's "Rulings made during execution"; `docs/plans/README.md` (mark the 2026-10-01 GT1–GT3 rows "Superseded by [GT1 production](2026-10-10-gt1-loams-git.md)" and add this plan's rows).

Steps:
1. Answer each of the following and record the answer, with file paths and commands, as a ruling:
   - The owner's answer to GT-Q1 (start now or keep D411's slot).
   - Is `dev` still free of `loams-git`, `loams-git-remote`, `loams-fs`, `loams-buildcache`, `loams-registry` and `proto/loams/git`? If any appeared, reconcile against it before Task 1.
   - `loams_store::Store` as built: confirm Ruling 25's gaps (`put_multipart`, `copy`, paged `list`) and how `FaultyStore` targets a path prefix or an `Op`.
   - Where `io.cloudevents.v1.CloudEventBatch` is generated (`crates/loams-stream-grpc/build.rs`, `tonic-prost-build`) and what moving it into `loams-cloudevents` (Ruling 23) changes for `loams-stream-grpc`'s users.
   - `loams-proto/build.rs`'s `FILES` list and how a package becomes a `CATALOGUE` row in `crates/loams/src/api/connect.rs` (copy `loams.graph.v1`'s pattern: available when the feature is compiled in, `unstable` until GA).
   - `loams_common::NamespaceId`'s type and `MetaStore::namespace_by_name` (Ruling 22).
   - `loams-hot::placement` as built: `PlacementKey`, `ResourceKind` (`#[non_exhaustive]`, only `Collection` at `1dc6e8a3`), `rendezvous_score`, and how a gateway node learns the owner set (`PlacementImpl::owners`).
   - `loams-worker`'s `Task`/`TaskSource` registration and lease renewal, for `git-seq`, `git-compact`, `git-gc` and `buildcache-sweep`.
   - `loams-cache::RangeCache`'s API (`read`, `read_with_size`, `forget`) as `RangeOdb`'s H1 (§36 says `loams_cache::Cache`; the type is `RangeCache`).
   - The metrics exporter in `loams` (none found at `1dc6e8a3`; Ruling 29) and any `/metrics` route convention (`loams-qdrant/src/rest.rs` serves one).
   - MT1's status (D451): is there an `Authorizer` or OpenFGA client on `dev`? If not, Task 18 ships `LoopbackDevAuthorizer` only.
   - The gix release train: exact versions at least 14 days old, features with `default-features = false`, `cargo deny check`, `cargo tree -d` deltas, the cold build time of `loams-git` alone (one measured build).
   - `git --version` locally and on the CI image; whether `git index-pack --stdin --fix-thin` and `git pack-objects --revs --stdout --thin` behave as Task 12 uses them; the git release that made protocol v2 the default (Ruling 9).
   - Q386: does git drive partial clone and lazy fetch through a helper's `stateless-connect`? (A test helper proxying to `git upload-pack --stateless-rpc`.) Q387: do pygit2's libgit2 and JGit's CLI fetch over v2? Record versions.
   - Q394: the WebDAV subset sccache v0.18.0 uses (cold and warm build against a logging WebDAV server); `SCCACHE_MULTILEVEL_CHAIN=disk,webdav`; sccache's S3 backend against RustFS with a read-only key.
   - Cargo's minimum version for `auth-required` sparse registries; whether index lines carry `pubtime`; kellnr's proxy and S3 storage (for the guide).
   - Whether RustFS runs in CI today, and its image tag.
   - Whether a Kubernetes chart for `loams` exists anywhere (`deploy/` has none at `1dc6e8a3`), for Task 43 and GT-Q7.
2. Update `docs/plans/README.md` as above.
3. Commit `docs(git): GT1 task 0 rulings; supersede the 2026-10-01 GT plans`.

## GT1a — Foundations and the WAL core (Tasks 1–11)

### Task 1: `loams-store` additions

**Files:** `crates/loams-store/src/{store.rs,fault.rs}`, `crates/loams-store/tests/` (its existing suite).

**Interfaces:**
- `Store::put_multipart(path, body: BoxStream<'static, io::Result<Bytes>>, part_size: usize) -> Result<ObjectVersion, StoreError>`: unconditional; aborts the upload on error.
- `Store::copy(from, to) -> Result<(), StoreError>`: server-side copy; `from == to` is an in-place copy that refreshes last-modified where the backend allows it (Task 32 relies on it; Task 0 records per backend).
- `Store::list_page(prefix, after: Option<&str>, limit: usize) -> Result<Vec<ObjectInfo>, StoreError>`: sorted, strictly after `after`.
- `FaultyStore` classifies the new calls as `Op::PutMultipart`, `Op::Copy` and `Op::List`.

Tests: `multipart_round_trip_65_mib`; `multipart_abort_leaves_no_object`; `copy_to_self_updates_last_modified` (in-memory and `file://`; skipped with a printed reason where the backend refuses); `list_page_is_sorted_and_resumable`; `faulty_store_injects_on_new_ops`.

Commit `store: add multipart puts, copies and paged listings`.

### Task 2: `loams-fs`

**Files:** `crates/loams-fs/` (as in the file structure). Tests: `tests/{native.rs,mem.rs}`.

**Interfaces:** §36 §17.2 verbatim (`FsPath`, `WriteMode`, `FsMeta`, `FsCaps`, `FsError`, `Fs`), `NativeFs::new(root, survives_restart)`, `MemFs::new(max_file)`, `conformance::run(fs)` with capability-gated cases, and `SpillDir` (a per-request directory under an `Fs` root, removed on drop).

Tests (in the conformance suite, run on both backends): `read_after_write`; `create_new_race_one_winner`; `range_read_boundaries`; `list_pagination`; `rename_is_atomic`; `path_rejects_dot_segments_and_over_1024_bytes`; plus `spill_dir_removed_on_drop`.

Commit `fs: add the Fs trait with native and in-memory backends`.

### Task 3: The CloudEvents protobuf codec in `loams-cloudevents`

**Files:** `crates/loams-cloudevents/{Cargo.toml,build.rs,src/protobuf.rs}`, move `crates/loams-stream-grpc/proto/io/cloudevents/v1/cloudevents.proto` to `proto/io/cloudevents/v1/cloudevents.proto`, `crates/loams-stream-grpc/{build.rs,src/lib.rs,src/events.rs}`.

**Interfaces:** feature `protobuf` (prost only): `loams_cloudevents::protobuf::{pb, to_proto(&CloudEvent) -> pb::CloudEvent, from_proto(pb::CloudEvent) -> Result<CloudEvent, _>, encode_batch, decode_batch}`. `loams-stream-grpc` uses `extern_path` to these types (Ruling 23).

Tests: `proto_round_trip_preserves_attribute_strings` (proptest); `batch_round_trip`; and `loams-stream-grpc`'s existing suite passes unchanged.

Commit `events: move the CloudEvents protobuf codec into loams-cloudevents`.

### Task 4: The `loams-git` crate, ids, paths and limits

**Files:** `crates/loams-git/{Cargo.toml,src/{lib.rs,limits.rs,ids.rs,paths.rs,error.rs}}`, `tests/format.rs` (ids part).

**Interfaces:** old GT1 Task 1's `ids` and `paths`, with Ruling 22 (`NamespaceId` re-exported from `loams-common`) and `RepoId` per Ruling 21 (`r` + 26 lowercase ULID characters for new repositories; `parse` still accepts §36's `[a-z0-9][a-z0-9._-]{0,99}`), `RepoName`, `RepoPaths::{names_record(name), gc_claim(ulid)}` in addition to old GT1's paths, and the limits table above.

Tests: `refname_validation_matches_git_check_ref_format` (40-name corpus, also run through `git check-ref-format`); `repo_id_charset`; `repo_name_rejects_dot_git_suffix`; `seq_formats_as_twenty_digits_and_sorts_lexically`; `paths_round_trip`; `limits_are_documented`.

Commit `git: add the loams-git crate with ids, paths and limits`.

### Task 5: Formats

**Files:** `proto/loams/git/v1/wal.proto` (§36 §4.3 verbatim), `crates/loams-git/build.rs`, `src/format/{mod.rs,segment.rs,checkpoint.rs,lpk.rs,event.rs}`, `tests/{format.rs,golden/*}`.

**Interfaces:** old GT1 Task 2's verbatim (`SEGMENT_MAGIC`, `encode_segment`, `decode_segment`, `CHECKPOINT_MAGIC`, `encode_checkpoint`, `LPK_MAGIC`, `LpkFooter`, `pack_checksum`, `TxnEvent`, `TxnBody`, `to_cloudevent`, `from_cloudevent`), with the batch body through Task 3's `encode_batch`. Events follow D415: `id` is the key in lowercase hex, `dataschema` is `urn:loams:proto:loams.git.v1.<Message>`, extensions `tenantid`, `traceparent`, `loamsseq` only.

Tests: `segment_round_trip`; `segment_over_one_mib_is_too_large`; `corrupt_crc_is_error`; `truncated_segment_is_error`; `seq_mismatch_is_error`; `golden_segment_v1`; `checkpoint_round_trip`; `golden_checkpoint_v1`; `lpk_footer_round_trip`; `pack_checksum_reads_trailer`; `event_attributes_match_design_table`; `event_without_tenantid_is_refused`; `dataschema_must_match_type`; `n_minus_one_is_read`.

Commit `git: add the loams.git.v1 formats`.

### Task 6: `BlobStore`

**Files:** `src/blob.rs`, `tests/blob.rs`.

**Interfaces:** §36 §5.3 verbatim; `StoreBlobStore::new(store, paths)`; `LpkWriter::from_parts(pack, idx) -> (BlobId, pb::PackRef, Bytes)`; `read_idx`, `read_pack`. Ruling 1; `put_stream` uses Task 1's `put_multipart` through a Task 2 `SpillDir`. Retryable errors retried three times (50, 200, 800 ms, jittered).

Tests: `put_is_create_only_and_idempotent`; `put_existing_with_other_length_is_mismatch`; `lost_ack_put_returns_existed_on_retry`; `get_range_reads_idx_section`; `range_past_end_is_out_of_range`; `lpk_from_git_pack_round_trips` (`git verify-pack`); `large_blob_uses_multipart`; `rustfs_large_blob_refused_until_q385` (until Task 6's manual Q385 run against RustFS with a polling reader records "no partial object in 200 tries", which lifts it).

Commit `git: add BlobStore with one-object pack bundles`.

### Task 7: `WalStore`

**Files:** `src/wal.rs`, `tests/wal.rs`.

**Interfaces:** §36 §5.1 verbatim; `StoreWalStore`; `WalError { TooLarge, Corrupt { seq, reason }, Store(StoreError) }`; `Hint::empty()`. Semantics as old GT1 Task 4 (create-only append; `AlreadyExists` → read back and compare `batch_id`; unknown outcome → read back, resend at most 5 times; `read_from` with up to 8 parallel GETs, stopping at the first gap; `latest_checkpoint` by hint then `list_page`).

Tests: `append_is_exclusive_across_writers` (32 racers); `retry_after_lost_ack_is_committed`; `retry_after_lost_ack_fenced_by_other`; `r2_style_429_resolves_by_read_back` (a `FaultyStore` rule returning a throttling error after apply); `read_stops_at_gap`; `corrupt_segment_is_error_not_stop`; `hint_missing_is_empty`; `latest_checkpoint_uses_hint_then_list`; `checkpoint_put_is_idempotent_on_equal_bytes`; `store_faults_never_produce_two_owners` (proptest).

Commit `git: add the bucket WAL with fenced create-only segments`.

### Task 8: The repository state machine and idempotency

**Files:** `src/state.rs`, `tests/state.rs`.

**Interfaces:** old GT1 Task 6's `RepoState`, `IdemIndex`, `Check` and methods, verbatim. `check` adds the `GitGrant`'s `branch_prefixes` (refusing with `OutOfScope`) once Task 18 lands; `cones` stay unchecked until GT4.

Tests: `model_matches_reference`; `group_sees_earlier_txns`; `replay_inside_window_returns_receipt`; `key_reuse_with_other_digest_is_mismatch`; `after_window_old_oid_decides`; `protected_ref_refuses_delete_and_force`; `duplicate_ref_in_one_txn_is_invalid`; `checkpoint_round_trip_keeps_window_entries_only`; `apply_segment_refuses_a_gap`.

Commit `git: add the repository state machine with idempotency windows`.

### Task 9: `ObjectDb` and the pack-cache `Odb`

**Files:** `src/odb/{mod.rs,pack_cache.rs}`, `tests/odb.rs`.

**Interfaces:** the `ObjectDb` trait (shared contracts); `PackCache { fs: Arc<dyn Fs>, max_bytes }` (content-keyed, LRU, deletable); `PackCacheOdb::open(blobs, cache, snapshot, parent)` implementing `ObjectDb`. Lookup order: the repository's packs newest first, then the fork parent's at its seq, at most `MAX_FORK_DEPTH` levels (`ForkTooDeep`). This type serves the helper, compaction and tests; Task 21's `RangeOdb` serves Smart HTTP.

Tests: `reads_objects_written_by_git`; `delta_objects_resolve`; `fork_falls_through_to_parent_at_seq`; `fork_depth_is_bounded`; `missing_from_closure_finds_a_missing_blob`; `cache_eviction_never_breaks_reads`.

Commit `git: add the object database trait and a pack-cache implementation`.

### Task 10: `BucketRefLog`: the sequencer and group commit

**Files:** `src/{reflog.rs,sequencer.rs,testing.rs}`, `tests/{reflog.rs,linearizable.rs}`.

**Interfaces:** §36 §5.2 verbatim; `SequencerConfig` (old GT1 Task 7, plus `max_fence_retries`), `BucketRefLog::{open, shutdown}`, `testing::{ManualClock, mem_reflog}`. Semantics: §36 §4.4 and old GT1 Task 7 steps 1–6 exactly. The sequencer emits `GitEvent`s on an in-process channel for Tasks 17 and 37 (`Committed { seq, txns }`, `Fenced { at }`, `GroupSize(n)`).

Tests (`tests/reflog.rs`): `commit_then_snapshot_sees_it`; `group_commit_batches_concurrent_txns` (200 commits, 50 ms PUT delay, fewer than 60 segments); `group_rejects_only_the_stale_txn`; `atomic_multi_ref_all_or_none`; `lost_ack_resolves_to_committed`; `fenced_group_revalidates`; `replay_returns_original_receipt`; `snapshot_is_a_prefix_of_commits`; `watch_has_no_gaps_across_fences`; `missing_pack_is_refused_before_queueing`; `shutdown_answers_unavailable`; `fence_retry_limit_is_unavailable`.

Tests (`tests/linearizable.rs`, `loams_meta_conformance::linearizability` with a 3-ref CAS model): `single_sequencer_linearizable`; `two_sequencers_one_store_linearizable`; `four_sequencers_under_faults_linearizable` (`FaultyStore`, 5% errors, 5% `ErrorAfterApply`, ≤ 20 ms delays; seeds 0..32 on PRs, 0..1024 nightly).

Commit `git: add the bucket RefLog with group commit, fencing and idempotency`.

### Task 11: Checkpoints, forks and segment GC

**Files:** `src/{checkpointer.rs,fork.rs,gc.rs}`, `tests/{checkpoint.rs,fork.rs,gc.rs}`.

**Interfaces:** old GT1 Task 8's `create_repo`, `fork_repo`, `CheckpointPolicy`, `SegmentGcPlan`, `plan_segment_gc`, `run_segment_gc`; listings through `list_page`.

Tests: `checkpoint_plus_replay_equals_state`; `checkpoint_never_delays_ack`; `fork_is_one_put`; `fork_sees_parent_refs_and_objects`; `fork_of_fork_falls_through`; `gc_keeps_exactly_retention`; `gc_deletes_unreachable_above_gap_after_grace`; `gc_never_deletes_segments_a_reader_needs`.

Commit `git: add checkpoints, O(1) forks and segment GC`.

## GT1b — The serverless helper and the first gate (Tasks 12–13)

### Task 12: `git-remote-loams`

**Files:** `crates/loams-git-remote/{Cargo.toml,src/{main.rs,protocol.rs,url.rs,fetch.rs,push.rs,local.rs}}`, `tests/{helper.rs,concurrent.rs,faults.rs}`.

**Interfaces:** the binary `git-remote-loams`; semantics as old GT1 Task 9 steps 1–7, with Ruling 7 changed (no create on push unless `LOAMS_GIT_CREATE_ON_PUSH=1`). Error lines: `fatal: loams: expected <store-url>/ns/<ns>/repos/<repo_id>, got <address>` and `fatal: repository '<address>' not found`.

Tests (`helper.rs`, the real `git` binary on a `file://` store): `clone_of_missing_repo_fails`; `push_to_missing_repo_fails_without_create_flag`; `first_push_creates_repo_with_flag`; `clone_fetch_push_round_trip`; `non_fast_forward_is_rejected_with_fetch_first`; `force_push_with_plus_succeeds`; `delete_ref`; `atomic_multi_ref_push_all_or_none`; `push_options_are_recorded`; `fork_then_clone`; `fetch_after_other_push_gets_new_pack`. (`concurrent.rs`): `concurrent_pushes_to_distinct_branches_all_land`; `concurrent_pushes_to_one_branch_one_wins_per_round`; `concurrent_pushes_never_lose_an_acked_push`. (`faults.rs`, feature `faults`): `push_survives_lost_acks`; `repeated_push_after_lost_answer_is_replayed`.

Commit `git: add git-remote-loams, a serverless remote helper over the bucket`.

### Task 13: Benchmarks, RustFS, CI, and the GT1b gate

**Files:** `bench/git-wal/`, `scripts/git/{rustfs.sh,bench.sh}`, `.github/workflows/{gt1.yml,gt1-nightly.yml}`, `docs/design/36-loams-git.md` (§4.4 measured table).

**Interfaces:** `git-wal-bench --store <url> --repos <n> --pushers <p> --duration <s> --pack-bytes <b> [--put-delay-ms <d>]`, one JSON line (commits/s, p50/p99 latency, mean group size, PUTs and GETs per commit). `gt1.yml`: path-filtered on `crates/loams-git*/**`, `crates/loams-fs/**`, `proto/loams/git/**`; `cargo test -p loams-git -p loams-git-remote -p loams-fs` with RustFS as a service (`LOAMS_TEST_S3_URL`), store-backed suites skipped with a printed reason when unset; seeds 0..32. `gt1-nightly.yml`: seeds 0..1024, the bench on RustFS.

Gate: every GT1a and GT1b test green; at `--put-delay-ms 100`, ≥ 30 commits/s on one hot repository with p99 under 1 s; RustFS and in-memory numbers recorded; R2, S3 Standard and S3 Express One Zone recorded when the owner provides credentials (GT-Q8), including `append_is_exclusive_across_writers` against an Express directory bucket (Q384); Q393 decided from the numbers.

Commit `bench: measure the bucket WAL per store`; `ci: run the loams-git suites against RustFS`.

## GT1c — Repositories as a service (Tasks 14–18)

### Task 14: `loams.repos.v1` protos, reasons, route map and the catalogue

**Files:** `proto/loams/repos/v1/repos.proto`; `crates/loams-proto/build.rs` (`FILES`); `crates/loams/src/api/connect.rs` (`CATALOGUE` row, available with feature `git`, `unstable` until Task 45); `crates/loams/Cargo.toml` (features `git`, `buildcache`, `registry`); `docs/api/{reasons.md,route-map.md}`. Tests: `crates/loams-proto/tests/repos.rs`, `crates/loams/tests/git/main.rs`.

Tests: `buf lint` and `buf breaking` (skipped while `unstable`, as R1.6 of PG2 did); `every_mutation_has_idempotency_key`; `reads_take_consistency_token`; `reasons_registered`; `catalogue_lists_repos_only_with_feature`; `default_features_exclude_git`.

Commit `proto: add loams.repos.v1`.

### Task 15: The repository catalog and `RepoService`

**Files:** `crates/loams-git/src/catalog.rs`, `crates/loams/src/api/repos.rs`, `tests/catalog.rs`, `crates/loams/tests/git/repos.rs`.

**Interfaces:**
- `BucketRepoCatalog` implementing `RepoCatalog` (Ruling 21). `create` writes the name record with `put_if_absent`, then `create_repo` (checkpoint 0). A create whose name write was undetermined and is retried reads the record back and answers the first call's `RepoRecord` when its `repo_id` matches the request's derived id (`repo_id = ulid_from(idempotency_key)` for keyed calls).
- `RepoService` handlers over `RepoCatalog`, `RepoSequencers` (Task 16) and `GitAuthorizer` (Task 18). `ForkRepo` is `fork_repo` plus a name record. `DeleteRepo` marks `Deleting`, returns an `Operation`; Task 29's GC purges data and the record after `GC_GRACE`, unless a live fork names the repository as its parent (then `repo_deleting` stays until the forks are gone, or GT-Q11's policy). `UpdateRepoConfig` commits a `ConfigChange` through the sequencer. `ListRefs` and `WatchRepo` read through `RefLog`.

Tests: `create_repo_replay_returns_same_repo`; `create_same_name_other_key_is_already_exists`; `undetermined_create_then_retry_returns_same_repo`; `recreate_after_purge_gets_new_repo_id`; `fork_repo_is_one_checkpoint_put_plus_name`; `delete_returns_operation_and_hides_repo`; `delete_with_live_fork_waits`; `update_config_sets_head_and_protections`; `list_refs_honours_consistency_token`; `watch_repo_snapshot_then_changes_then_heartbeat`; `list_repos_paginates`.

Commit `git: add the repository catalog and RepoService`.

### Task 16: Placement, sequencer ownership and forwarding

**Files:** `crates/loams-hot/src/placement.rs` (`ResourceKind::Repo`, `PlacementKey::repo`), `crates/loams-git/src/owner.rs`, `crates/loams/src/api/git_internal.rs`, `crates/loams/src/server.rs`, `tests/owner.rs`.

**Interfaces:**
- `RepoSequencers::get_or_open(ns, repo_id) -> Result<SeqHandle, RefError>` on a gateway node: if this node ranks first for `PlacementKey::repo`, it takes the lease `task/git-seq/<ns>/<repo_id>` through `loams-worker`'s lease API and opens a `BucketRefLog`; otherwise it returns a `Forwarding` handle.
- `Forwarding` posts `ForwardedTxn` to `POST /internal/v1/git/commit` on the owner; after `FORWARD_TIMEOUT` it commits through a local, unleased `BucketRefLog` (safe by fencing) and counts `loams_git_owner_fallbacks_total` (Ruling 27).
- Lease loss closes the sequencer (`shutdown` answers `Unavailable`, clients retry); idle eviction after `SEQ_IDLE_EVICT` (Ruling 28).

Tests: `rendezvous_repo_scores_are_stable` (golden scores); `owner_runs_the_only_leased_sequencer`; `forwarded_push_commits_on_owner`; `owner_down_fallback_is_fenced`; `zombie_owner_is_fenced` (pause renewals with `WorkerHandle::pause_renewals`, a second node takes over, the first one's append is `Fenced`); `helper_and_server_race_one_repo` (§36 risk 8); `idle_sequencer_is_evicted_and_reloads`.

Commit `git: place repository sequencers by rendezvous and forward pushes`.

### Task 17: The `_git` event-stream mirror

**Files:** `crates/loams-git/src/mirror_events.rs`, `crates/loams/src/server.rs` (wiring to the stream API as built), `tests/events.rs`.

**Interfaces:** `EventMirror` consumes Task 10's `GitEvent::Committed` and appends each transaction's CloudEvent to the namespace stream `_git` (partition key `repo_id`) through `loams_stream_grpc::EventProducer` (or the in-process equivalent Task 0 records), deduplicated by (`source`, `id`). `MirrorRepair` (worker task `git-mirror-repair/<ns>`, hourly) compares each active repository's `head` with the stream's last `loamsseq` and replays the gap from the WAL.

Tests: `each_commit_appears_once_in_git_stream`; `mirror_failure_never_blocks_ack`; `repair_fills_a_gap_after_crash`; `replayed_events_are_deduplicated`.

Commit `git: mirror committed transactions into the _git stream`.

### Task 18: Authorization, token scopes and the listener rule

**Files:** `crates/loams-git/src/{lib.rs (GitAuthorizer, GitGrant)}`, `crates/loams/src/api/{repos.rs,git_http.rs}`, `crates/loams/src/main.rs` (`--git-listen`, `--git-tokens <path>`), `crates/loams/tests/git/http.rs`.

**Interfaces:**
- `GitAuthorizer` and `GitGrant` (shared contracts).
- `LoopbackDevAuthorizer`: a TOML file of SHA-256 token hashes mapped to grants (as old GT3's `FileTokens`), loopback only.
- `MtAuthorizer` when MT1 lands (Ruling 30): the grant from the vended token's claims (§15 §8: namespace, repo, branch prefix, cones, cache class, registry read).
- The receive path and `RepoService` refuse writes outside `branch_prefixes` (`out of scope` / `out_of_scope`); HTTP Basic (`x-token:<token>`) and Bearer are both accepted for stock git.

Tests: `namespace_from_token_not_url`; `other_namespace_is_404`; `read_token_cannot_push`; `branch_prefix_scope_refuses_push`; `admin_needed_for_delete_and_config`; `non_loopback_listen_is_refused`; `token_never_logged` (captured `tracing` output and every error's `Debug`).

Commit `git: authorize repository access through a GitAuthorizer`.

## GT1d — Smart HTTP (Tasks 19–27)

### Task 19: pkt-lines, the listener and advertisements

**Files:** `src/protocol/{mod.rs,pktline.rs,advertise.rs}`, `crates/loams/src/api/git_http.rs`, `tests/protocol.rs`, `crates/loams/tests/git/http.rs`.

**Interfaces:** old GT2 Task 1 verbatim (`Pkt`, `PktReader`, `PktWriter`, `advertise_v2`, `advertise_receive_v0`, `GitConfig`), with the repository resolved by namespace name and repo name through the catalog (Ruling 22). Every response is streamed; spill goes to a `SpillDir`.

Tests: `pktline_round_trip_and_limits`; `advertise_v2_matches_design`; `advertise_receive_has_capabilities_after_nul`; `v0_upload_request_is_400_with_hint`; `unknown_namespace_is_404`; `deleting_repo_is_404`; `git_ls_remote_against_empty_repo`.

Commit `git: serve Smart HTTP advertisements`.

### Task 20: `ls-refs` and `object-info`

**Files:** `src/protocol/{lsrefs.rs,objectinfo.rs}`, `tests/lsrefs.rs`.

Semantics: old GT2 Task 2; Ruling 10. Tests: `ls_refs_prefix_filters`; `ls_refs_symrefs_and_unborn`; `ls_refs_peels_tags`; `ls_refs_at_least_seq_reads_own_push` (push on node A, read on node B); `object_info_sizes`.

Commit `git: answer ls-refs and object-info`.

### Task 21: `RangeOdb`, the range-read object database

**Files:** `src/odb/range.rs`, `tests/fetch.rs` (odb part).

**Interfaces:** old GT2 Task 3's `RangeOdb` over `BlobStore` and `loams_cache::RangeCache` (not `Cache`; Task 0), implementing `ObjectDb`, plus `locate`, `header`, `raw_entry`. Idx sections are read lazily (fan-out, then 4 KiB-aligned oid pages, then offsets); the MIDX replaces per-pack lookups when present; entry reads fetch at least 64 KiB.

Tests: `locate_matches_git_cat_file`; `read_resolves_ofs_and_ref_deltas`; `midx_lookup_matches_per_pack`; `range_reads_are_coalesced` (under 50 GETs for 1,000 objects of one pack); `fork_parent_lookup`; `cache_dropped_mid_read_still_correct`.

Commit `git: read objects by range from stored packs`.

### Task 22: Negotiation, commit graph and shallow

**Files:** `src/protocol/{fetch.rs,negotiate.rs,shallow.rs}`, `src/commit_graph.rs`, `tests/fetch.rs`.

Semantics: old GT2 Task 4; the commit graph from compaction's `commit-graph/<seq>.graph`, else built per request and cached in the `Fs` per pack-set seq.

Tests: `clone_negotiates_with_no_haves`; `fetch_after_push_sends_only_new_commits`; `ready_ends_negotiation_early`; `deepen_1_matches_git`; `deepen_since_and_not`; `want_ref_resolves`; `negotiation_round_limit_is_enforced`.

Commit `git: negotiate fetches with a commit graph, shallow and deepen`.

### Task 23: Pack assembly and filters

**Files:** `src/protocol/{assemble.rs,filter.rs}`, `tests/fetch.rs`.

Semantics: old GT2 Task 5; Ruling 11. `sparse:oid` refused with `ERR filter sparse:oid is not supported`.

Tests: `clone_round_trips_through_git_fsck`; `partial_clone_blob_none_has_no_blobs`; `blob_limit_filters_by_size`; `tree_zero_has_only_commits`; `thin_pack_resolves_on_client`; `whole_pack_reuse_after_repack`; `large_clone_bounded_rss` (1 GiB fixture, RSS under 256 MiB, nightly); `pack_size_within_1_5x_of_git`.

Commit `git: assemble fetch packs with delta reuse and partial-clone filters`.

### Task 24: `receive-pack`

**Files:** `src/protocol/{receive.rs,verify.rs,report.rs}`, `tests/receive.rs`.

Semantics: §36 §6.3 steps 1–6 and old GT2 Task 6, with Ruling 12, Task 16's ownership and Task 18's scopes. The idempotency key is §36 §4.5's derived key. A pack older than `GC_GRACE` at commit is refused (`stale info`). The receipt's seq goes out as `loams-seq=<n>` on side-band 2.

Tests: `push_new_branch`; `push_ff_and_reject_non_ff`; `atomic_push_all_or_none`; `non_atomic_push_reports_per_ref`; `delete_ref_with_delete_refs`; `push_options_reach_the_wal`; `malformed_pack_is_refused`; `missing_base_is_refused`; `connectivity_gap_is_refused`; `protected_branch_refuses_force_and_delete`; `out_of_scope_ref_is_refused`; `ack_after_segment_commit`; `retried_push_is_replayed`; `two_gib_push_bounded_rss` (nightly); `max_refs_and_max_pack_limits`; `receive_idle_timeout_aborts_cleanly`.

Commit `git: receive pushes with streaming verification and group commit`.

### Task 25: `stateless-connect` and `loams://` in the helper

**Files:** `crates/loams-git-remote/src/{connect.rs,url.rs}`, `tests/helper.rs`.

Semantics: old GT2 Task 8, per Task 0's Q386 result (fallback: partial clone over Smart HTTP, helper keeps `fetch`/`push`). For `loams::` addresses the in-process upload-pack runs Tasks 20–23 over the bucket; for `loams://<host>/<ns>/<repo>`, a v2 tunnel to `https://<host>/git/<ns>/<repo>.git/git-upload-pack` (plain `http://` only for loopback). Credentials from `git credential fill`.

Tests: `partial_clone_through_helper`; `lazy_fetch_of_missing_blob_through_helper`; `sparse_checkout_fetches_only_cone_blobs`; `loams_url_reaches_server`; `push_to_loams_url_uses_receive_pack`.

Commit `git: tunnel protocol v2 through git-remote-loams`.

### Task 26: The client matrix and the differential suite

**Files:** `crates/loams/tests/git/matrix.rs`, `scripts/git/{matrix.sh,pygit2_probe.py,jgit_probe.sh,gix_probe.sh}`, `tests/differential.rs`, `.github/workflows/gt1-e2e.yml`.

Semantics: old GT2 Task 9 (git CI and newest, `gix`, pygit2, JGit; clone, fetch after push, push variants, `--depth 1`, `--filter=blob:none`; 200 seeded histories against `git upload-pack --stateless-rpc`).

Tests: the matrix rows; `differential_object_sets_match`.

Commit `ci: run the git client matrix and the upload-pack differential suite`.

### Task 27: v0/v1 upload-pack (only if Task 0 finds a matrix client without v2)

**Files:** `src/protocol/{advertise.rs,v0.rs}`. Semantics and tests: old GT2 Task 10 (`v0_clone_and_fetch`, plus the needing client's matrix rows). If not needed, record "not needed" as a ruling and skip.

Commit `git: serve protocol v0 upload-pack for older clients`.

## GT1e — Compaction, GC and integrity (Tasks 28–30)

### Task 28: Compaction

**Files:** `src/compact.rs`, the `loams-worker` task source in `crates/loams/src/server.rs`, `tests/compact.rs`.

Semantics: §36 §7 and old GT2 Task 7's compaction half: the task `git-compact/<ns>/<repo_id>` (lease `task/git-compact/<ns>/<repo_id>`) when more than 16 packs sit above the `--geometric=2` progression, or on `CompactRepo`; Ruling 13's mirror; `git repack --geometric=2 -d --write-midx --write-bitmap-index` and `git commit-graph write --reachable --split=no`; upload new `.lpk`s, `midx/`, `commit-graph/`; commit a `PackSetChange` through the owner, fenced on the starting pack set; checkpoint after. `loams_git_cpu_seconds_total{op="compact"}` from the child's rusage.

Tests: `repack_preserves_every_reachable_object`; `readers_see_old_or_new_pack_set`; `concurrent_push_during_compaction_keeps_new_pack`; `compaction_lease_is_exclusive`; `compaction_mirror_deleted_mid_run_recovers`; `repack_time_per_mib_recorded`.

Commit `git: compact repositories with git repack`.

### Task 29: Pack GC, fork-family reachability and repository deletion

**Files:** `src/pack_gc.rs`, `tests/pack_gc.rs`.

Semantics: daily task `git-gc/<ns>`. A pack removed by a `PackSetChange` is deleted once no checkpoint inside `EXACTLY_RETENTION` and no fork at a seq that names it references it, after `GC_GRACE`, with a GC claim under `gc/claims/` (§03 §7, D59). Fork families are found from checkpoint 0's `parent` (a per-namespace index object `ns/<ns>/repos/_forks/<parent_repo_id>/<child_repo_id>`, written by `ForkRepo`). A `Deleting` repository with no live forks has every object under its prefix deleted after `GC_GRACE`, then `RepoCatalog::purge`.

Tests: `pack_gc_respects_grace_and_forks`; `fork_family_reachability_keeps_parent_packs`; `gc_claim_blocks_second_collector`; `deleted_repo_is_purged_after_grace`; `deleted_parent_with_live_fork_keeps_packs`.

Commit `git: collect retired packs and deleted repositories`.

### Task 30: Integrity scrubs

**Files:** `src/scrub.rs`, `tests/scrub.rs`, `gt1-nightly.yml`.

Semantics: §36 risk 3. A nightly worker task samples repositories (1% or at least 10 per namespace), hydrates a mirror, runs `git fsck --full --connectivity-only` and checks every live pack's footer CRC and every checkpoint's CRC; findings go to `loams_git_scrub_failures_total` and an alert (Task 37).

Tests: `scrub_detects_missing_pack`; `scrub_detects_corrupt_footer`; `clean_repo_scrubs_clean`.

Commit `git: scrub sampled repositories nightly`.

## GT1f — Build cache and crates mirror (Tasks 31–36)

Old GT3, unchanged in substance except: `CacheGrant` is derived from Task 18's `GitGrant` (`cache_class`, `repo`, `principal`), the class still comes from the credential only; `TokenResolver` is satisfied by `GitAuthorizer`; refresh-on-hit uses Task 1's `Store::copy`; the sweeper uses `list_page`.

### Task 31: Keys, trust classes and the WebDAV subset

Files, interfaces and semantics: old GT3 Task 1 (`TrustClass`, `CacheGrant`, `CacheKeys`, `router`, `CacheConfig`; `--cache-listen`). Tests: `put_then_get_round_trip`; `head_reports_length`; `miss_is_404`; `untrusted_put_to_trusted_is_403`; `scratch_is_private_to_its_principal`; `untrusted_reads_trusted`; `read_from_parent_repo`; `class_comes_from_token_not_path`; `other_namespace_in_url_is_404`; `oversized_put_is_413`; `non_loopback_is_refused`. Commit `cache: serve sccache's WebDAV backend with trust classes`.

### Task 32: Refresh-on-hit, the sweeper and the hooks

Old GT3 Task 2 (lease `task/buildcache-sweep/<ns>`). Tests: `hit_on_old_entry_refreshes_once`; `fresh_hit_does_not_copy`; `sweeper_deletes_expired`; `sweeper_enforces_quota_oldest_first`; `sweeper_lease_is_exclusive`; `hits_misses_puts_are_counted`. Commit `cache: add approximate LRU, the sweeper and the hooks`.

### Task 33: The direct path

Old GT3 Task 3 (`DirectRecipe`, `CredentialVendor`, `R2Vendor`, `StaticVendor`, `recipe`); credentials are the boundary, never sccache settings. Tests: `recipe_for_untrusted_is_read_only`; `r2_vendor_request_body_matches_api`; `r2_vendor_never_requests_admin_permissions`; `static_vendor_per_class`; `static_vendor_refuses_undeclared_scope`; `untrusted_key_cannot_write_or_read_outside_prefix`; `vended_secret_not_in_recipe_debug`. Commit `cache: add the direct path's credential vending`.

### Task 34: CI templates and the end-to-end builds

Old GT3 Task 4. Tests: `cold_then_warm_hit_rate` (≥ 90% of cacheable compilations); `untrusted_ci_cannot_write_trusted`; `direct_path_on_rustfs`; `multilevel_disk_then_webdav` (if Task 0 confirmed the chain). Commit `ci: add the sccache template and the build-cache end-to-end job`.

### Task 35: The crates mirror

Old GT3 Task 5 (`RegistryConfig`, `Policy`, `router`; `--registry-listen`; audit records `io.loams.dev.packages.download.v1` to `_packages`). Tests: `config_json_has_dl_template`; `index_is_cached_and_revalidated_with_etag`; `index_lines_are_served_verbatim`; `invalid_index_path_is_404`; `crate_is_fetched_once_and_content_addressed`; `crate_with_wrong_cksum_is_refused_and_not_stored`; `download_of_hidden_version_is_404`; `denied_crate_is_404`; `pinned_versions_only`; `quarantined_version_is_hidden`; `audit_record_per_download`; `package_requests_are_counted`. Commit `registry: add the crates.io sparse-index mirror`.

### Task 36: The mirror end to end

Old GT3 Task 6. Tests: `cargo_fetch_through_mirror`; `cargo_cannot_reach_upstream_directly`; `second_fetch_is_all_cache`. Commit `registry: run cargo end to end through the mirror`.

## GT1g — Production operations (Tasks 37–45)

### Task 37: Metrics, traces, dashboards and alerts

**Files:** `crates/loams-git/src/metrics.rs`, the `/metrics` route (Ruling 29), `deploy/observability/loams-git/{dashboards/,alerts.yaml}`, `scripts/ci/no-metering.sh` (extended to the new crates).

**Interfaces:** §36 §11's families exactly, with labels `org` and `namespace` only, plus operational families: `loams_git_commit_latency_seconds` (histogram), `loams_git_fences_total`, `loams_git_owner_fallbacks_total`, `loams_git_sequencers_active`, `loams_git_scrub_failures_total`, `loams_git_mirror_lag_segments`, `loams_git_compaction_seconds`. Spans: `git.receive`, `git.upload`, `git.commit` (with `seq`, group size), `git.compact`; `traceparent` from the request reaches the WAL event. Alerts: commit p99 over 2 s for 10 min; fence rate over 1/s per node; mirror lag over 256 segments; any scrub failure; compaction backlog (repositories over 64 packs).

Tests: `metric_families_match_design`; `no_repo_label_anywhere`; `traceparent_reaches_wal_event`; `every_alert_fires_in_test` (promtool rules tests); `no_metering_guard_covers_git_crates`.

Commit `git: export usage hooks, traces, dashboards and alerts`.

### Task 38: Quotas and limits

**Files:** `crates/loams-git/src/limits.rs`, `crates/loams/src/api/{repos.rs,git_http.rs}`, `tests/receive.rs`.

**Interfaces:** per-namespace limits from namespace configuration (as built; Task 0): `max_repos` (default 10,000), `max_stored_bytes` (default 100 GiB, from checkpoints' live pack sets), `max_push_rate_per_repo` (default 60 pushes/min, token bucket on the owner), `max_concurrent_uploads_per_ns` (default 64). Refusals: `quota_exceeded` on the API, `quota exceeded` in report-status, `429` with `Retry-After` on HTTP.

Tests: `repo_count_limit_refuses_create`; `stored_bytes_limit_refuses_push`; `push_rate_limit_returns_429_with_retry_after`; `upload_concurrency_limit`; and each `limits` constant's at-limit and past-limit test.

Commit `git: enforce quotas and limits`.

### Task 39: The failure table, deterministic simulation and a chaos soak

**Files:** `tests/sim.rs` (with `loams-sim`), `scripts/git/chaos.sh`, `gt1-nightly.yml`, `docs/runbooks/loams-git/failures.md`.

**Semantics:** the failure table, each row with a test and an expected outcome:

| Failure | Expected |
|---|---|
| Sequencer owner killed mid-PUT | Unknown outcome resolved by read-back on the next owner; no lost or doubled ack |
| Lease lost while a PUT is in flight | The append still commits or fences; the zombie answers `Unavailable` after |
| Store returns 5xx/429 storm for 60 s | Pushes slow and retry; none acknowledged without a segment |
| Network partition between gateway nodes | Forwarding falls back to fenced local commits |
| Compaction worker killed after upload, before `PackSetChange` | Orphan packs collected after grace; nothing removed |
| GC races a fork creation | The fork's parent packs survive (the `_forks` index is written before checkpoint 0) |
| Node disk full (spill) | Push refused with `unpack disk full`; no WAL write |
| Clock skew of ±5 min between nodes | Idempotency windows and grace use the sequencer's clock; tested with `ManualClock` |

Plus a `loams-sim` workload (pushers, readers, forks, compaction, GC, crashes, store faults) checked with the linearizability checker and an "every acked push is reachable" invariant, and a 6-hour chaos soak against RustFS in CI (kill -9 of gateway and worker nodes at random).

Tests: one per table row (`owner_killed_mid_put_no_lost_ack`, `lease_lost_inflight_put`, `store_storm_no_unacked_commit`, `partition_falls_back_fenced`, `compaction_killed_before_packset_change`, `gc_races_fork_creation`, `spill_disk_full_refuses_push`, `clock_skew_window_uses_sequencer_clock`); `sim_10k_seeds_no_lost_ack` (nightly; 256 seeds on PRs); `chaos_soak_6h` (scheduled).

Commit `git: add the failure table, simulation and chaos soak`.

### Task 40: Backup, disaster recovery and the restore drill

**Files:** `scripts/git/restore-drill.sh`, `docs/runbooks/loams-git/{restore.md,bucket-versioning.md}`, `tests/checkpoint.rs` (rebuild part).

**Semantics:** a repository is rebuilt from the bucket alone (no node state): a fresh node opens it from `head` or a LIST of `checkpoints/`. The drill: replicate a bucket prefix to a second RustFS (or provider replication), delete the primary, point a fresh `loams dev --git` at the replica, clone every sampled repository and compare ref sets and `git fsck` with the source. Point-in-time reads use `Exactly(seq)` inside `EXACTLY_RETENTION`; longer history relies on bucket versioning (documented, not built).

Tests: `rebuild_from_bucket_only`; `rebuild_without_hint_uses_list`; `restore_drill_matches_source` (CI, twice green before GA).

Commit `git: add the restore drill and recovery runbooks`.

### Task 41: The security review

**Files:** `docs/security/loams-git-threat-model.md`, `crates/loams-git/fuzz/`, `scripts/ci/git-licence.sh`, `gt1-nightly.yml`.

**Semantics:** the threat model (STRIDE per entry point: Smart HTTP, the helper, the API, the internal route, the cache, the mirror, the compaction worker running `git` on tenant data). Fuzz targets: `pktline`, `receive_pack` (a pack and commands through verification with an in-memory store; invariant: no panic, no WAL write for an invalid pack), `segment`, `checkpoint`. Hardening checks: ref names with `..`, `@{`, control bytes; deflate bombs (an entry inflating past `MAX_PACK_BYTES` × 4 is refused); delta chains deeper than 50 refused; path traversal in repo names; `git` subprocesses run with `GIT_CONFIG_NOSYSTEM=1`, `HOME` set to an empty directory, `protocol.allow=never` except `file`, and no hooks (`core.hooksPath=/dev/null`), so tenant data cannot run code on the worker. `git-licence.sh` fails on `git2`, `libgit2-sys` or any GPL crate in `Cargo.lock`.

Tests: `fuzz_pktline`, `fuzz_pack_receive`, `fuzz_segment`, `fuzz_checkpoint` (nightly, 30 min each; corpora checked in); `deflate_bomb_is_refused`; `deep_delta_chain_is_refused`; `hostile_refnames_are_refused`; `compaction_git_runs_without_hooks_or_config`; `no_libgit2_in_lockfile`. An external review closes every high and critical finding before GA.

Commit `git: threat model, fuzzing and hardening`.

### Task 42: The performance gate

**Files:** `bench/git-wal/`, `scripts/git/bench.sh`, `docs/design/36-loams-git.md` (§4.4 and §12 measured).

**Gate** (on RustFS on the reference machine, and on R2 when credentials exist, GT-Q8, GT-Q12):
- 30 pushes/s sustained on one hot repository through Smart HTTP, p99 push latency under 1.5 s (RustFS) and the measured R2 figure recorded;
- 1,000 repositories with one push each per minute, sustained for 1 h, with sequencer eviction working;
- a cold clone of a 1 GiB compacted repository within 1.5× `git clone` from a local `git http-backend`;
- request counts per push (PUTs, GETs) within §36 §12's model, checked in CI as a cost regression test (§12 §2 item 11).

Tests: `bench_hot_repo_30_per_s`; `bench_many_small_repos`; `bench_cold_clone_1gib`; `requests_per_push_within_model`.

Commit `bench: the GT1 performance gate`.

### Task 43: Single-node mode and deployment

**Files:** `crates/loams/src/main.rs` (`loams dev --git`), `deploy/loams-git-dev/compose.yaml`, `docs/guides/git.md`.

**Semantics:** `loams dev --git` serves Smart HTTP, `RepoService` and the worker tasks on loopback over the dev store (local `file://` or RustFS from the compose file), with `--git-tokens` generated on first run. Kubernetes: the gateway and worker roles of the `loams` chart, if Task 0 found one; otherwise GT-Q7 decides, and this task ships only the compose file.

Tests: `dev_git_one_command_clone_push` (CI); `compose_up_clone_push_down`.

Commit `deploy: loams dev --git and the compose stack`.

### Task 44: CLI and SDK exposure

**Files:** the `loams` CLI as built (`loams repo create|list|fork|delete|refs|compact`, `loams git credential` as a git credential helper), the SDK templates' package maps (as PG2's R1.3 found), `crates/loams/src/api/connect.rs`.

Tests: `cli_repo_lifecycle`; `git_credential_helper_returns_token`; `sdk_package_maps_name_repos`.

Commit `cli: repository commands and the git credential helper`.

### Task 45: Docs, runbooks and plan close

**Files:** `docs/design/36-loams-git.md` ("As built (GT1x)" notes; measured tables; Rulings 20–30 folded into §36 as amendments with D-numbers assigned at merge), `docs/runbooks/loams-git/` (sequencer stuck, fence storm, mirror lag, compaction backlog, scrub failure, restore), `docs/guides/{git.md,build-cache.md,crates-mirror.md}`, the limits table, `CHANGELOG.md`, `docs/plans/README.md`, this plan's rulings. `loams.repos.v1` loses `unstable` when the exit criteria are green.

Commit `docs: record Loams Git as built and close GT1`.

---

## Exit criteria for production (with the owning tasks)

- [ ] **WAL core:** formats golden, fencing and idempotency proven, linearizable under faults: Tasks 5–11.
- [ ] **Serverless helper:** clone, fetch, push, fork through `loams::`; concurrent pushers lose nothing: Tasks 12, 25.
- [ ] **API:** `loams.repos.v1` served, linted, idempotent, with reasons and route-map rows: Tasks 14, 15.
- [ ] **Ownership:** one leased sequencer per repository, forwarding, fenced fallback, eviction: Task 16.
- [ ] **Events:** `_git` mirror with repair: Task 17.
- [ ] **Auth:** tenant from credential, scopes enforced; MT1 integration for non-loopback serving: Task 18 (and MT1).
- [ ] **Smart HTTP:** the client matrix and the differential suite green: Tasks 19–27.
- [ ] **Compaction, GC, integrity:** no object lost across forks; nightly scrub clean: Tasks 28–30.
- [ ] **Build cache and mirror:** hit rate ≥ 90% warm, untrusted cannot write, cargo through the mirror only: Tasks 31–36.
- [ ] **Observability:** families exported, dashboards, every alert tested: Task 37.
- [ ] **Quotas:** every limit refused in a test: Task 38.
- [ ] **HA and DR:** the failure table green, simulation seeds clean, chaos soak clean, the restore drill green twice: Tasks 39, 40.
- [ ] **Security:** threat model, fuzzing clean, hardening tests, external review closed for high and critical, licence check: Task 41.
- [ ] **Performance:** the Task 42 gate on RustFS, with R2 recorded: Task 42.
- [ ] **Deployment and docs:** single node in one command, CLI, runbooks, guides: Tasks 43–45.

## Self-review

- **Spec coverage.**

  | §36 section | Task(s) |
  |---|---|
  | §4.1 Layout | 4, 15 (catalog), 29 (`_forks`) |
  | §4.2 Commit protocol | 7, 10 |
  | §4.3 Formats | 3, 5 |
  | §4.4 Sequencer and group commit | 10, 16 |
  | §4.5 Idempotency | 8, 12, 24 |
  | §4.6 Reads | 10, 15, 20 |
  | §4.7 Forks | 11, 15, 29 |
  | §4.8 Event-stream mirror | 17 |
  | §5 Traits | 6, 7, 9, 10, 21 (`Materializer`: GT4) |
  | §6.1 Smart HTTP | 19–23, 26, 27 |
  | §6.2 `git-remote-loams` | 12, 25 |
  | §6.3 Receive path | 24 |
  | §6.4 `loams-vfs` | out of scope (GT4); `GitGrant.cones` carried |
  | §7 Compaction and GC | 11, 28, 29, 30 |
  | §8 Build cache | 31–34 |
  | §9 Package mirror | 35, 36 |
  | §10 Security | 18, 41 |
  | §11 Usage hooks | 32, 35, 37 |
  | §12 Cost | 13, 42 |
  | §14 Risks | 1 → 26; 2 → 42; 3 → 28–30; 5 → 31–34; 6–7 → 13, 42; 8 → 16; 9 → 41; 10 → Global Constraints (pins) |
  | §17 `Fs` and providers | 2, 33 |

- **Types.** `WalStore`, `RefLog`, `BlobStore`, `ObjectDb`, `GitAuthorizer`, `RepoCatalog`, the API and the limits are defined once, in the shared contracts.
- **Review Focus.** Items 1–12 each name owning tests (Tasks 2, 5, 7, 9–12, 14, 16, 18, 23, 24, 28–31, 33–35, 39, 41).
- **Carried from the superseded plans.** Every test name of the 2026-10-01 GT1, GT2 and GT3 plans appears here, except `first_push_creates_repo` (now `first_push_creates_repo_with_flag`, Ruling 7).

## Open questions

Plan-local numbers; Task 45 assigns Q-numbers in the decision log when they are answered or folded into §36.

| # | Question | Default if unanswered | Owner | Needed by |
|---|---|---|---|---|
| GT-Q1 | **Timing.** D411 keeps track GT after M3, and M3 has no plan on `dev`. Start this production plan now, or keep the slot? | Keep D411's slot; Task 0 waits | Founder | Task 0 |
| GT-Q2 | **API package.** `loams.repos.v1` for the public API, with `loams.git.v1` kept as the on-disk format package (Ruling 20), or one `loams.git.v1` package generated once in `loams-proto` (making `loams-git` depend on buffa and `loams-proto`)? | `loams.repos.v1` | Eng + Founder | Task 14 |
| GT-Q3 | **Catalog location.** Bucket-native name records (Ruling 21) or new `MetaStore` methods on every backend? | Bucket | Eng | Task 15 |
| GT-Q4 | **Git LFS at GA.** §36 §2.2 leaves LFS to §15 §3.2's W1 plan (batch API on the namespace CAS). In this plan or a follow-up? | Follow-up plan; GA without LFS | Founder | Task 45 |
| GT-Q5 | **Webhooks at GA.** §36 promises "webhooks and mirroring only"; this plan ships the `_git` stream. Push webhooks through links and notifications, or later? | Later; consumers read `_git` | Founder | Task 45 |
| GT-Q6 | **GA and MT1.** Is a loopback-only GA acceptable for single-node and desktop use, with networked serving after MT1, or does GA wait for MT1 (Ruling 30)? | GA waits for MT1 | Founder | Task 18 |
| GT-Q7 | **Kubernetes packaging.** No chart in `deploy/` at `1dc6e8a3`. A `loams` chart here, `loams-platform`'s, or compose only at GA? | Compose only; the chart follows the platform's | Founder | Task 43 |
| GT-Q8 | **Credentials** for R2, S3 Standard and S3 Express One Zone (Q384's measurement and Task 42's R2 figure) | RustFS and in-memory only; R2 recorded "not measured" | Founder | Tasks 13, 42 |
| GT-Q9 | **Superseding the 2026-10-01 plans** and reusing the name GT1 for the production track | Supersede; the old names map as in the header | Founder | Task 0 |
| GT-Q10 | **Exporter.** Prometheus text on `/metrics` (Ruling 29) until §27's exporter exists, or build §27's OTLP exporter first (Q-UH-1)? | Prometheus text | Eng | Task 37 |
| GT-Q11 | **Deleting a forked parent.** Refuse (`repo_deleting` until forks are gone), or detach forks by copying the packs they reach? | Refuse | Founder | Task 15 |
| GT-Q12 | **The GA performance bar.** 30 pushes/s on one hot repository is a draft figure (§36 D391). Gate on RustFS only, or also on R2? | RustFS gate, R2 recorded | Founder | Task 42 |

## Rulings made during execution

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| — | (Task 0 fills this table; later tasks append) | | |
