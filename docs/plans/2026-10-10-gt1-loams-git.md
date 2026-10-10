# GT1 — Loams Git in Production (Monorepo Mode) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact names, paths, constants, messages or defaults, use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file. The code is not pre-written in this plan (M0.3 Ruling 1).
>
> **Status: Planned** (2026-10-10; **rewritten the same day for monorepo mode**). Track GT. Design: [§36](../design/36-loams-git.md), meaning D388–D399, D381 and D382 (amended by D411 and D415), plus **§19 "Monorepo mode"** (proposed D825–D847, Q750–Q760). The owner's direction of 2026-10-10 reads: *"loam git should be similar to google monorepo, download only touched file, spawn million of worktrees on s3, it should wrap git command and take over"*. The owner then added **secrets as code** in `env/` (§36 §19.13). This rewrite replaces the morning's version of this plan. That version had 46 tasks and no workspaces, no virtual filesystem, no shim and no env. No task of either version has started.
>
> **This plan supersedes the three 2026-10-01 plans** [GT1](2026-10-01-gt1-wal-git-core.md), [GT2](2026-10-01-gt2-smart-http.md) and [GT3](2026-10-01-gt3-build-cache-and-mirror.md), and §36's unplanned GT4. None of them started: `dev` at `814d6dac` has no `loams-git`, `loams-git-remote`, `loams-fs`, `loams-vfs`, `loams-vault`, `loams-buildcache` or `loams-registry` crate, and no `proto/loams/git/`. Their task contracts and test names are folded in by reference ("old GT2 Task 4") and keep their names, so reviews that cite them still apply. GT5 (NVMe replicas, gossip, any-node writes) stays out of scope. Task 0 marks the old plans superseded in `docs/plans/README.md`. Writing this plan changed only this file and §36.
>
> **Schedule: start now** (D825, superseding D411; the coordinator's brief resolved GT-Q1 on 2026-10-10). GA is loopback and local-first before MT1 (D837).

**Goal:** Loams Git GA in monorepo mode: a Piper/CitC-style monorepo on object storage, for people and agent fleets. It delivers:
- the §36 §4 WAL core (create-only segments as the commit point, checkpoints, one-object packs, a per-repository sequencer with group commit, fencing, idempotency, O(1) forks);
- **server-side workspaces** (§19.4): a base commit plus an overlay, saved as a create-only snapshot chain on the bucket. Creating one is O(1), millions are cheap, and any machine or agent can mount any workspace. There is a snapshot per save, a single writer by fencing, takeover and salvage;
- **`ObjectService`** (§19.5): lazy batched object and tree reads with sizes, has-checks, workspace packs, and derived data (tree aux, commit graph with Bloom filters, blame) for server-side `log`, `blame`, `grep` and `diff --stat`;
- **`loams-vfs`** (§19.6): a FUSE daemon that shows the whole tree, fetches contents on first read, caches per host, uploads only touched files, keeps status O(changed), and redirects build directories;
- **the `git` shim** (§19.7): installed as `git`. It passes through outside workspaces, runs the common porcelain natively inside them, and falls through to real git on a projected `.git` with a notice. `git clone` and `git worktree add` create workspaces;
- **interop**: Smart HTTP (v2 upload-pack, v0/v1 receive-pack), `git-remote-loams`, partial and sparse clone, hidden workspace refs;
- **env secrets as code** (§19.13): sealed `env/` files, a KMS-backed vault with external adapters, environments promoted on merge, rotation without commits, redaction and audit, key-level review, leak scanning, a plain-git filter;
- the sccache build cache and the crates.io mirror (§8–§9);
- production operations: metrics, quotas, a failure table with simulation and chaos, a restore drill, a security review with fuzzing, a performance gate, single-node mode, CLI and docs.

The exit is "Exit criteria for production" at the end of this plan, with the owning tasks. Tasks 83–87 come after GA.

**Architecture** (§36 §4–§9, §19.3):
- **`crates/loams-git`** is the portable core. It holds the formats, the traits (`WalStore`, `RefLog`, `BlobStore`, `ObjectDb`, `WorkspaceStore`), the state machine, `BucketRefLog`, the workspace store, derived data, history queries, and the transport-free protocol module. It depends on `loams-store`, `loams-cloudevents`, `loams-fs` and gitoxide crates only, and has no listener.
- **`crates/loams-vfs`** is the client library plus the binary `loams-vfsd`. It contains the local store, the backends (`ApiBackend` over `loams.repos.v1`, `DirectBackend` over `loams-git` and the bucket), the FUSE adapter (`fuser`), the save pipeline, the projection and the control socket.
- **`crates/loams-git-porcelain`** implements git's porcelain over a workspace view: the argument allowlists and output that is byte-identical to git. **`crates/loams-git-shim`** builds `loams-git`, which is installed as `git`. It is a thin client of vfsd.
- **`crates/loams-git-remote`** builds `git-remote-loams`. **`crates/loams-env`** holds the sealed format, fingerprints, the key-level diff and merge, the scanner and the git filter process. **`crates/loams-vault`** holds versions, KMS wrapping, `SecretBackend` and the adapters.
- **The `loams` binary** gains the off-by-default features `git` (`RepoService`, `WorkspaceService`, `ObjectService`, `EnvService`, Smart HTTP, sequencers, and the worker tasks `git-compact`, `git-gc`, `git-scrub`, `env-promote` and `env-rotate`), `buildcache` and `registry`.
- **Correctness never depends on one sequencer or one writer.** Repository commits are fenced by the create-only segment PUT (§36 §4.2), workspace saves by the create-only snapshot PUT (§19.4.3), and environment revisions by the create-only revision PUT (§19.13.4).
- **Object storage is the source of truth** (D1). The host cache, the overlay's uploaded files, the projection, pack caches, the H1 range cache, compaction mirrors and spill files are rebuildable. The one exception is overlay data not yet saved, which is local until its snapshot commits (§19.5.3).
- **Stock `git` is a process, never a library** (D394, D838). It runs in the shim's fallthrough, in the helper, in the compaction worker and in the tests.

**Tech Stack:**
- Rust 1.97 (workspace `rust-version`), edition 2024, workspace lints (`unsafe_code = "forbid"`: FUSE's `unsafe` stays inside `fuser`).
- gitoxide crates from one release train, at least 14 days old (Task 0 records exact versions; `gix` 0.88.0 of 2026-09-25 qualifies): `gix-hash`, `gix-validate`, `gix-object`, `gix-pack`, `gix-packetline` (async), `gix-commitgraph`, `gix-features`, `gix-config`, `gix-index` (the projection's sparse index), `gix-ignore`, `gix-blame` (if Task 0 finds it adequate), with `default-features = false` and only `sha1`. MIT OR Apache-2.0.
- New permissive dependencies: `fuser` (MIT; Task 0 pins it and confirms that it mounts through `fusermount3` without linking libfuse), `grep-regex`/`grep-searcher` and `ignore` (MIT OR Unlicense), `dotenvy` (MIT), `aes-gcm` and `hmac` (MIT OR Apache-2.0), `similar` only as a test oracle (Apache-2.0; the native diff is Loams' own Myers implementation, checked byte for byte against git).
- Workspace crates already on `dev`: `loams-store`, `loams-cloudevents`, `loams-stream-grpc`, `loams-meta-conformance`, `loams-worker`, `loams-hot::placement`, `loams-cache::RangeCache`, `loams-sim`, `loams-proto` (connectrpc 0.9, buffa 0.9), `loams-durable` (promotion and rotation workflows), `rusqlite` 0.32 (the workspace pin; vfsd's `meta.sqlite`).
- Workspace dependencies: `prost`/`prost-build` 0.14 (on-disk formats), `crc32c` 0.6, `sha2` 0.11, `axum` 0.8, `reqwest` 0.13, `object_store` 0.14, `tokio`, `bytes`, `futures`, `async-trait`, `thiserror`, `tracing`, `proptest`.
- Tools run as processes and never linked: `git` (the CI image's and the newest release, both ≥ 2.45; the projection needs sparse index and fsmonitor), `fusermount3`, the `gix` CLI, `pygit2` from a `uv` venv, JGit's CLI jar, `sccache` v0.18.0, `cargo`, `rustfs/rustfs:1.0.x` (D61), OpenBao (dev-mode container, for the adapter suite), `cargo-fuzz` (nightly, CI only), `buf`, `promtool`.

**Spec:**
- [§36](../design/36-loams-git.md) (all; §19 for monorepo mode and env), D388–D399, D411, D415, Q384–Q395, and proposed D825–D847 and Q750–Q760.
- [§15](../design/15-agent-workspaces.md) §3, §5, §6, §7, §8, §11, W1's client matrix; [§50](../design/50-loams-desktop-daemon.md) (agentd, D788, D790) and [DD1](2026-10-09-dd1-desktop-daemon.md) (`CreateWorktree`); [§39](../design/39-software-factory-and-loams-bot.md) D-SF-18.
- [§02](../design/02-stream-engine.md) §7.4 (D270), [§03](../design/03-storage-formats.md) §6–§7, [§04](../design/04-hot-tier.md), [§09](../design/09-links-and-workers.md), [§10](../design/10-operations.md) (D96, `KeyProvider`), [§18](../design/18-metastore-backends-and-router.md) §5.3 (D75), [§24](../design/24-cpu-time-runtime.md) §5 (D189), [§27](../design/27-usage-hooks.md), [§44](../design/44-unified-api-and-sdks.md) §4–§8, [§46](../design/46-loams-postgres-production.md) (`ResetRolePassword`).
- As built (`dev` at `814d6dac`): the files the 2026-10-10 morning version listed (`crates/loams-store/src/{store.rs,fault.rs,error.rs}`, `crates/loams-cloudevents/src/lib.rs`, `crates/loams-stream-grpc/…`, `crates/loams-meta-conformance/src/linearizability.rs`, `crates/loams-worker/src/lib.rs`, `crates/loams-hot/src/placement.rs`, `crates/loams-cache/src/range_cache.rs`, `crates/loams/src/{main.rs,server.rs,api/connect.rs,api/internal.rs}`, `crates/loams-proto/build.rs`), plus `crates/loams-durable/`, `crates/loams-agentd-sessions/` (worktrees) and `crates/loams-agentd-rpc/`.

## Global Constraints

- **Worktree and branches.** Work in `~/Documents/Ostriumlabs/loams-wt/gt1-<milestone>` (for example `gt1d-workspaces`). There is one branch per milestone: `feat/gt1a-wal-core`, `feat/gt1b-repo-service`, `feat/gt1c-object-service`, `feat/gt1d-workspaces`, `feat/gt1e-vfs`, `feat/gt1f-shim`, `feat/gt1g-interop`, `feat/gt1h-compaction`, `feat/gt1i-agents`, `feat/gt1j-env`, `feat/gt1k-cache-mirror` and `feat/gt1l-ops`. Each is based on `dev`, with stacked PRs targeting `dev`. Use `git commit -s` (DCO). Never `git stash`. Commit areas: `git`, `vfs`, `shim`, `env`, `vault`, `store`, `fs`, `events`, `proto`, `api`, `worker`, `cache`, `registry`, `bench`, `ci`, `deploy`, `docs`.
- **Rust builds** use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR` and never build in `/tmp`. Run one cargo build at a time on the build machine (D127). Build the touched crates (`cargo test -p loams-git`), and run `cargo test -p loams --features git` only before a commit that touches `loams`. Never `--workspace --all-features` locally. FUSE suites, RustFS, OpenBao, the client matrix and the chaos soak run in CI, or locally only when no cargo build is running. Stop and report if `/home` has under 8 GB free.
- **The default build does not change.** On default features, `cargo tree -p loams -e normal` must list none of `gix-pack`, `loams-git`, `loams-vfs`, `loams-env`, `loams-vault`, `loams-buildcache` or `loams-registry` (Task 12 test `default_features_exclude_git`).
- **Never link copyleft code** (D11, D394, D838). `scripts/ci/git-licence.sh` (Task 78) refuses `git2`, `libgit2-sys` and any GPL or AGPL crate. **No code is copied from Sapling, EdenFS, Mononoke, git or git-crypt**: every PR that implements a §19.2 idea cites the public document it worked from, never source files.
- **Object storage is the source of truth.** No state outside the bucket is read for correctness. Every local cache is keyed by content or by a sequence number and can be deleted at any time; each task that adds a cache has a test that deletes it mid-run. vfsd's unsaved overlay is the only local state that matters, and Task 32's journal-replay tests cover it.
- **Plaintext secrets never leave the editing host.** No `env/` value appears in a git object, a workspace pack, a snapshot, a WAL event, a CloudEvent, a log line, an error, a metric label, a trace attribute, a report-status line or a test snapshot file. Each env task has a test that greps every byte written to the store (in-memory store dump) for the plaintext fixtures (`no_plaintext_in_store`).
- **One limits module per crate.** Every constant this plan names lives in `loams_git::limits`, `loams_vfs::limits`, `loams_env::limits` (or the cache and registry crates' modules), with a doc comment naming its § and a row in the limits table (D88). Each limit has a test at the limit and one past it.
- **AP0 API rules** (§44) for `loams.repos.v1`: every mutation has `idempotency_key = 15`; reads are `NO_SIDE_EFFECTS`; errors are `loams.errors.v1` with reasons registered in `docs/api/reasons.md`; pagination is `page_size`/`page_token`; watch streams send a snapshot, then changes, then a heartbeat every 15 s.
- **Tenant from the credential** (D182). The namespace comes from the token, never from the URL alone. A URL that names another namespace answers `404` with no detail.
- **Loopback and local-first until MT1** (D111, D837). Every new listener refuses a non-loopback address with `<listener> listen on <addr>: only loopback addresses are served until the unified auth plan (D111)`. Authorization goes through `GitAuthorizer` (Task 16). vfsd's control socket is `$XDG_RUNTIME_DIR/loams/vfsd.sock` (mode 0600; D788's rules: no TCP, peer uid checked).
- **No billing** (D393). No field, metric, table or endpoint may be named `plan`, `price`, `invoice`, `credit`, `meter`, `billable` or `usage`. `scripts/ci/no-metering.sh` covers every new crate (Task 74).
- **Secrets.** Tokens, vended credentials, vault values, data keys and `repo_fp_key` are `Secret<T>`, whose `Debug` and `Display` print `[redacted]`.
- **Pins.** Exact versions for every new crate, image and tool, at least 14 days old. Record each pin in the task's commit message.
- **Docs and code stay in step.** A change to a §36 format, constant or rule updates §36 in the same PR, marked "As built (GT1x)".

## Rulings made while writing this plan

Rulings 1–30 are the morning version's (old GT1 1–8, GT2 9–14, GT3 15–19, production 20–30). They stand except where marked. Rulings 31–50 are monorepo mode's and env's.

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| 1–6, 8–19 | As the morning version of this plan (blob multipart rule; the helper as its own sequencer; helper capabilities; whole-pack helper fetch; helper fast-forward checks; helper idempotency keys; SHA-1 only; v2-only upload-pack; `ls-refs` from `snapshot(Latest)`; delta reuse only; per-ref `RefTxn`s; compaction mirrors; `packfile-uris` off; WebDAV cache path; refresh-on-hit; per-repository cache keys; one crates store; `auth-required`) | Unchanged by monorepo mode | As recorded there |
| 7 | A serverless `loams::` push to a missing repository fails unless `LOAMS_GIT_CREATE_ON_PUSH=1` | A typo must not create a repository | One extra call |
| 20 | `loams.repos.v1` is the public API (now with `WorkspaceService`, `ObjectService` and `EnvService`); `loams.git.v1` is the on-disk format package (prost, never exposed). **Resolves GT-Q2** | Generated once; the core stays portable | A rename before GA |
| 21 | The repository catalog, workspace names and environment revisions live **in the bucket** (create-only and ETag CAS), not the metastore. **Resolves GT-Q3** | D1; portable; the `MetaStore` has no generic records | Listings are LISTs |
| 22–29 | As the morning version (numeric `NamespaceId` in paths; the CloudEvents codec in `loams-cloudevents`; `loams-fs` built minimally; `loams-store` additions; `ResourceKind::Repo`; forwarding over the internal HTTP route; idle sequencer eviction; Prometheus text on `/metrics`, **resolving GT-Q10**) | Unchanged | As recorded there |
| 30 | **Changed:** GA serves loopback and local-first (`DirectBackend`) before MT1, with `GitAuthorizer` and env grants as seams; networked serving follows MT1 with no change to GT1's code paths. **Resolves GT-Q6** (D837) | Coordinator's brief, 2026-10-10 | None for local use; teams wait for MT1 |
| 31 | **Workspaces have no lease service.** The snapshot chain fences writers; takeover is a snapshot (D827) | One mechanism, the same as the WAL's; no server state | A takeover race costs one 412 and a salvage |
| 32 | **A snapshot is complete, not a delta** (§19.4.2), capped at 1 MiB; overlays live in tree objects inside workspace packs | Mounting is one GET; RustFS atomicity | A save writes about `k × depth` tree objects |
| 33 | **One FUSE session per user** at `~/loams`, workspaces as subdirectories; `git worktree add`/`clone` link into it; `--mount` gives a dedicated session | CitC's layout; thousands of agents on one session | Tools that canonicalize symlinks see `~/loams/...` paths |
| 34 | **The shim is native only on an exact allowlist** of subcommands, flags and config; everything else falls through. An unknown flag is never ignored | A wrong native answer is worse than a slow correct one | Some commands are slower until made native |
| 35 | **The projection is real files in the overlay's `.git`**, regenerated when stale and absorbed after real git exits (or at the next save) | Real git, libgit2 and IDEs see a normal partial-clone repository | Absorption must parse git's index (via `gix-index`) |
| 36 | **The native diff is Loams' own Myers implementation**, checked byte for byte against `git diff` on 10,000 seeded cases. `--patience`, `--histogram` and `--minimal` fall through | git's default output is what agents and tools parse | Rare tie-break differences found by the differential suite are bugs to fix |
| 37 | **`PushWorkspace` assembles packs on the server** from workspace packs (D831); the client never re-uploads | Saves already uploaded the objects | Server CPU per push |
| 38 | **Snapshot commits are deterministic** (author and committer `Loams Snapshot <snapshot@loams.invalid>`, time = `created_unix_ms` truncated to seconds, message `loams snapshot <ws_id>@<n>`) | Equal states give equal ids across hosts | None |
| 39 | **vfsd keeps `meta.sqlite` in WAL mode** with one writer task; the journal is a table, not a separate file | One durable local store; `rusqlite` is already pinned | SQLite write throughput bounds local save bookkeeping (more than 10k rows/s, ample) |
| 40 | **Unchanged files' mtime is the base commit's committer time**; files changed by a switch or rebase get the switch time | Build tools' mtime fingerprints stay valid | Tools that expect checkout-time mtimes see older times |
| 41 | **FUSE passthrough is used when the kernel offers it (≥ 6.9)**, else `FOPEN_KEEP_CACHE` | Native read speed for cached files | Older kernels pay a context switch per read |
| 42 | **env values are references, not ciphertext** (D839, Q757's default) | Revocable; nothing to brute-force in history | Reading needs the vault |
| 43 | **Fingerprints are HMAC-SHA256 truncated to 128 bits** under a per-repository key; sealing reuses a reference when the fingerprint is unchanged | Determinism and idempotency without vault calls | Equality of values within one repository is visible to people who can see fingerprints (reviewers); acceptable by design |
| 44 | **The vault's default backend stores version bodies in the bucket** under KMS-wrapped data keys; external backends store bodies and Loams keeps records only | D1 and D96 | Two code paths, covered by one conformance suite |
| 45 | **Promotion and rotation are `loams-durable` workflows** idempotent on (repository, environment, seq) and (secret, rotation number) | Crash-safe and exactly-once | A dependency on track D's runtime (on `dev`) |
| 46 | **The server is the authority for sealing and scanning**; client checks are early warnings | Clients can be modified | Refused pushes after local success when a client is outdated |
| 47 | **Agents carry no env grant by default** (D844); write-without-read is allowed and audited (Q760's default) | Values must not reach model transcripts | An extra grant step for agents that need values |
| 48 | **The secret-scanning rule set is generated from gitleaks' default TOML** at a pinned commit by `scripts/env/gen-rules.sh` and checked in as `crates/loams-env/rules/gitleaks.toml` with its MIT notice | Licence-clean reuse of a maintained rule set | Rule updates are a regenerate-and-review PR |
| 49 | **Kubernetes packaging, the External Secrets Operator provider, webhooks, the LFS-compatible read path, macOS and Windows are post-GA tasks 83–87.** **Resolves GT-Q4, GT-Q5 and GT-Q7** | Coordinator's brief, 2026-10-10 | None at GA |
| 50 | **The 2026-10-01 plans are superseded**, and the name GT1 is reused. **Resolves GT-Q9.** Deleting a forked parent is refused while forks live. **Resolves GT-Q11.** The performance gate runs on RustFS, with R2 recorded. **Resolves GT-Q12** | Defaults of the morning version | As recorded there |

---

## Review Focus

1. **No acknowledged push or save is lost, and none is applied twice.** Tests: Task 7 `append_is_exclusive_across_writers`; Task 10 `lost_ack_resolves_to_committed`, `four_sequencers_under_faults_linearizable`; Task 23 `snapshot_append_is_exclusive`, `lost_ack_save_is_committed`; Task 32 `crash_before_snapshot_replays_journal`; Task 76 `sim_10k_seeds_no_lost_ack`, `sim_no_lost_save`.
2. **Fencing holds** for sequencers, zombies, direct writers, and two mounts writing one workspace. Tests: Task 14 `zombie_owner_is_fenced`, `helper_and_server_race_one_repo`; Task 33 `two_writers_one_is_fenced_and_salvaged`, `takeover_fences_old_writer`.
3. **Formats are exact and versioned.** Tests: Task 5 `golden_segment_v1`, `golden_checkpoint_v1`; Task 22 `golden_snapshot_v1`; Task 61 `golden_sealed_env_v1`.
4. **Lazy means lazy.** Mounting, `ls -l`, `stat`, `git status`, `git switch` and `git pull` download no blob they do not need. Tests: Task 30 `stat_fetches_no_blob`; Task 37 `status_fetches_no_blob`; Task 43 `pull_downloads_only_changed_paths`; Task 58 `thousand_agents_share_cache`.
5. **The shim never gives a wrong native answer.** Every native command matches real git byte for byte on the differential corpus, and every unlisted flag falls through. Tests: Task 37 `unknown_flag_falls_through` (generated over every subcommand); Task 45 `agent_corpus_matches_real_git` (2,000 seeded states × the corpus).
6. **Fallthrough round-trips.** Real git on the projection, and its effects absorbed, give the same workspace state as the same operation on a real clone. Tests: Task 45 `rebase_via_fallthrough_absorbs`, `projection_is_valid_partial_clone` (`git fsck --connectivity-only` with promisor).
7. **Readers never see a partial state.** Tests: Task 10 `snapshot_is_a_prefix_of_commits`; Task 33 `read_only_follow_sees_whole_snapshots`; Task 53 `readers_see_old_or_new_pack_set`.
8. **Compaction and GC never lose an object**, across forks, workspace forks and in-flight pushes. Tests: Task 53 `repack_preserves_every_reachable_object`; Task 54 `ws_pack_gc_keeps_named_and_forked`, `gc_races_push_workspace`.
9. **Tenancy and scope.** No tenant reads another's repository or workspace; no token writes outside its branch prefix or cones. Tests: Task 16 `namespace_from_token_not_url`, `branch_prefix_scope_refuses_push`; Task 25 `other_owner_cannot_write_or_take_over`; Task 31 `write_outside_cones_is_eacces`.
10. **No plaintext secret is stored or leaked.** Tests: Task 62 `no_plaintext_in_store`, `redacted_for_unauthorised`, `agent_without_grant_sees_redacted`; Task 63 `unsealed_env_push_is_refused`, `foreign_reference_is_refused`; Task 67 `known_value_leak_is_refused`; Task 64 `plain_git_clone_sees_only_sealed`.
11. **Environments change only by promotion or rotation, exactly once.** Tests: Task 65 `merge_promotes_exactly_once`, `merge_never_rolls_back_rotation`; Task 66 `rotation_makes_no_commit`, `old_credential_revoked_after_grace`.
12. **Cache poisoning and supply chain.** Tests: Task 68 `untrusted_put_to_trusted_is_403`; Task 72 `crate_with_wrong_cksum_is_refused_and_not_stored`.
13. **Unbounded memory or disk.** Tests: Task 50 `two_gib_push_bounded_rss`; Task 49 `large_clone_bounded_rss`; Task 28 `cache_respects_budget`; Task 2 `spill_dir_removed_on_drop`.
14. **Copyleft or the default build.** Tests: Task 78 `no_libgit2_in_lockfile`, `no_gpl_crate`; Task 12 `default_features_exclude_git`.

---

## File structure

```
proto/loams/git/v1/{wal.proto,workspace.proto}                Tasks 5, 22 (on-disk; prost)
proto/loams/repos/v1/{repos.proto,workspaces.proto,objects.proto,env.proto}   Tasks 12, 18, 25, 61 (public API)
crates/loams-store/src/{store.rs,fault.rs}                    Task 1
crates/loams-fs/                                              Task 2
crates/loams-cloudevents/{build.rs,src/protobuf.rs}           Task 3
crates/loams-git/                                             Tasks 4–11, 13–15, 17–27, 46–50, 53–55
  src/{lib.rs,limits.rs,ids.rs,paths.rs,error.rs,catalog.rs,
       format/{segment.rs,checkpoint.rs,lpk.rs,event.rs,snapshot.rs},
       blob.rs,wal.rs,state.rs,odb/{mod.rs,pack_cache.rs,range.rs,ws.rs},reflog.rs,sequencer.rs,
       checkpointer.rs,fork.rs,gc.rs,pack_gc.rs,scrub.rs,owner.rs,mirror_events.rs,compact.rs,
       ws/{mod.rs,store.rs,tree_build.rs,merge3.rs,push.rs,edits.rs,retention.rs},
       derived/{mod.rs,tree_aux.rs,graph.rs,blame.rs},
       history/{log.rs,grep.rs,diffstat.rs},
       protocol/{pktline.rs,advertise.rs,lsrefs.rs,objectinfo.rs,fetch.rs,negotiate.rs,assemble.rs,
                 filter.rs,shallow.rs,receive.rs,verify.rs,report.rs,v0.rs},
       metrics.rs,testing.rs}
  tests/…  tests/golden/{segment_v1.lgw,checkpoint_v1.lgc,lpk_v1_footer.bin,snapshot_v1.lws}
  fuzz/fuzz_targets/{pktline.rs,receive_pack.rs,segment.rs,checkpoint.rs,snapshot.rs}
crates/loams-vfs/                                             Tasks 28–35, 45, 56
  src/{lib.rs,limits.rs,config.rs,backend/{mod.rs,api.rs,direct.rs},
       store/{meta.rs,cache.rs,overlay.rs,journal.rs},fetch.rs,inode.rs,fuse.rs,write.rs,ignore.rs,
       redirect.rs,save.rs,mount.rs,takeover.rs,salvage.rs,prefetch.rs,control.rs,projection.rs,
       absorb.rs,fsmonitor.rs,env_render.rs,bin/loams-vfsd.rs}
  tests/{store.rs,fetch.rs,fuse_read.rs,fuse_write.rs,save.rs,mount.rs,prefetch.rs,posix.rs,projection.rs}
crates/loams-git-porcelain/                                   Tasks 37–45
  src/{lib.rs,args.rs,allow.rs,view.rs,revparse.rs,format/{pretty.rs,status.rs,diff.rs,log.rs},
       myers.rs,cmd/{status.rs,diff.rs,show.rs,add.rs,rm.rs,mv.rs,restore.rs,reset.rs,commit.rs,hooks.rs,
       branch.rs,switch.rs,tag.rs,log.rs,blame.rs,grep.rs,fetch.rs,pull.rs,push.rs,worktree.rs,plumbing.rs}}
  tests/{differential.rs,corpus/agent-commands.txt,…}
crates/loams-git-shim/                                        Task 36 (bin loams-git)
crates/loams-git-remote/                                      Task 51
crates/loams-env/                                             Tasks 61–67
  src/{lib.rs,limits.rs,format.rs,fingerprint.rs,seal.rs,render.rs,diff.rs,merge.rs,policy.rs,
       scan/{mod.rs,rules.rs,known.rs,allowlist.rs},filter.rs}  rules/gitleaks.toml
crates/loams-vault/                                           Tasks 59–60, 65–66
  src/{lib.rs,keys.rs,version.rs,store.rs,env_rev.rs,backend/{mod.rs,loams.rs,openbao.rs,aws.rs,azure.rs,gcp.rs},
       conformance.rs,promote.rs,rotate.rs,providers/{random.rs,loams_postgres.rs,loams_sql.rs,external.rs,webhook.rs}}
crates/loams-buildcache/, crates/loams-registry/              Tasks 68–73
crates/loams-hot/src/placement.rs                             Task 14
crates/loams-agentd-sessions/src/…                            Task 57 (worktrees → workspaces, through its RPC)
crates/loams/src/api/{repos.rs,workspaces.rs,objects.rs,env.rs,git_http.rs,git_internal.rs,buildcache.rs,registry.rs}
crates/loams/src/{main.rs,server.rs}                          --git-listen, dev --git, vfs/env subcommands (Task 81)
bench/{git-wal,git-ws,git-vfs}/                               Tasks 13, 58, 80
scripts/git/{rustfs.sh,openbao.sh,bench.sh,matrix.sh,record-agent-corpus.sh,mono-fixture.sh,restore-drill.sh,chaos.sh}
scripts/env/gen-rules.sh  scripts/ci/{git-licence.sh,no-metering.sh}
deploy/loams-git-dev/compose.yaml  deploy/observability/loams-git/{dashboards/,alerts.yaml}
docs/security/loams-git-threat-model.md  docs/runbooks/loams-git/  docs/guides/{git.md,workspaces.md,git-shim.md,env.md,build-cache.md,crates-mirror.md}
.github/workflows/{gt1.yml,gt1-fuse.yml,gt1-e2e.yml,gt1-nightly.yml}
```

## Shared contracts (all tasks use these names)

### Object layout (§36 §4.1, §19.4.1, §19.13; Rulings 21–22)

```
ns/<ns_id>/repos/_names/<name>.json                   catalog record
ns/<ns_id>/repos/_forks/<parent_repo_id>/<child_repo_id>   fork index (Task 54)
ns/<ns_id>/repos/<repo_id>/
  head  wal/<seq:020>.lgw  checkpoints/<seq:020>.lgc  packs/<checksum>.lpk  midx/<seq>.midx  commit-graph/<seq>.graph  gc/claims/<ulid>.json
  ws/<ws_id>/{snap/<n:020>.lws, head, packs/<checksum>.lpk, salvage/<mount_id>/<ulid>.lws}
  _ws_names/<owner>/<name>.json
  derived/{tree-aux/<tree_oid>.lta, graph/<seq:020>.graph, blame/<commit>/<sha256(path)>.lbl}
ns/<ns_id>/vault/secrets/<secret_id>/v/<version:010>.lsv  (and meta.json: repo, path, key)
ns/<ns_id>/vault/repos/<repo_id>/fp-key.lsv              repo_fp_key, KMS-wrapped
ns/<ns_id>/vault/env/<repo_id>/<environment>/rev/<n:020>.ler   (+ head hint)
ns/<ns_id>/cache/sccache/…   ns/_public/packages/crates/…
```

### Traits (`loams-git`)

`WalStore`, `RefLog`, `BlobStore` and their types are exactly §36 §5.1–§5.3. `ObjectDb`, `Clock`, `GitAuthorizer`, `RepoCatalog` and `RepoRecord` are as in the morning version, with these additions:

```rust
pub struct GitGrant {
    pub org: String, pub namespace: NamespaceId, pub principal: Principal,
    pub repos: RepoMatch, pub access: GitAccess,             // Read | Write | Admin
    pub branch_prefixes: Vec<String>, pub cones: Vec<RepoPath>,
    pub workspaces: WsMatch,                                 // Own | All | Ids(Vec<WsId>)
    pub env: Vec<EnvGrant>,                                  // §19.13.7; empty for agents unless granted
    pub cache_class: Option<TrustClass>, pub registry_read: bool,
}
pub struct EnvGrant { pub environment: String, pub access: EnvAccess }   // Read | Write | Promote

pub struct WsId(String);          // "w" + 26 lowercase ULID chars
pub struct SnapN(pub u64);
pub struct MountId(String);       // "m" + ULID, per vfsd mount, persisted in meta.sqlite
pub enum SnapAppended { Committed { n: SnapN }, Fenced { existing: Arc<pb::WorkspaceSnapshot> } }

#[async_trait]
pub trait WorkspaceStore: Send + Sync + fmt::Debug {
    /// Writes the name record (if named, put_if_absent) and snapshot 0. Idempotent on `spec.ws_id`.
    async fn create(&self, spec: CreateWs) -> Result<Arc<pb::WorkspaceSnapshot>, WsError>;
    /// Hint, then probe forward until 404. Linearizable at return.
    async fn latest(&self, repo: &RepoId, ws: &WsId) -> Result<Arc<pb::WorkspaceSnapshot>, WsError>;
    async fn get(&self, repo: &RepoId, ws: &WsId, n: SnapN) -> Result<Option<Arc<pb::WorkspaceSnapshot>>, WsError>;
    /// Create-only PUT of `snap.n`; a 412 reads back: same writer and same bytes = Committed.
    async fn append(&self, snap: &pb::WorkspaceSnapshot) -> Result<SnapAppended, WsError>;
    async fn salvage(&self, snap: &pb::WorkspaceSnapshot, mount: &MountId) -> Result<Ulid, WsError>;
    async fn list(&self, repo: &RepoId, owner: Option<&str>, after: Option<&str>, limit: usize)
        -> Result<Vec<WsSummary>, WsError>;
    async fn fork(&self, from: (&RepoId, &WsId, SnapN), spec: CreateWs) -> Result<Arc<pb::WorkspaceSnapshot>, WsError>;
}
// Implementations: BucketWorkspaceStore (StoreWalStore's patterns), MemWorkspaceStore (tests).

/// Client-facing object and history access; ApiBackend and DirectBackend both implement it.
#[async_trait]
pub trait ObjectSource: Send + Sync + fmt::Debug {
    fn get_objects(&self, ws: &WsCtx, oids: Vec<ObjectId>) -> BoxStream<'static, Result<RawObject, FetchError>>;
    async fn get_trees(&self, ws: &WsCtx, root: ObjectId, depth: u8) -> Result<Vec<TreeWithAux>, FetchError>;
    async fn has_objects(&self, ws: &WsCtx, oids: &[ObjectId]) -> Result<Vec<bool>, FetchError>;
    async fn put_workspace_pack(&self, ws: &WsCtx, lpk: Bytes) -> Result<pb::PackRef, FetchError>;
}
pub struct TreeWithAux { pub oid: ObjectId, pub entries: Vec<AuxEntry> }   // name, mode, oid, size
```

### `loams-vfs` and the shim

```rust
/// vfsd's control protocol over $XDG_RUNTIME_DIR/loams/vfsd.sock: u32 BE length + postcard frames,
/// `ControlRequest { version: 1, id, body }` → `ControlResponse { id, body }`.
pub enum ControlBody {
    Resolve { path: PathBuf },                                 // → Resolved { ws, root, rel, writer: bool }
    Flush { ws: WsId },                                        // force a save; → Saved { n }
    State { ws: WsId },                                        // → WsState (the snapshot + local view)
    Mutate { ws: WsId, expect_n: SnapN, ops: Vec<WsOp> },      // → Saved { n } | Conflict
    Changed { ws: WsId, since: Option<JournalToken> },         // fsmonitor v2 answer
    Project { ws: WsId }, Absorb { ws: WsId },
    Mount { repo: RepoRef, ws: WsSel, at: Option<PathBuf>, take: bool, read_only: bool },
    Unmount { ws: WsId }, Status,
}
pub enum WsOp { SetIndex { tree: ObjectId, conflicts: Vec<pb::Conflict> }, SetHead { oid: ObjectId, head_ref: Option<String> },
                SetRefs(Vec<pb::LocalRef>), AddObjects { lpk: Bytes }, SwitchBase { tree: ObjectId, base: ObjectId, base_seq: Seq },
                WriteFiles(Vec<(RepoPath, FileWrite)>), Label(String) }
```

The mounts file `$XDG_RUNTIME_DIR/loams/mounts` holds one line per root or workspace: `<abs path>\t<ws_id|root>\t<repo>`. It is rewritten atomically by vfsd and read by the shim.

### Public API (`loams.repos.v1`)

`RepoService` is as in the morning version. The new services follow §36 §19.10 and §19.13:

```proto
service WorkspaceService {
  rpc CreateWorkspace(CreateWorkspaceRequest) returns (CreateWorkspaceResponse);     // repo, track, at?, name?
  rpc GetWorkspace(GetWorkspaceRequest) returns (GetWorkspaceResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ListWorkspaces(ListWorkspacesRequest) returns (ListWorkspacesResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ForkWorkspace(ForkWorkspaceRequest) returns (ForkWorkspaceResponse);
  rpc DeleteWorkspace(DeleteWorkspaceRequest) returns (DeleteWorkspaceResponse);
  rpc ListSnapshots(ListSnapshotsRequest) returns (ListSnapshotsResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc GetSnapshot(GetSnapshotRequest) returns (GetSnapshotResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc SaveSnapshot(SaveSnapshotRequest) returns (SaveSnapshotResponse);              // fenced on n
  rpc ReadFiles(ReadFilesRequest) returns (stream ReadFilesResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ApplyEdits(ApplyEditsRequest) returns (ApplyEditsResponse);                    // expect_n; seals env; scans
  rpc Diff(DiffRequest) returns (stream DiffResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc Rebase(RebaseRequest) returns (RebaseResponse);
  rpc PushWorkspace(PushWorkspaceRequest) returns (PushWorkspaceResponse);
  rpc WatchWorkspace(WatchWorkspaceRequest) returns (stream WatchWorkspaceResponse);
  rpc ListSalvage(ListSalvageRequest) returns (ListSalvageResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc RestoreSalvage(RestoreSalvageRequest) returns (RestoreSalvageResponse);
}
service ObjectService {
  rpc GetObjects(GetObjectsRequest) returns (stream GetObjectsResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc GetTrees(GetTreesRequest) returns (GetTreesResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc HasObjects(HasObjectsRequest) returns (HasObjectsResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc PutWorkspacePack(stream PutWorkspacePackRequest) returns (PutWorkspacePackResponse);
  rpc Log(LogRequest) returns (stream LogResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc Blame(BlameRequest) returns (BlameResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc Grep(GrepRequest) returns (stream GrepResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc DiffStat(DiffStatRequest) returns (DiffStatResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
}
service EnvService {
  rpc Seal(SealRequest) returns (SealResponse);                  // idempotent on (secret, fingerprint)
  rpc Open(OpenRequest) returns (OpenResponse);                  // audited; redacts what the grant cannot read
  rpc ListEnvironments(ListEnvironmentsRequest) returns (ListEnvironmentsResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc GetLiveSet(GetLiveSetRequest) returns (GetLiveSetResponse);   // values only with Read; audited
  rpc WatchEnvironment(WatchEnvironmentRequest) returns (stream WatchEnvironmentResponse);   // revisions, no values
  rpc Rotate(RotateRequest) returns (RotateResponse);            // -> Operation
  rpc Destroy(DestroyRequest) returns (DestroyResponse);         // a version
  rpc Allow(AllowRequest) returns (AllowResponse);               // scanner allowlist entry
}
```

The new reasons are those listed in §36 §19.10.

### Limits

The morning version's `loams_git::limits` table stands (`MAX_GROUP_TXNS` 64, `MAX_SEGMENT_BYTES` 1 MiB, `IDEMPOTENCY_WINDOW` 1 h, `CHECKPOINT_EVERY_*` 256 / 8 MiB, `MAX_REFS_PER_*` 4096, `EXACTLY_RETENTION` 24 h, `GC_GRACE` 1 h, `MAX_PACK_BYTES` 2 GiB, `MULTIPART_THRESHOLD` 64 MiB, `MAX_FORK_DEPTH` 8, `MAX_FENCE_RETRIES` 16, the negotiation limits, `RECEIVE_IDLE_TIMEOUT` 60 s, `UPLOAD_DEADLINE` 30 min, `SEQ_IDLE_EVICT` 10 min, `FORWARD_TIMEOUT` 2 s, `HINT_EVERY` 1 s). Additions:

| Constant | Value | § |
|---|---|---|
| `MAX_SNAPSHOT_BYTES` | 1 MiB | §19.4.2 |
| `MAX_CHANGED_HINT` | 10,000 paths | §19.4.2 |
| `WRITER_IDLE` | 10 min | §19.4.3 |
| `WS_PACKS_COMPACT_AT` | 64 | §19.4.5 |
| `SNAPSHOT_KEEP_ALL` / `SNAPSHOT_KEEP_HOURLY` | 7 d / 30 d | §19.4.5 |
| `SALVAGE_RETENTION` | 30 d | §19.4.5 |
| `FETCH_BATCH_OIDS` | 1,024 | §19.5.1 |
| `TREE_PREFETCH_DEPTH` | 2 | §19.5.1 |
| `MIN_COALESCED_RANGE` | 64 KiB | §19.5.1 |
| `SAVE_DEBOUNCE` / `SAVE_MIN_INTERVAL` | 2 s / 1 s | §19.5.3 |
| `FETCH_TIMEOUT` | 30 s | §19.6.3 |
| `CACHE_DEFAULT_BYTES` | 20 GiB | §19.6.2 |
| `MAX_WORKSPACES_PER_PRINCIPAL` | 100,000 (quota default) | Task 75 |
| `SHIM_PASSTHROUGH_BUDGET` / `SHIM_NATIVE_STARTUP_BUDGET` | 2 ms / 20 ms | §19.7.6 |
| `ENV_RENDER_TTL` | 5 min | §19.13.7 |
| `MAX_ENV_FILE_BYTES` / `MAX_ENV_KEYS` | 256 KiB / 2,048 | §19.13.2 |
| `SCAN_MAX_FILE_BYTES` | 8 MiB (larger files are scanned in their first and last 8 MiB, and binary files are skipped) | §19.13.9 |

---

## Execution order

1. Task 0 (no owner gate: the schedule was resolved on 2026-10-10).
2. **GT1a** (Tasks 1–11). Tasks 1, 2 and 3 are independent and come first.
3. **GT1b** (12–16), then **GT1c** (17–21), both after GT1a. Task 13's gate (now inside Task 11's milestone close) records Q384's numbers.
4. **GT1d** (22–27) after GT1c Task 18.
5. **GT1e** (28–35) after GT1d Task 24. Task 28 can start beside GT1d.
6. **GT1f** (36–45) after GT1e Task 33. Task 36 (dispatch and passthrough) can start as soon as the mounts file format (Task 33's contract) is fixed.
7. **GT1g** (46–52) after GT1b. It is independent of GT1d–GT1f, so it can run in the build machine's spare slots. Task 45's projection needs Task 51's `stateless-connect`.
8. **GT1h** (53–55) after GT1g Task 50 and GT1d.
9. **GT1i** (56–58) after GT1f.
10. **GT1j** (59–67): Tasks 59–61 after GT1a; Task 62 after Task 32; Task 63 after Tasks 27 and 50; Task 65 after Task 15.
11. **GT1k** (68–73) needs only Tasks 1 and 4. It runs in spare slots.
12. **GT1l** (74–82) last. Task 78's fuzz targets start as soon as their parsers exist.
13. **After GA:** Tasks 83–87.

---

### Task 0: Reconcile with the code as built

**Files:** this plan's "Rulings made during execution"; `docs/plans/README.md` (mark the 2026-10-01 GT1–GT3 rows "Superseded by [GT1 production](2026-10-10-gt1-loams-git.md)" and add this plan's rows).

Steps:
1. Answer each item below and record the answer, with file paths and commands, as a ruling:
   - Every reconciliation item of the morning version: `dev` still free of the new crates; `loams_store::Store` gaps; where `CloudEventBatch` is generated; `loams-proto/build.rs` and the `CATALOGUE`; `NamespaceId` and `namespace_by_name`; `loams-hot::placement`; `loams-worker` registration; `RangeCache`'s API; the metrics exporter; MT1's status; the gix train and cold build time; `git --version` locally and in CI; Q386 (lazy fetch through `stateless-connect`); Q387 (libgit2 and JGit on v2); Q394 (sccache's WebDAV subset); cargo's `auth-required` minimum; RustFS in CI; any Kubernetes chart.
   - **FUSE:** the `fuser` version and licence, mounting without libfuse (`fusermount3`), FUSE passthrough support in `fuser` and on the CI kernel (≥ 6.9), `/dev/fuse` on GitHub-hosted runners (else a self-hosted runner for `gt1-fuse.yml`), and unprivileged FUSE in a user namespace (for Task 56).
   - **git features the projection needs:** `index.sparse` and `core.fsmonitor` (a hook speaking fsmonitor protocol v2), promisor remotes through a helper's `stateless-connect`, `filter.<driver>.process`. Record the minimum git version and the CI version.
   - **`gix-blame`:** its maturity against `git blame` on 50 seeded files (byte-identical `--porcelain`?). If it is not adequate, Task 21 builds blame over `Log` and the porcelain's Myers diff.
   - **`gix-index`:** can it read and write a sparse index (`sdir` extension)? If not, Task 45 writes a full index limited to the changed directories plus `skip-worktree` entries, and records the cost.
   - **`loams-durable`:** its workflow API for Tasks 65–66 (registration, idempotency keys, timers).
   - **`loams-agentd-sessions`:** where `CreateWorktree` and `DeleteWorktree` live (DD1 T0-1 keeps them) and how a repository kind could be switched to Loams workspaces (Task 57).
   - **pg-control:** `ResetRolePassword` as built (§46) and whether a second role can be created for A/B rotation (Task 66). **Loams SQL:** `RotateRolePassword` (SQ1).
   - **D96's `KeyProvider`:** absent at `814d6dac` (no `KeyProvider` in `crates/`). If still absent, Task 59 builds the trait with `FileKeyProvider` and `AwsKmsKeyProvider`, `GcpKmsKeyProvider` and `AzureKeyVaultKeyProvider` over REST with `reqwest`, in `loams-vault` behind features, ready to move to `loams-common` when M2 needs it.
   - **gitleaks:** the commit of its default `gitleaks.toml` to pin (Ruling 48), and the rule count after Rust `regex` compatibility filtering (rules using look-around are dropped and listed).
   - **The agent corpus:** record the git commands that Claude Code, Codex and opencode issue in their current versions (from transcripts in `~/.claude/projects`, `~/.codex/sessions`, and the harness fixtures in `crates/loams-agentd-harness`) with `scripts/git/record-agent-corpus.sh`. The result, `crates/loams-git-porcelain/tests/corpus/agent-commands.txt`, is the native allowlist's input (Q751).
2. Update `docs/plans/README.md` as above.
3. Commit `docs(git): GT1 task 0 rulings; supersede the 2026-10-01 GT plans`.

## GT1a — Foundations and the WAL core (Tasks 1–11)

These tasks are unchanged from the morning version of this plan. Their files, interfaces, tests and commit messages are given in full there and repeated here in short form. They are the old GT1 Tasks 1–8, amended by Rulings 20–29.

### Task 1: `loams-store` additions
`Store::put_multipart`, `Store::copy`, `Store::list_page`; `FaultyStore` classifies them. Tests: `multipart_round_trip_65_mib`; `multipart_abort_leaves_no_object`; `copy_to_self_updates_last_modified`; `list_page_is_sorted_and_resumable`; `faulty_store_injects_on_new_ops`. Commit `store: add multipart puts, copies and paged listings`.

### Task 2: `loams-fs`
§36 §17.2 verbatim, `NativeFs`, `MemFs`, `conformance::run`, `SpillDir`. Tests: `read_after_write`; `create_new_race_one_winner`; `range_read_boundaries`; `list_pagination`; `rename_is_atomic`; `path_rejects_dot_segments_and_over_1024_bytes`; `spill_dir_removed_on_drop`. Commit `fs: add the Fs trait with native and in-memory backends`.

### Task 3: The CloudEvents protobuf codec in `loams-cloudevents`
Feature `protobuf`; `loams-stream-grpc` re-exports it (Ruling 23). Tests: `proto_round_trip_preserves_attribute_strings`; `batch_round_trip`; `loams-stream-grpc`'s suite unchanged. Commit `events: move the CloudEvents protobuf codec into loams-cloudevents`.

### Task 4: The `loams-git` crate, ids, paths and limits
Old GT1 Task 1's `ids` and `paths`, plus `RepoId` (Ruling 21), `RepoName`, **`WsId`, `SnapN`, `MountId`, `SecretId`**, `RepoPaths::{names_record, gc_claim, ws_snapshot(ws, n), ws_head(ws), ws_pack(ws, checksum), ws_salvage(ws, mount, ulid), ws_name(owner, name), derived_tree_aux(oid), derived_graph(seq), derived_blame(commit, path)}`, and both limits tables. Tests: `refname_validation_matches_git_check_ref_format`; `repo_id_charset`; `ws_id_charset`; `repo_name_rejects_dot_git_suffix`; `seq_formats_as_twenty_digits_and_sorts_lexically`; `paths_round_trip`; `limits_are_documented`. Commit `git: add the loams-git crate with ids, paths and limits`.

### Task 5: Formats
`proto/loams/git/v1/wal.proto` (§36 §4.3 verbatim); segment, checkpoint, `.lpk` and event codecs (old GT1 Task 2, D415). Tests: `segment_round_trip`; `segment_over_one_mib_is_too_large`; `corrupt_crc_is_error`; `truncated_segment_is_error`; `seq_mismatch_is_error`; `golden_segment_v1`; `checkpoint_round_trip`; `golden_checkpoint_v1`; `lpk_footer_round_trip`; `pack_checksum_reads_trailer`; `event_attributes_match_design_table`; `event_without_tenantid_is_refused`; `dataschema_must_match_type`; `n_minus_one_is_read`. Commit `git: add the loams.git.v1 formats`.

### Task 6: `BlobStore`
§36 §5.3 and Ruling 1. Tests: `put_is_create_only_and_idempotent`; `put_existing_with_other_length_is_mismatch`; `lost_ack_put_returns_existed_on_retry`; `get_range_reads_idx_section`; `range_past_end_is_out_of_range`; `lpk_from_git_pack_round_trips`; `large_blob_uses_multipart`; `rustfs_large_blob_refused_until_q385`. Commit `git: add BlobStore with one-object pack bundles`.

### Task 7: `WalStore`
§36 §5.1. Tests: `append_is_exclusive_across_writers`; `retry_after_lost_ack_is_committed`; `retry_after_lost_ack_fenced_by_other`; `r2_style_429_resolves_by_read_back`; `read_stops_at_gap`; `corrupt_segment_is_error_not_stop`; `hint_missing_is_empty`; `latest_checkpoint_uses_hint_then_list`; `checkpoint_put_is_idempotent_on_equal_bytes`; `store_faults_never_produce_two_owners`. Commit `git: add the bucket WAL with fenced create-only segments`.

### Task 8: The repository state machine and idempotency
Old GT1 Task 6. Tests: `model_matches_reference`; `group_sees_earlier_txns`; `replay_inside_window_returns_receipt`; `key_reuse_with_other_digest_is_mismatch`; `after_window_old_oid_decides`; `protected_ref_refuses_delete_and_force`; `duplicate_ref_in_one_txn_is_invalid`; `checkpoint_round_trip_keeps_window_entries_only`; `apply_segment_refuses_a_gap`. Commit `git: add the repository state machine with idempotency windows`.

### Task 9: `ObjectDb` and the pack-cache `Odb`
As the morning version. Tests: `reads_objects_written_by_git`; `delta_objects_resolve`; `fork_falls_through_to_parent_at_seq`; `fork_depth_is_bounded`; `missing_from_closure_finds_a_missing_blob`; `cache_eviction_never_breaks_reads`. Commit `git: add the object database trait and a pack-cache implementation`.

### Task 10: `BucketRefLog`: the sequencer and group commit
As the morning version, including `GitEvent`s. Tests (`reflog.rs`): `commit_then_snapshot_sees_it`; `group_commit_batches_concurrent_txns`; `group_rejects_only_the_stale_txn`; `atomic_multi_ref_all_or_none`; `lost_ack_resolves_to_committed`; `fenced_group_revalidates`; `replay_returns_original_receipt`; `snapshot_is_a_prefix_of_commits`; `watch_has_no_gaps_across_fences`; `missing_pack_is_refused_before_queueing`; `shutdown_answers_unavailable`; `fence_retry_limit_is_unavailable`. Tests (`linearizable.rs`): `single_sequencer_linearizable`; `two_sequencers_one_store_linearizable`; `four_sequencers_under_faults_linearizable`. Commit `git: add the bucket RefLog with group commit, fencing and idempotency`.

### Task 11: Checkpoints, forks, segment GC, and the core gate
As the morning version's Task 11, plus the morning version's Task 13 bench and CI (`bench/git-wal/`, `scripts/git/{rustfs.sh,bench.sh}`, `gt1.yml`, `gt1-nightly.yml`). Tests: `checkpoint_plus_replay_equals_state`; `checkpoint_never_delays_ack`; `fork_is_one_put`; `fork_sees_parent_refs_and_objects`; `fork_of_fork_falls_through`; `gc_keeps_exactly_retention`; `gc_deletes_unreachable_above_gap_after_grace`; `gc_never_deletes_segments_a_reader_needs`. **Gate:** at `--put-delay-ms 100`, at least 30 commits/s on one hot repository with p99 under 1 s; RustFS and in-memory numbers recorded (GT-Q8 for the cloud stores); Q393 decided from the numbers. Commits `git: add checkpoints, O(1) forks and segment GC`; `bench: measure the bucket WAL per store`; `ci: run the loams-git suites against RustFS`.

## GT1b — Repositories as a service (Tasks 12–16)

These tasks are the morning version's Tasks 14–18, with these changes.

### Task 12: `loams.repos.v1` protos, reasons, route map and the catalogue
**Files:** `proto/loams/repos/v1/repos.proto` (`RepoService`), with **empty stubs** of `workspaces.proto`, `objects.proto` and `env.proto` that later tasks fill in (the package is registered once); `crates/loams-proto/build.rs`; `crates/loams/src/api/connect.rs`; `crates/loams/Cargo.toml` (features `git`, `buildcache`, `registry`); `docs/api/{reasons.md,route-map.md}` (all §19.10 reasons registered now). Tests: `buf lint`; `every_mutation_has_idempotency_key`; `reads_take_consistency_token`; `reasons_registered`; `catalogue_lists_repos_only_with_feature`; `default_features_exclude_git`. Commit `proto: add loams.repos.v1`.

### Task 13: The repository catalog and `RepoService`
As the morning version's Task 15. Tests: `create_repo_replay_returns_same_repo`; `create_same_name_other_key_is_already_exists`; `undetermined_create_then_retry_returns_same_repo`; `recreate_after_purge_gets_new_repo_id`; `fork_repo_is_one_checkpoint_put_plus_name`; `delete_returns_operation_and_hides_repo`; `delete_with_live_fork_waits`; `update_config_sets_head_and_protections`; `list_refs_honours_consistency_token`; `watch_repo_snapshot_then_changes_then_heartbeat`; `list_repos_paginates`. Commit `git: add the repository catalog and RepoService`.

### Task 14: Placement, sequencer ownership and forwarding
As the morning version's Task 16. Tests: `rendezvous_repo_scores_are_stable`; `owner_runs_the_only_leased_sequencer`; `forwarded_push_commits_on_owner`; `owner_down_fallback_is_fenced`; `zombie_owner_is_fenced`; `helper_and_server_race_one_repo`; `idle_sequencer_is_evicted_and_reloads`. Commit `git: place repository sequencers by rendezvous and forward pushes`.

### Task 15: The `_git` event-stream mirror and `GitEvent` consumers
As the morning version's Task 17. In addition, `GitEvents::subscribe()` gives in-process consumers (Task 65's `EnvPromoter`) the committed transactions with their seq, at least once, resumable by seq from the WAL. Tests: `each_commit_appears_once_in_git_stream`; `mirror_failure_never_blocks_ack`; `repair_fills_a_gap_after_crash`; `replayed_events_are_deduplicated`; `subscriber_resumes_from_seq_after_restart`. Commit `git: mirror committed transactions into the _git stream`.

### Task 16: Authorization, token scopes and the listener rule
As the morning version's Task 18, with `GitGrant` as in the shared contracts (`workspaces`, `env`). `LoopbackDevAuthorizer`'s TOML gains `workspaces` and `env`, and agent tokens default to `env = []`. Tests: `namespace_from_token_not_url`; `other_namespace_is_404`; `read_token_cannot_push`; `branch_prefix_scope_refuses_push`; `admin_needed_for_delete_and_config`; `non_loopback_listen_is_refused`; `token_never_logged`; `agent_grant_has_no_env_by_default`. Commit `git: authorize repository access through a GitAuthorizer`.

## GT1c — Object service and derived data (Tasks 17–21)

### Task 17: `RangeOdb`, the range-read object database
**Files:** `src/odb/{range.rs,ws.rs}`, `tests/odb.rs`. **Interfaces:** old GT2 Task 3's `RangeOdb` over `BlobStore` and `RangeCache`, implementing `ObjectDb`, with `locate`, `header` (type and **result size** from the entry header, reading a delta's result-size varint without inflating it), and `raw_entry`. **`WsOdb::open(repo_odb, ws_packs)`** layers a workspace's packs over the repository's. Tests: `locate_matches_git_cat_file`; `read_resolves_ofs_and_ref_deltas`; `header_size_matches_cat_file_s_without_inflate` (counts inflate calls: 0); `midx_lookup_matches_per_pack`; `range_reads_are_coalesced`; `fork_parent_lookup`; `ws_packs_shadow_nothing_and_add_objects`; `cache_dropped_mid_read_still_correct`. Commit `git: read objects by range from stored packs`.

### Task 18: `ObjectService` reads and the immutable object route
**Files:** `proto/loams/repos/v1/objects.proto`, `crates/loams/src/api/objects.rs`, `crates/loams/src/api/git_http.rs` (`GET /git/<ns>/<repo>.git/loams/objects/<oid>`), `crates/loams-git/tests/objects.rs`. **Interfaces:** `GetObjects` (at most `FETCH_BATCH_OIDS`, streamed, grouped by pack, ranges coalesced to at least `MIN_COALESCED_RANGE`), `GetTrees` (to `depth`, with aux data from Task 19), `HasObjects`. Access is by `GitGrant` on the repository plus the workspace (`WsMatch`); objects in a workspace pack are readable only through a workspace the caller may read. Tests: `get_objects_round_trip_and_order`; `batch_over_limit_is_invalid_argument`; `get_trees_depth_two_has_sizes`; `has_objects_bitmap`; `object_route_is_immutable_cacheable`; `ws_object_needs_ws_access`; `get_objects_bounded_memory` (100k objects streamed, RSS under 128 MiB). Commit `api: serve lazy object and tree reads`.

### Task 19: Derived data: tree aux
**Files:** `src/derived/{mod.rs,tree_aux.rs}`, `tests/derived.rs`. **Interfaces:** `DerivedStore` (create-only puts under `derived/`, content-keyed, readable through H1); `tree_aux(odb, tree_oid) -> Arc<TreeAux>` (entries: name, mode, oid, size), computed from `header` and cached as `.lta` (magic `LGITTAUX`, version 1, CRC32C). Tests: `tree_aux_matches_ls_tree_long`; `tree_aux_is_cached_once` (two concurrent computers, one PUT wins, both answers equal); `golden_tree_aux_v1`; `derived_deleted_is_recomputed`. Commit `git: derive tree aux data with entry sizes`.

### Task 20: Commit graph, `Log` and `DiffStat`
**Files:** `src/derived/graph.rs`, `src/history/{log.rs,diffstat.rs}`, `tests/history.rs`. **Interfaces:** `GraphView` (compaction's `commit-graph` with changed-path Bloom filters plus `derived/graph/<seq>.graph` layers built on demand for later commits, in gix-commitgraph's read format, so the same reader serves both); `Log(rev, ranges, pathspec, limit, fields)` streaming `LogEntry { oid, parents, author, committer, message, changed: Option<Vec<ChangedPath>> }`; `DiffStat(a, b, pathspec)` with per-file added and removed counts. Tests: `log_matches_git_log_format_raw` (200 seeded histories); `log_pathspec_uses_bloom_then_diff` (counts tree reads); `log_ranges_and_merges_match_git`; `diffstat_matches_git_numstat`; `graph_layer_built_once_per_seq`. Commit `git: serve log and diff stats from the commit graph`.

### Task 21: `Blame` and `Grep`
**Files:** `src/derived/blame.rs`, `src/history/grep.rs`, `tests/history.rs`. **Interfaces:** `Blame(rev, path, line_range)` over `gix-blame`, or Task 0's fallback, cached in `derived/blame/`; `Grep(rev, pattern, pathspec, flags)` with `grep-regex` and `grep-searcher`, parallel over blobs, `GREP_DEADLINE` 30 s, `GREP_MAX_MATCHES` 10,000, results streamed in path order. Tests: `blame_porcelain_matches_git` (50 files); `blame_is_cached`; `grep_matches_git_grep` (flags `-n -i -l -w -F -E`); `grep_deadline_returns_partial_with_flag`; `grep_skips_binary_like_git`. Commit `git: serve blame and grep on the server`.

## GT1d — Server-side workspaces (Tasks 22–27)

### Task 22: The workspace snapshot format
**Files:** `proto/loams/git/v1/workspace.proto` (§36 §19.4.2 verbatim), `src/format/snapshot.rs`, `tests/format.rs`, `tests/golden/snapshot_v1.lws`. **Interfaces:** `SNAPSHOT_MAGIC` (`LGITWSNP`), `encode_snapshot`, `decode_snapshot` (N and N−1 versions), the 1 MiB cap, and `snapshot_commit(snap) -> (ObjectId, Bytes)` (Ruling 38). Tests: `snapshot_round_trip`; `snapshot_over_one_mib_is_too_large`; `golden_snapshot_v1`; `corrupt_snapshot_crc_is_error`; `snapshot_commit_is_deterministic` (two hosts, same state, same oid; matches `git hash-object -t commit`); `changed_hint_overflow_sets_flag`. Commit `git: add the workspace snapshot format`.

### Task 23: `WorkspaceStore`
**Files:** `src/ws/{mod.rs,store.rs}`, `tests/ws_store.rs`. **Interfaces:** the trait in the shared contracts; `BucketWorkspaceStore`, `MemWorkspaceStore`. `create` writes the name record (`put_if_absent`; `WorkspaceNameTaken` if another `ws_id` holds it), then snapshot 0, retrying an undetermined outcome by read-back. `append` is create-only, with read-back on 412 and on unknown outcomes. `latest` uses the hint and a probe. `fork` copies snapshot `n` as snapshot 0 with `fork` set. Tests: `create_is_one_put_unnamed_two_named`; `create_replay_returns_same_ws`; `name_taken_is_already_exists`; `snapshot_append_is_exclusive` (32 racers, one winner); `lost_ack_save_is_committed`; `fenced_append_returns_existing`; `latest_uses_hint_then_probe`; `latest_without_hint_lists`; `fork_is_one_put_and_shares_packs`; `million_workspaces_listing_pages` (in-memory store, 1,000,000 creates, paged list, nightly). Commit `git: store workspaces as create-only snapshot chains`.

### Task 24: Workspace packs and tree building
**Files:** `src/ws/tree_build.rs`, `crates/loams/src/api/objects.rs` (`PutWorkspacePack`), `tests/ws_tree.rs`. **Interfaces:** `TreeBuilder::apply(base_tree, edits: impl Iterator<(RepoPath, Edit)>, odb) -> (new_root, new_objects)`, which writes only the trees along changed paths, sorted by git's tree order; `PutWorkspacePack` verifies every object's hash and the §6.3 limits, runs Task 67's scanner hook (a no-op until then), and stores `ws/<ws_id>/packs/<checksum>.lpk`. Tests: `tree_builder_matches_git_write_tree` (proptest over edit sets, compared with `git update-index` plus `write-tree`); `tree_builder_writes_k_times_depth_trees`; `mode_and_symlink_and_exec_bit_round_trip`; `put_ws_pack_rejects_bad_hash`; `put_ws_pack_is_create_only`; `put_ws_pack_needs_ws_write`. Commit `git: build trees from overlays and store workspace packs`.

### Task 25: `WorkspaceService`: lifecycle, snapshots and access
**Files:** `proto/loams/repos/v1/workspaces.proto`, `crates/loams/src/api/workspaces.rs`, `crates/loams/tests/git/workspaces.rs`. **Interfaces:** `CreateWorkspace`, `GetWorkspace`, `ListWorkspaces`, `ForkWorkspace`, `DeleteWorkspace`, `ListSnapshots`, `GetSnapshot`, `SaveSnapshot` (checks that `worktree_tree`, `index_tree`, `head` and `snapshot_commit` exist in the repository or the workspace packs it names, and that the trees are connected; fenced on `n`), `WatchWorkspace`, `ListSalvage`, `RestoreSalvage`. Access follows Q750's default: repository readers can read, and writing or taking over needs ownership or `WsMatch::All` with `Write`. Tests: `create_get_list_delete_round_trip`; `save_snapshot_fenced_on_n`; `save_with_missing_tree_is_object_missing`; `other_owner_cannot_write_or_take_over`; `repo_reader_can_read_ws` (Q750 default; a config flag flips it, tested both ways); `watch_ws_snapshot_then_saves_then_heartbeat`; `delete_writes_deleted_label`; `restore_salvage_replays_as_edits`. Commit `api: add WorkspaceService lifecycle and snapshots`.

### Task 26: Mount-less editing: `ReadFiles`, `ApplyEdits`, `Diff`, `Rebase`
**Files:** `src/ws/{edits.rs,merge3.rs}`, `crates/loams/src/api/workspaces.rs`, `crates/loams/tests/git/ws_edits.rs`. **Interfaces:** `ReadFiles(ws@n, paths)` (env rendering through Task 62's hook, redacting until then); `ApplyEdits(ws, expect_n, edits[])`, which builds trees (Task 24), writes one workspace pack and appends a snapshot (seal and scan hooks for Tasks 62 and 67); `Diff(ws@n, against: Head | Base | Index | Commit)` streaming unified diffs (the porcelain's Myers, Task 38's crate, used as a library); `Rebase(ws, onto?)` (`merge3` over the changed paths only; conflicts recorded in `conflicts`). `merge3` is a clean-room diff3 over the Myers diff (Ruling 36). Tests: `apply_edits_then_read_files_round_trip`; `apply_edits_fenced_on_expect_n`; `rebase_disjoint_paths_is_clean`; `rebase_same_path_conflict_is_recorded`; `rebase_reads_only_changed_paths` (counts blob reads); `merge3_matches_git_merge_file` (2,000 seeded cases, clean and conflicting, compared on output and exit code); `diff_matches_git_diff`. Commit `git: edit, diff and rebase workspaces without a mount`.

### Task 27: `PushWorkspace`
**Files:** `src/ws/push.rs`, `crates/loams/src/api/workspaces.rs`, `crates/loams/tests/git/ws_push.rs`. **Interfaces:** `PushWorkspace(ws@n, updates[], options, idempotency_key)` per §36 §19.7.5: the closure of new tips the repository lacks (`missing_from_closure` against `snapshot(Latest)`), read from `WsOdb`, written as one `.lpk` (old GT2's pack writer in copy mode), then one `RefTxn` through `RepoSequencers` (Task 14), with fast-forward, protection, scope and Task 67's scan. The answer carries `{seq, consistency_token, per-ref status}`. Tests: `push_workspace_moves_no_client_bytes` (counts client upload bytes: 0); `push_closure_is_minimal` (equals `git rev-list --objects new --not old`); `push_non_ff_is_rejected`; `push_force_with_lease`; `push_is_idempotent_on_key`; `push_racing_another_push_fences_and_revalidates`; `pushed_branch_clones_with_stock_git`. Commit `git: push workspaces without re-uploading objects`.

## GT1e — `loams-vfs` (Tasks 28–35)

### Task 28: The local store
**Files:** `crates/loams-vfs/{Cargo.toml,src/{lib.rs,limits.rs,config.rs,store/{meta.rs,cache.rs,overlay.rs,journal.rs}}}`, `tests/store.rs`. **Interfaces:** `MetaDb` (`meta.sqlite` in WAL mode, one writer task; tables `inodes(ws, path, ino, generation)`, `workspaces(ws, repo, last_n, writer, mount_id, read_only)`, `journal(ws, seq, path, kind, at)`, `fetchlog(ws, cmd, path, at)`); `ObjectCache` (content-addressed in an `Fs`, LRU by atime, `CACHE_DEFAULT_BYTES`, `pin`/`unpin` for open files); `Overlay` (real files at `overlay/<ws_id>/<path>`, copy-up, whiteouts as journal entries); `Journal::{append, since(token), truncate_through(n)}`. Tests: `meta_survives_restart`; `inode_numbers_are_stable_across_restart`; `cache_respects_budget`; `pinned_entries_survive_eviction`; `cache_deleted_mid_run_refetches`; `journal_since_token_is_exact`; `overlay_copy_up_preserves_mode`. Commit `vfs: add the local store`.

### Task 29: Backends and the fetcher
**Files:** `src/{backend/{mod.rs,api.rs,direct.rs},fetch.rs}`, `tests/fetch.rs`. **Interfaces:** `WorkspaceBackend: WorkspaceStore + ObjectSource + History` (History has `log`, `blame`, `grep` and `diff_stat`); `ApiBackend` (connectrpc client, a vended token from `loams` credentials); `DirectBackend` (the `loams-git` core against a `loams::<store-url>`, computing tree aux and history locally; for local-first use); `Fetcher` (deduplicates concurrent requests for one oid, batches up to `FETCH_BATCH_OIDS` with a 2 ms linger, applies `FETCH_TIMEOUT`, writes into `ObjectCache`). Tests: `concurrent_reads_of_one_blob_fetch_once`; `batching_groups_requests`; `fetch_timeout_is_eio`; `direct_and_api_backends_agree` (the same scenario on both; equal snapshots and objects); `offline_serves_cached`. Commit `vfs: add API and direct backends and the batching fetcher`.

### Task 30: The FUSE read path
**Files:** `src/{inode.rs,fuse.rs,mount.rs}`, `tests/fuse_read.rs` (in `gt1-fuse.yml`). **Interfaces:** `VfsFs` implementing `fuser::Filesystem` for `lookup`, `getattr`, `readdir(plus)`, `open`, `read`, `readlink`, `statfs`, `release`; a root `~/loams` with `<ns>/<repo>/<owner>/<ws>/` directories (listing `<owner>` levels only for the caller's own and opened workspaces); sizes and modes from tree aux data; mtimes per Ruling 40; passthrough per Ruling 41; per-directory prefetch to `TREE_PREFETCH_DEPTH` on `readdir`. Tests: `mount_shows_whole_tree`; `stat_fetches_no_blob`; `first_read_fetches_one_blob`; `read_after_cache_eviction_refetches`; `passthrough_used_when_available` (skipped with a printed reason on older kernels); `mtime_is_base_commit_time`; `symlinks_and_exec_bits_match_git_checkout`; `readdir_prefetches_children`. Commit `vfs: serve a lazy read-only tree over FUSE`.

### Task 31: The FUSE write path, ignored paths and redirections
**Files:** `src/{write.rs,ignore.rs,redirect.rs}`, `tests/fuse_write.rs`. **Interfaces:** `create`, `write`, `setattr` (truncate, chmod for the executable bit only, mtime), `rename`, `unlink`, `mkdir`, `rmdir`, `symlink`, `fsync`, `flush`; `link` returns `EPERM`; every mutation appends to the journal; `IgnoreMatcher` (`gix-ignore` over `.gitignore` and `.git/info/exclude`) marks paths as local-only; `Redirections` (`.loams/redirections.toml`, the defaults of §36 §19.6.4) served as passthrough to `redirect/<ws_id>/`; cones from the grant (`EACCES` outside them). Tests: `write_then_read_round_trip`; `rename_across_directories`; `unlink_of_lazy_file_needs_no_fetch`; `truncate_on_open_needs_no_fetch`; `hard_link_is_eperm`; `ignored_paths_never_journalled_for_upload`; `redirected_target_is_native_speed` (writes 1 GiB into `target/` within 1.5× the speed of a plain disk directory); `write_outside_cones_is_eacces`; `fsync_makes_overlay_durable` (kill -9 after fsync; data present after restart). Commit `vfs: accept writes through an overlay with redirections`.

### Task 32: The save pipeline
**Files:** `src/save.rs`, `tests/save.rs`. **Interfaces:** `Saver` per §36 §19.5.3: trigger, debounce (`SAVE_DEBOUNCE`, `SAVE_MIN_INTERVAL`), journal since the last save, the seal and scan hooks (Tasks 62 and 67), hashing on a blocking pool, `TreeBuilder`, the snapshot commit, `has_objects`, one workspace pack, `append`, the hint; `Saver::flush(ws) -> SnapN` for the shim; crash recovery that replays journal entries newer than the last committed snapshot. Tests: `save_uploads_only_touched_files` (counts uploaded blobs); `unchanged_content_is_not_uploaded` (a has-check hit); `debounce_coalesces_bursts` (10,000 writes in 2 s give at most 3 snapshots); `flush_returns_durable_n`; `crash_before_snapshot_replays_journal`; `crash_after_pack_before_snapshot_is_harmless`; `snapshot_records_changed_hint`; `untracked_files_are_in_worktree_tree_not_index`. Commit `vfs: save touched files as workspace snapshots`.

### Task 33: Mounting any workspace: writers, takeover, salvage and the control socket
**Files:** `src/{takeover.rs,salvage.rs,control.rs,bin/loams-vfsd.rs}`, `tests/mount.rs`. **Interfaces:** `loams-vfsd` (one per user; `--root`, `--foreground`; systemd `--user` unit text in `docs/guides/workspaces.md`); `Mount { take, read_only }` per §36 §19.4.3 (`WRITER_IDLE`); fencing handling (salvage, then read-only, then a desktop notification through agentd when present, else a log line); read-only `@n` pins and follow mode (`WatchWorkspace` or hint polling); the control protocol of the shared contracts; the mounts file. Tests: `mount_from_second_host_sees_saved_state` (two vfsd instances on two cache dirs, one store); `two_writers_one_is_fenced_and_salvaged`; `takeover_fences_old_writer`; `idle_writer_is_taken_over_automatically`; `read_only_follow_sees_whole_snapshots`; `pinned_mount_never_changes`; `control_socket_refuses_other_uid`; `mounts_file_is_atomic`. Commit `vfs: mount any workspace with fenced writers, takeover and salvage`.

### Task 34: Prefetch profiles, the fetch log and offline behaviour
**Files:** `src/prefetch.rs`, `tests/prefetch.rs`. **Interfaces:** `loams vfs prefetch --profile <name>` and `--paths`; profiles in `.loams/profiles/<name>.txt`; the fetch log, keyed by the reading process's command name (from `/proc/<pid>/comm` of the FUSE request's pid); `loams vfs profile suggest --cmd cargo` writes a profile from the log; `loams vfs status` (online or offline, queued saves, cache use). Tests: `profile_prefetch_fetches_matching_blobs_in_batches`; `suggest_profile_from_fetch_log`; `offline_status_reported_and_saves_resume`. Commit `vfs: add prefetch profiles and offline status`.

### Task 35: POSIX conformance and real workloads
**Files:** `tests/posix.rs`, `.github/workflows/gt1-fuse.yml`, `bench/git-vfs/`. **Semantics:** a POSIX subset suite (`pjdfstest`, BSD-2-Clause, run as a process, limited to the operations §36 §19.6.3 supports, with each expected failure listed and justified); real workloads in a mount: `cargo build` of a fixture workspace, `npm ci` into a redirected `node_modules`, `pytest` on a fixture, and an editor-style atomic save (write a temp file, fsync, rename). Tests: `pjdfstest_subset_passes`; `cargo_build_in_mount`; `editor_atomic_save_round_trip`; `npm_ci_in_redirected_node_modules`; `bench_status_and_read_latency` (records §36 §19.7.6's figures; enforced by Task 80). Commit `vfs: run POSIX conformance and real workloads in CI`.

## GT1f — The `git` shim (Tasks 36–45)

### Task 36: Dispatch, passthrough, installation and clone takeover
**Files:** `crates/loams-git-shim/{Cargo.toml,src/main.rs}`, `crates/loams-git-porcelain/src/{lib.rs,args.rs}`, `crates/loams-git-shim/tests/dispatch.rs`. **Interfaces:** the binary `loams-git`, following §36 §19.7.1: real-git discovery (`LOAMS_REAL_GIT`, then `PATH` minus self, cached in `$LOAMS_HOME/real-git`); effective-directory resolution (`-C`, `--git-dir`, `--work-tree`, `GIT_DIR`, `GIT_WORK_TREE`, cwd); the mounts-file check; `exec` of real git with argv and environment untouched; the `LOAMS_GIT_IN_FALLBACK` guard; `git clone` takeover for Loams URLs (detecting the `loams-workspaces` capability for `https://`), with `--no-loams` and `LOAMS_GIT_CLONE=real`; `loams git shim install|uninstall|status` (a symlink `$LOAMS_HOME/bin/git → loams-git`, and the shell profile line with a prompt, Q752). Tests: `outside_mount_execs_real_git_untouched` (argv, env, exit code, stdin, and the tty passed through); `passthrough_overhead_under_budget` (p99 under `SHIM_PASSTHROUGH_BUDGET` over 1,000 runs); `real_git_discovery_skips_self`; `nested_git_in_fallback_goes_to_real_git`; `dash_c_into_mount_is_native`; `clone_loams_url_creates_workspace_and_links`; `clone_no_loams_is_real_clone`; `install_and_uninstall_are_reversible`. Commit `shim: take over git with passthrough outside workspaces`.

### Task 37: The porcelain core: allowlists, state, `status` and plumbing
**Files:** `crates/loams-git-porcelain/src/{allow.rs,view.rs,revparse.rs,format/status.rs,cmd/{status.rs,plumbing.rs}}`, `tests/{allow.rs,differential.rs}`. **Interfaces:** `Allowlist` (per subcommand: accepted flags, value forms and the config keys that change behaviour; anything else gives `Native::FallThrough(reason)`); `WsView` (the snapshot plus vfsd's live journal, fetched with `Flush` then `State`); revision parsing (`HEAD~n`, `^`, `@{u}`, `<branch>`, abbreviated oids, `A..B`, `A...B`); `status` in every form of §36 §19.7.2; `rev-parse`, `ls-files`, `cat-file`, `config --get/--list` (read through `gix-config` with git's precedence), `symbolic-ref`, `merge-base`, `rev-list --count`. Tests: `unknown_flag_falls_through` (generated: every subcommand × a fuzzed flag); `status_matches_real_git` (all forms, 500 seeded states); `status_fetches_no_blob`; `status_porcelain_v2_matches`; `rev_parse_matches_real_git`; `ls_files_matches_real_git`; `cat_file_batch_matches`; `config_precedence_matches_git`; `native_startup_under_budget`. Commit `shim: native status and plumbing on an exact allowlist`.

### Task 38: `diff` and `show`
**Files:** `src/{myers.rs,format/diff.rs,cmd/{diff.rs,show.rs}}`, `tests/differential.rs`. **Interfaces:** Loams' Myers diff with git's default heuristics (indent heuristic on, as git's default since 2.14; the porcelain matches `diff.indentHeuristic`); unified output with headers, mode lines, rename detection (`-M` default 50%, as git's default for `diff`), binary detection, `--stat`, `--numstat`, `--name-only`, `--name-status`, `-U<n>`, `--color` (git's default palette and `color.diff.*`), `--exit-code`, `--quiet`; env files rendered key-level from Task 64 (line diff until then). Tests: `myers_matches_git_diff_10k_seeded` (Ruling 36); `rename_detection_matches_git`; `binary_files_differ_line_matches`; `stat_width_matches_git` (`COLUMNS` 80 and 120); `color_output_matches_git`; `diff_cached_and_ranges_match`; `show_commit_matches_git`; `patience_falls_through`. Commit `shim: native diff and show, byte-identical to git`.

### Task 39: `add`, `rm`, `mv`, `restore`, `reset`
**Files:** `src/cmd/{add.rs,rm.rs,mv.rs,restore.rs,reset.rs}`. **Interfaces:** index edits as `WsOp::SetIndex`; worktree edits through the mount; `reset --hard` swaps the overlay for the target tree without fetching; `add` respects ignore rules and `-f`; env files are sealed at `add` through Task 62's hook. Tests: `add_rm_mv_match_real_git` (seeded sequences, compared on the following `status --porcelain=v2` and `ls-files -s`); `reset_soft_mixed_hard_match`; `restore_staged_and_source_match`; `reset_hard_fetches_nothing`; `add_patch_falls_through`. Commit `shim: native index and worktree edits`.

### Task 40: `commit` and hooks
**Files:** `src/cmd/{commit.rs,hooks.rs}`. **Interfaces:** commit objects identical to git's for the same inputs (author and committer from config and environment, `GIT_AUTHOR_*` and `GIT_COMMITTER_*`, dates in git's formats, message cleanup per `commit.cleanup`, the editor through `GIT_EDITOR`, `core.editor`, `VISUAL` and `EDITOR`); hooks `pre-commit`, `prepare-commit-msg`, `commit-msg` and `post-commit` run with git's environment and arguments; `--amend`, `-a`, `--allow-empty`, `--no-verify`; signing configured gives fallthrough; `commit` waits for `Flush` (remote durability). Tests: `commit_oid_matches_real_git` (same inputs and dates give the same oid); `message_cleanup_modes_match`; `hooks_run_in_order_with_git_env`; `failing_pre_commit_aborts_like_git`; `amend_matches`; `gpgsign_falls_through`; `commit_waits_for_durable_snapshot`. Commit `shim: native commit with hooks`.

### Task 41: `branch`, `switch`, `checkout`, `tag`
**Files:** `src/cmd/{branch.rs,switch.rs,tag.rs}`. **Interfaces:** workspace-local refs (`WsOp::SetRefs`); `switch` and `checkout` swap the base as an O(1) `SwitchBase`, refuse when overlay paths conflict (git's message, byte-identical), carry clean overlay changes, and update mtimes of changed files only (Ruling 40); upstream tracking (`-u`, `--set-upstream-to`, `branch -vv` against `refs/remotes/origin/*` from the last `fetch`); lightweight tags. Tests: `branch_ops_match_real_git`; `switch_is_o1_and_fetches_nothing`; `switch_conflict_message_matches_git`; `checkout_paths_matches`; `branch_vv_matches`; `annotated_tag_falls_through`. Commit `shim: native branches, switching and tags`.

### Task 42: `log`, `blame`, `grep`
**Files:** `src/{format/{pretty.rs,log.rs},cmd/{log.rs,blame.rs,grep.rs}}`. **Interfaces:** `log` and `show` formatting over `ObjectSource::log` (`--oneline`, `--format`/`--pretty` with the placeholder table `%H %h %T %t %P %p %an %ae %ad %ar %at %ai %aI %cn %ce %cd %cr %ct %ci %cI %s %b %B %d %D %n %%`, `-n`, `--stat`, `--name-only`, `--name-status`, `-p`, `--graph` up to 1,000 commits, `--all`, `--author`, `--since`, `--until`, `--grep`, ranges, pathspecs); local commits not yet pushed are merged in from the workspace; `blame` over `Blame` with overlay lines shown as `Not Committed Yet` exactly as git; `grep` over `Grep` plus local overlay search, where the overlay result replaces the server's for the same path. Tests: `log_formats_match_real_git` (every placeholder; 200 histories); `log_includes_local_commits`; `log_graph_matches_under_limit`; `blame_matches_including_uncommitted`; `grep_merges_overlay_and_server`; `log_follow_falls_through`. Commit `shim: native log, blame and grep through server history`.

### Task 43: `fetch`, `pull`, `push`
**Files:** `src/cmd/{fetch.rs,pull.rs,push.rs}`. **Interfaces:** `fetch` refreshes `refs/remotes/origin/*` from `ListRefs` (no objects) and prints git's ref-update lines; `pull` (rebase by default in a workspace; `--no-rebase` falls through) runs `WsOp::SwitchBase` plus `merge3` over changed paths only, replays local commits natively when every one is clean and falls through to real `git rebase` otherwise; `push` through `PushWorkspace` with git's output format (`To <url>`, ref lines, `[rejected]` reasons, `-u` setting upstream); a remote that is not the workspace's repository falls through. Tests: `fetch_downloads_no_objects`; `pull_downloads_only_changed_paths`; `pull_conflict_writes_markers_like_git`; `pull_replays_local_commits_like_git_rebase`; `push_output_matches_real_git`; `push_rejected_non_ff_matches`; `push_to_other_remote_falls_through`. Commit `shim: native fetch, pull and push for workspaces`.

### Task 44: `worktree` as workspaces
**Files:** `src/cmd/worktree.rs`. **Interfaces:** `worktree add <path> [-b <b>] [<commit>]` (`CreateWorkspace` from the current HEAD, then `Mount`, then a symlink at `<path>`, or `--mount` per Ruling 33); `list` (git's format, listing the user's workspaces of this repository with their link paths); `remove [--force]` (refuses unsaved or unpushed work as git refuses a dirty worktree; deletes the workspace); `move`; `prune`. Tests: `worktree_add_is_one_put_and_mounts`; `worktree_list_format_matches_git`; `worktree_remove_refuses_dirty_like_git`; `hundred_worktrees_add_under_30s`; `worktree_lock_falls_through`. Commit `shim: worktrees are Loams workspaces`.

### Task 45: Fallthrough on a projected `.git`, and the agent corpus
**Files:** `crates/loams-vfs/src/{projection.rs,absorb.rs,fsmonitor.rs}`, `crates/loams-git-shim/src/fallthrough.rs`, `crates/loams-vfs/tests/projection.rs`, `crates/loams-git-porcelain/tests/corpus/agent-commands.txt`, `tests/differential.rs`. **Interfaces:** `Projection::ensure(ws)` per §36 §19.7.4 (HEAD, refs, packed-refs, config, sparse index through `gix-index` or Task 0's fallback, `loams-projection` stamp, the per-workspace promisor store); `loams-git fsmonitor` (protocol v2, answering from `Changed { since }`); `Absorb` (HEAD, refs, index with conflicts, new loose and packed objects into the next save); the notice line and `loams.quietFallback`; absorption of out-of-band changes to `.git` at save. Tests: `projection_is_valid_partial_clone` (`git fsck --connectivity-only` and `git status` on the projection equal the native status); `rebase_via_fallthrough_absorbs` (real `git rebase -i` with a scripted editor gives the same state as on a real clone); `stash_via_fallthrough_round_trips`; `ide_libgit2_write_absorbed_at_save` (a pygit2 commit on the projection); `fsmonitor_answers_from_journal`; `notice_printed_once_and_logged`; `agent_corpus_matches_real_git` (every corpus line × 2,000 seeded states, native or fallthrough, compared byte for byte with a real clone in the same state; nightly at full size, 100 states on PRs). Commit `shim: fall through to real git on a projected .git`.

## GT1g — Interop: Smart HTTP and the helper (Tasks 46–52)

These are the morning version's Tasks 19–26 (old GT2 Tasks 1–10) with `RangeOdb` already built (Task 17).

### Task 46: pkt-lines, the listener and advertisements
As the morning version's Task 19, plus the capability `loams-workspaces` (§36 §19.9). Tests: `pktline_round_trip_and_limits`; `advertise_v2_matches_design`; `advertise_receive_has_capabilities_after_nul`; `v0_upload_request_is_400_with_hint`; `unknown_namespace_is_404`; `deleting_repo_is_404`; `git_ls_remote_against_empty_repo`; `stock_git_ignores_loams_capability`. Commit `git: serve Smart HTTP advertisements`.

### Task 47: `ls-refs`, `object-info` and hidden workspace refs
As the morning version's Task 20, plus `refs/loams/ws/<ws_id>` hidden unless requested by prefix, and snapshot commits allowed as wants for callers who may read the workspace (D835). Tests: `ls_refs_prefix_filters`; `ls_refs_symrefs_and_unborn`; `ls_refs_peels_tags`; `ls_refs_at_least_seq_reads_own_push`; `object_info_sizes`; `ws_refs_hidden_by_default`; `fetch_ws_snapshot_commit_with_stock_git`; `ws_ref_needs_ws_read`. Commit `git: answer ls-refs and object-info, with hidden workspace refs`.

### Task 48: Negotiation, commit graph and shallow
As the morning version's Task 22, using Task 20's `GraphView`. Tests: `clone_negotiates_with_no_haves`; `fetch_after_push_sends_only_new_commits`; `ready_ends_negotiation_early`; `deepen_1_matches_git`; `deepen_since_and_not`; `want_ref_resolves`; `negotiation_round_limit_is_enforced`. Commit `git: negotiate fetches with a commit graph, shallow and deepen`.

### Task 49: Pack assembly and filters
As the morning version's Task 23. Tests: `clone_round_trips_through_git_fsck`; `partial_clone_blob_none_has_no_blobs`; `blob_limit_filters_by_size`; `tree_zero_has_only_commits`; `thin_pack_resolves_on_client`; `whole_pack_reuse_after_repack`; `large_clone_bounded_rss`; `pack_size_within_1_5x_of_git`. Commit `git: assemble fetch packs with delta reuse and partial-clone filters`.

### Task 50: `receive-pack`
As the morning version's Task 24, with Task 63's env check and Task 67's scan as hooks (no-ops until then). Tests: `push_new_branch`; `push_ff_and_reject_non_ff`; `atomic_push_all_or_none`; `non_atomic_push_reports_per_ref`; `delete_ref_with_delete_refs`; `push_options_reach_the_wal`; `malformed_pack_is_refused`; `missing_base_is_refused`; `connectivity_gap_is_refused`; `protected_branch_refuses_force_and_delete`; `out_of_scope_ref_is_refused`; `ack_after_segment_commit`; `retried_push_is_replayed`; `two_gib_push_bounded_rss`; `max_refs_and_max_pack_limits`; `receive_idle_timeout_aborts_cleanly`. Commit `git: receive pushes with streaming verification and group commit`.

### Task 51: `git-remote-loams`
The morning version's Tasks 12 and 25 combined: serverless `fetch`/`push`/`option` over `loams::<store-url>` (Rulings 2–7), then `stateless-connect` and `loams://` addresses. It is also the projection's promisor transport (Task 45). Tests: every test of the morning version's Tasks 12 and 25 (`clone_of_missing_repo_fails` … `fetch_after_other_push_gets_new_pack`; `concurrent_pushes_to_distinct_branches_all_land`; `concurrent_pushes_to_one_branch_one_wins_per_round`; `concurrent_pushes_never_lose_an_acked_push`; `push_survives_lost_acks`; `repeated_push_after_lost_answer_is_replayed`; `partial_clone_through_helper`; `lazy_fetch_of_missing_blob_through_helper`; `sparse_checkout_fetches_only_cone_blobs`; `loams_url_reaches_server`; `push_to_loams_url_uses_receive_pack`), plus `promisor_fetch_reads_ws_objects`. Commits `git: add git-remote-loams, a serverless remote helper over the bucket`; `git: tunnel protocol v2 through git-remote-loams`.

### Task 52: The client matrix, the differential suite, and v0 if needed
As the morning version's Tasks 26 and 27 (v0/v1 upload-pack only if Task 0 found a matrix client without v2; otherwise record "not needed"). Tests: the matrix rows; `differential_object_sets_match`; `v0_clone_and_fetch` (conditional). Commit `ci: run the git client matrix and the upload-pack differential suite`.

## GT1h — Compaction, GC and integrity (Tasks 53–55)

### Task 53: Compaction, including workspace packs
As the morning version's Task 28, with two changes: `git commit-graph write --reachable --changed-paths`, and **workspace pack compaction** (§36 §19.4.5). A worker task `git-ws-compact/<ns>/<ws_id>` merges the reachable objects of a workspace's packs once a snapshot names more than `WS_PACKS_COMPACT_AT`. It writes a merged pack. vfsd (or `ApplyEdits`) names the merged pack in its next save, fenced on the `n` the compactor read; a compactor that loses the race leaves an orphan pack for GC. Tests: `repack_preserves_every_reachable_object`; `readers_see_old_or_new_pack_set`; `concurrent_push_during_compaction_keeps_new_pack`; `compaction_lease_is_exclusive`; `compaction_mirror_deleted_mid_run_recovers`; `repack_time_per_mib_recorded`; `commit_graph_has_bloom_filters`; `ws_pack_compaction_adopted_on_next_save`; `ws_compaction_losing_race_leaves_only_garbage`. Commit `git: compact repositories and workspace packs`.

### Task 54: Pack GC, workspace retention and deletion
As the morning version's Task 29, plus workspace retention per §36 §19.4.5 (snapshot thinning, workspace pack GC, salvage expiry, deleted-workspace purge, references from forked workspaces) and orphan workspace packs older than `GC_GRACE`. Tests: `pack_gc_respects_grace_and_forks`; `fork_family_reachability_keeps_parent_packs`; `gc_claim_blocks_second_collector`; `deleted_repo_is_purged_after_grace`; `deleted_parent_with_live_fork_keeps_packs`; `snapshot_thinning_keeps_hourly_then_daily`; `named_and_latest_snapshots_kept`; `ws_pack_gc_keeps_named_and_forked`; `orphan_ws_pack_collected_after_grace`; `gc_races_push_workspace` (a pack read by an in-flight `PushWorkspace` survives); `deleted_ws_purged_after_grace`. Commit `git: collect retired packs, thin snapshots and purge deleted workspaces`.

### Task 55: Integrity scrubs
As the morning version's Task 30, plus sampled workspaces: decode each sampled workspace's latest snapshot and check that its trees and snapshot commit are reachable from its packs or the repository. Tests: `scrub_detects_missing_pack`; `scrub_detects_corrupt_footer`; `clean_repo_scrubs_clean`; `scrub_detects_ws_missing_tree`. Commit `git: scrub sampled repositories and workspaces nightly`.

## GT1i — Agent scale and integrations (Tasks 56–58)

### Task 56: Workspaces in sandboxes
**Files:** `crates/loams-vfs/src/mount.rs`, `docs/guides/workspaces.md` (sandbox section), `crates/loams-vfs/tests/sandbox.rs`. **Semantics:** three ways in, each tested. (1) Bind-mount a workspace directory from the host's vfsd into a container (`--mount type=bind`, with `propagation` documented). (2) vfsd inside an unprivileged user namespace with `/dev/fuse`. (3) A virtiofs export recipe for microVMs (documented, and smoke-tested where the CI runner has KVM). The agent's token is scoped to its workspace and cones. Tests: `bind_mounted_ws_in_container_reads_and_saves`; `vfsd_in_user_namespace_mounts`; `agent_token_cannot_mount_other_ws`; `virtiofs_recipe_smoke` (KVM runners only, skipped with a printed reason). Commit `vfs: run workspaces in sandboxes`.

### Task 57: `loams-agentd` and software-factory integration
**Files:** `crates/loams-agentd-sessions/src/…` (as Task 0 finds), `crates/loams-agentd/src/…` (supervising vfsd as D790 supervises the engine), `docs/design/50-loams-desktop-daemon.md` (an "As built (GT1i)" cross-reference only). **Semantics:** for a repository whose remote is a Loams URL, agentd's `CreateWorktree` creates a workspace (`WorkspaceService`) and mounts it through vfsd, instead of running `git worktree add`. `DeleteWorktree` deletes the workspace. Each harness run gets a token scoped to its workspace with no env grant (D844). §39's `forgejo.propose_patch` uses `ApplyEdits`, `Rebase` and `PushWorkspace` when it runs without a mount. Tests: `agentd_create_worktree_makes_workspace`; `agentd_supervises_vfsd_restart`; `harness_run_token_scoped_to_ws`; `propose_patch_without_mount_pushes_branch`. Commit `vfs: back agentd worktrees with Loams workspaces`.

### Task 58: Agent-scale benchmark
**Files:** `bench/git-ws/`, `scripts/git/mono-fixture.sh`. **Semantics:** a synthetic monorepo (`mono-fixture.sh`: 10 million files, depth 12, 200 GB of history generated deterministically, Q755) and the Linux kernel's history. Scenarios: create 1,000,000 workspaces; 1,000 concurrent agents on 4 hosts each mounting a workspace, editing 20 files, saving every 10 s and pushing every 5 min for 1 h; 10,000 mount-less agents through `ApplyEdits`. Every scenario records PUT and GET counts per operation against §36 §19.11's model. Tests: `million_workspaces_create` (p99 create latency and store objects recorded); `thousand_agents_share_cache` (host cache hit rate at least 95% after warm-up; no duplicate fetch of one blob per host); `agent_hour_requests_within_model`; `mountless_agents_10k`. Commit `bench: agent-scale workspaces`.

## GT1j — Env: secrets as code (Tasks 59–67)

### Task 59: `loams-vault`: versions and KMS envelope encryption
**Files:** `crates/loams-vault/{Cargo.toml,src/{lib.rs,keys.rs,version.rs,store.rs}}`, `tests/vault.rs`. **Interfaces:** `KeyProvider` (D96: `wrap(dek) -> WrappedKey`, `unwrap(WrappedKey) -> Secret<Dek>`, `key_id()`) with `FileKeyProvider` and, behind features, the AWS, GCP and Azure providers (Task 0's ruling); `SecretId`; `VersionRecord { secret, version, fingerprint, state: Draft | Live | Superseded | Destroyed, created_by, created_unix_ms, cause }`; `Vault::{put_version, get_version, set_state, destroy, list_versions}` over create-only `.lsv` objects (magic `LVAULTSV`, AES-256-GCM, the data key wrapped, AAD = (namespace, secret, version)); `repo_fp_key(repo)` (created once, wrapped). Tests: `version_round_trip`; `ciphertext_bound_to_secret_and_version` (moving an object to another path fails decryption); `put_version_is_create_only_and_idempotent`; `destroy_removes_body_keeps_record`; `kms_unavailable_is_retryable_error`; `no_plaintext_in_store`; `fp_key_created_once_under_race`. Commit `vault: store secret versions under KMS-wrapped data keys`.

### Task 60: External vault adapters
**Files:** `crates/loams-vault/src/{backend/{mod.rs,loams.rs,openbao.rs,aws.rs,azure.rs,gcp.rs},conformance.rs}`, `scripts/git/openbao.sh`, `tests/backends.rs`. **Interfaces:** `SecretBackend::{put(secret, version, value) -> BackendRef, get(BackendRef) -> Secret<Bytes>, destroy(BackendRef), health()}`; `LoamsVaultBackend` (Task 59); `OpenBaoBackend` (KV v2: `data/`, `metadata/`, `destroy/`; token and AppRole auth; also HashiCorp Vault); `AwsSecretsManagerBackend` (`PutSecretValue` with `VersionStages`); `AzureKeyVaultBackend` (secret versions); `GcpSecretManagerBackend` (`addVersion`, `destroy`); `conformance::run(backend)`. The cloud adapters run against recorded HTTP fixtures in CI, and live only with credentials (GT-Q8's pattern). Tests: `conformance_loams`; `conformance_openbao` (the OpenBao dev container in CI); `conformance_vault_kv2_fixture`; `conformance_aws_fixture`; `conformance_azure_fixture`; `conformance_gcp_fixture`; `backend_credentials_never_logged`. Commit `vault: add OpenBao, Vault, AWS, Azure and GCP backends`.

### Task 61: The sealed env format and `EnvService` seal and open
**Files:** `crates/loams-env/{Cargo.toml,src/{lib.rs,limits.rs,format.rs,fingerprint.rs,seal.rs,render.rs,policy.rs}}`, `proto/loams/repos/v1/env.proto`, `crates/loams/src/api/env.rs`, `tests/format.rs`, `tests/golden/sealed_env_v1.env`. **Interfaces:** `EnvFile::parse` (dotenv per `dotenvy`'s rules, comments, `# loams:plain`, order kept); `SealedLine`; `fingerprint(repo_fp_key, key, value)`; `seal(file, previous: Option<&EnvFile>, sealer) -> SealedEnv` (reuses unchanged fingerprints with no vault call; otherwise `Seal`, idempotent on (secret, fingerprint)); `render(sealed, opener, grant) -> Rendered` (plaintext, redacted or rotated-annotated lines); `Policy::parse(env/policy.toml)` with Q758's default patterns; `EnvService.Seal` and `Open` (audited, §36 §19.13.7). Tests: `golden_sealed_env_v1`; `seal_is_deterministic_and_idempotent` (status-style repeated seals create 0 versions); `plain_marker_keeps_value`; `multiline_and_quoted_values_round_trip`; `render_redacts_without_grant`; `render_annotates_rotation`; `open_is_audited_without_values`; `limits_env_file_and_keys`; `no_plaintext_in_store`. Commit `env: add the sealed env format, sealing and rendering`.

### Task 62: Sealing and rendering in vfsd, the shim and `ApplyEdits`
**Files:** `crates/loams-vfs/src/env_render.rs`, `crates/loams-vfs/src/save.rs` (seal hook), `crates/loams-git-porcelain/src/cmd/add.rs` (seal at `add`), `crates/loams-git/src/ws/edits.rs` (seal on the server), `crates/loams-vfs/tests/env.rs`. **Interfaces:** vfsd renders env files at `lookup` and `open` for the mount's principal (in memory, `ENV_RENDER_TTL`), seals at save, refuses a changed redacted line with `EACCES` (log line naming the key), and offers `loams vfs scrub-env`; `ReadFiles` renders by the caller's grant; `ApplyEdits` seals server-side; write-without-read per Q760's default (sealing a new value blind is allowed with `Write`; audited). Tests: `authorised_mount_reads_plaintext`; `redacted_for_unauthorised`; `agent_without_grant_sees_redacted`; `edit_of_redacted_line_is_refused`; `unchanged_redacted_line_keeps_reference`; `blind_write_with_write_grant_seals_and_audits`; `rendered_plaintext_not_persisted` (scans the cache and `meta.sqlite`); `no_plaintext_in_store` (all GT1d and GT1e scenarios with env fixtures). Commit `env: seal on save and render on read`.

### Task 63: Server enforcement
**Files:** `crates/loams-git/src/protocol/receive.rs` (hook), `src/ws/{push.rs,edits.rs}`, `crates/loams/src/api/{objects.rs,workspaces.rs}`, `crates/loams-git/tests/env_enforce.rs`. **Semantics:** for every new blob at an env path (by the policy at the new tip), `receive-pack`, `PushWorkspace`, `PutWorkspacePack` and `ApplyEdits` require a fully sealed file whose references belong to this repository (the secret's `meta.json` names `repo_id`). Otherwise they refuse with `env_unsealed` / `env_foreign_reference` and report-status `env file <path> is not sealed: run 'loams env init' or push through Loams Git`. Forks get no env grants (D847). Tests: `unsealed_env_push_is_refused`; `foreign_reference_is_refused`; `fork_cannot_read_parent_secrets`; `policy_at_new_tip_decides_env_paths`; `sealed_push_from_plain_git_with_filter_is_accepted`. Commit `env: refuse unsealed env files and foreign references on every write path`.

### Task 64: Key-level diff and merge, and the plain-git filter
**Files:** `crates/loams-env/src/{diff.rs,merge.rs,filter.rs}`, `crates/loams-git-porcelain/src/cmd/diff.rs` (env rendering), `crates/loams/src/main.rs` (`loams env init|filter|diff-textconv|merge-driver`), `crates/loams-env/tests/{diff.rs,filter.rs}`. **Interfaces:** `key_diff(old, new) -> Vec<KeyChange>` rendered as in §36 §19.13.8; `merge3_keys(base, ours, theirs) -> Merged | Conflict(keys)`; `loams env filter` speaking git's long-running filter protocol (`filter.<driver>.process`: `clean` seals, `smudge` renders or redacts); the textconv and merge-driver entry points; `loams env init` writing `.gitattributes` and `env/policy.toml`. The projection's config (Task 45) points to these. Tests: `key_diff_shows_names_never_values`; `server_diff_and_grep_hide_values`; `merge_different_keys_clean`; `merge_same_key_conflict_shows_fingerprints`; `plain_git_with_filter_round_trips` (real git: clone, smudge, edit, add (clean), commit, push, accepted); `plain_git_clone_sees_only_sealed`; `filter_process_protocol_matches_git_docs` (capability negotiation, `status=success`, `abort`). Commit `env: review env changes by key and interoperate with plain git`.

### Task 65: Environments, promotion and delivery
**Files:** `crates/loams-vault/src/{env_rev.rs,promote.rs}`, `crates/loams/src/server.rs` (worker `env-promote`), `crates/loams/src/main.rs` (`loams env run|show|environments`), `crates/loams-vault/tests/promote.rs`, the Dapr secret-store component definition in `docs/guides/env.md`. **Interfaces:** `EnvRevStore` (create-only `.ler` chain plus hint; `live(env) -> LiveSet`); `EnvPromoter` (a `loams-durable` workflow per (repository, environment, seq), consuming Task 15's `GitEvents::subscribe()`, applying §36 §19.13.5 steps 1–3, emitting `io.loams.dev.env.promoted.v1` on `_env`); `EnvService.GetLiveSet`, `WatchEnvironment`, `ListEnvironments`; `loams env run --env <e> [--watch] -- <cmd>` (restarts the child on a new revision with `SIGTERM` and then `SIGKILL` after 10 s). The Live runtime reads through `GetLiveSet` (a seam documented in §45's terms; no Live code changes in this task). Tests: `merge_promotes_exactly_once` (with crash injection in the workflow); `merge_never_rolls_back_rotation`; `removed_key_leaves_live_set`; `unmapped_branch_does_not_promote`; `promotion_event_has_no_values`; `env_run_injects_and_restarts_on_change`; `live_set_needs_read_grant`. Commit `env: promote env changes on merge and deliver live sets`.

### Task 66: Rotation
**Files:** `crates/loams-vault/src/{rotate.rs,providers/{random.rs,loams_postgres.rs,loams_sql.rs,external.rs,webhook.rs}}`, `crates/loams/src/server.rs` (worker `env-rotate`), `crates/loams-vault/tests/rotate.rs`. **Interfaces:** `RotationPolicy` from `env/policy.toml` at the environment's branch tip; `RotationProvider::{create, revoke}`; the durable workflow of §36 §19.13.6, idempotent on (secret, rotation number); providers `random`, `loams-postgres` (A/B roles through pg-control, per Task 0), `loams-sql`, `external` (following the external vault's latest version) and `webhook` (HTTPS, an HMAC-signed body, a timeout and retries); `EnvService.Rotate` for manual rotation. Tests: `rotation_makes_no_commit` (repository seq unchanged); `rotation_writes_version_and_revision`; `old_credential_revoked_after_grace`; `rotation_crash_resumes_without_double_create`; `postgres_ab_rotation_keeps_connections_working` (PG2's test stack, CI e2e); `webhook_provider_signed_and_https_only`; `manual_rotate_operation`. Commit `env: rotate secrets by policy without commits`.

### Task 67: Leak prevention
**Files:** `crates/loams-env/src/scan/{mod.rs,rules.rs,known.rs,allowlist.rs}`, `crates/loams-env/rules/gitleaks.toml`, `scripts/env/gen-rules.sh`, `NOTICE`, hooks in Tasks 24, 26, 27, 32, 40 and 50, `crates/loams-env/tests/scan.rs`. **Interfaces:** `Scanner::scan(path, bytes, known: &KnownFingerprints) -> Vec<Finding { path, line, rule, certain }>`, which applies the rules (Ruling 48), entropy checks and known-value matching (§36 §19.13.9); `.loams/secret-allowlist.toml`; `EnvService.Allow`; the scan runs at the client points (save hold, native commit refusal) and the server points (`PutWorkspacePack`, `ApplyEdits`, `PushWorkspace`, `receive-pack`). Tests: `gitleaks_fixture_corpus_detected` (positive and negative fixtures per rule); `known_value_leak_is_refused` (a vault value pasted into `src/config.rs`); `save_holds_file_local_and_status_reports`; `commit_refuses_with_remedy`; `receive_pack_refuses_with_path_line_rule`; `allowlist_entry_needs_reason_and_is_audited`; `allowlist_never_allows_known_value`; `scanner_throughput` (at least 200 MiB/s per core on source text; records the figure). Commit `env: scan for leaked secrets on every write path`.

## GT1k — Build cache and crates mirror (Tasks 68–73)

These are the morning version's Tasks 31–36 (old GT3), unchanged. `CacheGrant` comes from `GitGrant`.

### Task 68: Keys, trust classes and the WebDAV subset
Tests: `put_then_get_round_trip`; `head_reports_length`; `miss_is_404`; `untrusted_put_to_trusted_is_403`; `scratch_is_private_to_its_principal`; `untrusted_reads_trusted`; `read_from_parent_repo`; `class_comes_from_token_not_path`; `other_namespace_in_url_is_404`; `oversized_put_is_413`; `non_loopback_is_refused`. Commit `cache: serve sccache's WebDAV backend with trust classes`.

### Task 69: Refresh-on-hit, the sweeper and the hooks
Tests: `hit_on_old_entry_refreshes_once`; `fresh_hit_does_not_copy`; `sweeper_deletes_expired`; `sweeper_enforces_quota_oldest_first`; `sweeper_lease_is_exclusive`; `hits_misses_puts_are_counted`. Commit `cache: add approximate LRU, the sweeper and the hooks`.

### Task 70: The direct path
Tests: `recipe_for_untrusted_is_read_only`; `r2_vendor_request_body_matches_api`; `r2_vendor_never_requests_admin_permissions`; `static_vendor_per_class`; `static_vendor_refuses_undeclared_scope`; `untrusted_key_cannot_write_or_read_outside_prefix`; `vended_secret_not_in_recipe_debug`. Commit `cache: add the direct path's credential vending`.

### Task 71: CI templates and the end-to-end builds
Tests: `cold_then_warm_hit_rate`; `untrusted_ci_cannot_write_trusted`; `direct_path_on_rustfs`; `multilevel_disk_then_webdav`; and `cargo_build_in_workspace_uses_cache` (sccache in a Loams workspace mount with `target/` redirected). Commit `ci: add the sccache template and the build-cache end-to-end job`.

### Task 72: The crates mirror
Tests: `config_json_has_dl_template`; `index_is_cached_and_revalidated_with_etag`; `index_lines_are_served_verbatim`; `invalid_index_path_is_404`; `crate_is_fetched_once_and_content_addressed`; `crate_with_wrong_cksum_is_refused_and_not_stored`; `download_of_hidden_version_is_404`; `denied_crate_is_404`; `pinned_versions_only`; `quarantined_version_is_hidden`; `audit_record_per_download`; `package_requests_are_counted`. Commit `registry: add the crates.io sparse-index mirror`.

### Task 73: The mirror end to end
Tests: `cargo_fetch_through_mirror`; `cargo_cannot_reach_upstream_directly`; `second_fetch_is_all_cache`. Commit `registry: run cargo end to end through the mirror`.

## GT1l — Production operations (Tasks 74–82)

### Task 74: Metrics, traces, dashboards and alerts
As the morning version's Task 37, plus these families (labels `org` and `namespace` only): `loams_git_ws_created_total`, `loams_git_ws_saves_total{result}`, `loams_git_ws_takeovers_total`, `loams_git_ws_fenced_total`, `loams_git_objects_served_total`, `loams_git_object_bytes_served_total`, `loams_git_push_workspace_seconds`; client-side vfsd families on a local `/metrics` socket route (`loams_vfs_fetch_seconds`, `loams_vfs_cache_hit_ratio`, `loams_vfs_save_seconds`, `loams_vfs_offline`, `loams_shim_native_total{cmd}`, `loams_shim_fallthrough_total{cmd}`); env (`loams_env_seals_total`, `loams_env_reads_total{result=plaintext|redacted|denied}`, `loams_env_promotions_total`, `loams_env_rotations_total{result}`, `loams_env_scan_findings_total{where}`). Alerts are added for rotation failures, promotion lag over 5 min and a fallthrough rate over 20% per command for 1 h (a sign the allowlist needs work). Tests: the morning version's (`metric_families_match_design`, `no_repo_label_anywhere`, `traceparent_reaches_wal_event`, `every_alert_fires_in_test`, `no_metering_guard_covers_git_crates`), plus `no_secret_in_any_label_or_span`. Commit `git: export usage hooks, traces, dashboards and alerts`.

### Task 75: Quotas and limits
As the morning version's Task 38, plus `max_workspaces_per_principal` (`MAX_WORKSPACES_PER_PRINCIPAL`), `max_ws_stored_bytes` per namespace, `max_saves_per_ws_per_min` (default 120), and `max_env_keys_per_repo` (default 10,000). Tests: the morning version's, plus `workspace_count_limit_refuses_create`; `save_rate_limit_backs_off_client`; `env_key_limit_refuses_seal`; and every new limit's at-limit and past-limit test. Commit `git: enforce quotas and limits`.

### Task 76: The failure table, deterministic simulation and a chaos soak
As the morning version's Task 39, with these rows added:

| Failure | Expected |
|---|---|
| vfsd killed between pack PUT and snapshot PUT | The orphan pack is collected after grace; the journal replays and the next save commits |
| Two hosts mount one workspace as writer through a partition | One writer; the other is fenced, salvaged and read-only |
| Server unreachable for 10 min during edits | Cached reads work; saves queue; nothing is lost; status reports offline |
| KMS unavailable | Seal and open fail retryably; saves of non-env files continue; env files stay local until KMS returns |
| `EnvPromoter` crash mid-workflow | Resumes; one revision per (environment, seq) |
| Rotation provider times out after creating a credential | Resume finds it by idempotency key; no second credential |

Tests: the morning version's eight; `vfsd_killed_between_pack_and_snapshot`; `partitioned_writers_one_fenced`; `server_outage_queues_saves`; `kms_outage_holds_env_only`; `promoter_crash_exactly_once`; `rotation_timeout_no_double_create`; `sim_10k_seeds_no_lost_ack`; `sim_no_lost_save` (a `loams-sim` workload of mounts, saves, takeovers, pushes, compaction and GC); `chaos_soak_6h`. Commit `git: add the failure table, simulation and chaos soak`.

### Task 77: Backup, disaster recovery and the restore drill
As the morning version's Task 40, plus workspaces (a restored replica mounts every sampled workspace at its latest snapshot with identical trees) and the vault (wrapped data keys restore only with the same KMS key; the runbook covers KMS key loss as unrecoverable by design). Tests: `rebuild_from_bucket_only`; `rebuild_without_hint_uses_list`; `restore_drill_matches_source`; `restored_workspaces_mount_identically`; `restored_vault_opens_with_same_kms_key`. Commit `git: add the restore drill and recovery runbooks`.

### Task 78: The security review
As the morning version's Task 41, plus these entry points in the threat model: vfsd's control socket, the FUSE surface (hostile file names, path lengths, a symlink pointing out of the mount, which is served as a symlink and never followed by vfsd), the projection (real git run on tenant data with `GIT_CONFIG_NOSYSTEM=1`, no system hooks, `protocol.allow` limited to `loams` and `file`), the shim's argv handling, env (fingerprint oracle limits, `Open` rate limits per principal, audit completeness), and the scanner (ReDoS: every rule compiled with the `regex` crate, which is linear-time). Fuzz targets add `snapshot`, `sealed_env` and `filter_protocol`. `git-licence.sh` adds `no_gpl_crate` (any GPL or AGPL licence expression in `cargo deny`'s output). Tests: the morning version's, plus `fuzz_snapshot`, `fuzz_sealed_env`, `fuzz_filter_protocol`, `hostile_filenames_in_tree_are_safe`, `projection_git_runs_without_system_config_or_hooks`, `open_rate_limited_per_principal`, `no_gpl_crate`. An external review closes every high and critical finding before GA. Commit `git: threat model, fuzzing and hardening`.

### Task 79: Single-node mode and deployment
As the morning version's Task 43, with `loams dev --git` now also starting vfsd (local-first: `DirectBackend` over the dev store), a file KMS key, and `loams git shim install --yes` in the compose demo. Tests: `dev_git_one_command_clone_push`; `compose_up_clone_push_down`; `dev_git_workspace_clone_edit_push` (`git clone loams://localhost/…`, then edit, commit and push through the shim). Commit `deploy: loams dev --git with workspaces and the compose stack`.

### Task 80: The performance gate
As the morning version's Task 42, plus §36 §19.7.6's budgets on the Task 58 fixtures (Q755): passthrough overhead, native `status`, `diff --stat`, workspace create, mount, first uncached read, save of 100 files, and push. Each budget is checked at p99 on RustFS on the reference machine; R2 figures are recorded when credentials exist (GT-Q8). Tests: the morning version's four, plus `bench_shim_budgets`, `bench_vfs_budgets`, `bench_ws_create_mount_save_push`. Commit `bench: the GT1 performance gate`.

### Task 81: CLI and SDK exposure
As the morning version's Task 44, with `loams repo …`, `loams ws create|list|show|fork|delete|snapshots|salvage|sync|mount|unmount`, `loams vfs status|prefetch|profile|scrub-env`, `loams env init|show|run|rotate|allow|environments`, `loams git shim install|uninstall|status`, and `loams git credential`, written into CL1's command vocabulary (CL1 owns the `loams` CLI layout; this task adds the groups through CL1's registry as built). Tests: `cli_repo_lifecycle`; `cli_ws_lifecycle`; `cli_env_show_requires_grant`; `git_credential_helper_returns_token`; `sdk_package_maps_name_repos`. Commit `cli: repository, workspace, vfs and env commands`.

### Task 82: Docs, runbooks, plan close and the GA gate
As the morning version's Task 45, plus the guides `workspaces.md`, `git-shim.md` and `env.md`, and runbooks for a stuck save queue, fence storms, cache corruption, a KMS outage, rotation failure and a leaked secret (destroy the version, rotate, scan history). §36 §19 decisions go to the decision log with their confirmed numbers. `loams.repos.v1` loses `unstable` when the exit criteria are green. Commit `docs: record Loams Git as built and close GT1`.

## After GA (Tasks 83–87)

### Task 83: The LFS-compatible read path
**Files:** `crates/loams/src/api/git_lfs.rs`, `crates/loams/tests/git/lfs.rs`. **Semantics:** the LFS batch API (`POST /git/<ns>/<repo>.git/info/lfs/objects/batch`, download only at first) serving objects named by LFS pointer files from the namespace CAS, so stock clients cloning repositories imported from LFS hosts get their content; `loams repo import --lfs` rewrites pointers to real blobs when the owner prefers lazy fetch. Tests: `git_lfs_pull_through_batch_api`; `pointer_import_rewrites_to_blobs`; `upload_is_refused_with_hint`. Commit `git: serve an LFS-compatible read path`.

### Task 84: Push webhooks
**Files:** `crates/loams-git/src/webhooks.rs` (a `_git` stream consumer through links and notifications, §09), `crates/loams-git/tests/webhooks.rs`. **Semantics:** per-repository webhook subscriptions, HMAC-signed GitHub-compatible `push` payloads (ref, before, after, commits, pusher; env keys never values), retries with backoff, and a dead-letter stream. Tests: `push_webhook_delivered_signed`; `webhook_retries_then_dead_letters`; `webhook_payload_has_no_env_values`. Commit `git: deliver push webhooks`.

### Task 85: macOS
**Files:** `crates/loams-vfs/src/nfs/` (an NFSv3 loopback server, Q753's default), later `crates/loams-vfs/src/fskit/`. **Semantics:** the same `VfsFs` core behind an NFSv3 server on a loopback port with a per-mount token; `mount_nfs` with `nolocks,locallocks`; FSKit when Q753 is answered. Tests: `nfs_mount_read_write_save_macos` (macOS runner); `status_matches_linux_suite`. Commit `vfs: serve workspaces on macOS over NFSv3 loopback`.

### Task 86: Windows
**Files:** `crates/loams-vfs/src/projfs/`. **Semantics:** ProjFS placeholders backed by the same core; the shim built for Windows with `git.exe` discovery. Tests: `projfs_placeholder_hydrates_on_read`; `shim_windows_passthrough`. Commit `vfs: serve workspaces on Windows with ProjFS`.

### Task 87: Kubernetes packaging and env delivery to Kubernetes
**Files:** `deploy/charts/loams/` (gateway and worker roles with Git enabled, if no chart exists by then; otherwise values in the existing chart), `crates/loams-vault/src/eso.rs` (an External Secrets Operator provider: an HTTP endpoint that ESO's webhook provider calls, authenticated per environment), `docs/guides/env.md` (Kubernetes section). **Semantics:** a promotion bumps an annotation on configured Deployments for a rolling restart. Tests: `chart_installs_on_kind`; `eso_sync_reflects_promotion`; `rollout_annotation_bumped_on_promotion`. Commit `deploy: package Loams Git for Kubernetes and sync env to Kubernetes Secrets`.

---

## Exit criteria for production (with the owning tasks)

- [ ] **WAL core:** formats golden, fencing and idempotency proven, linearizable under faults, the core gate met: Tasks 1–11.
- [ ] **Repositories as a service:** API, catalog, ownership, events and auth: Tasks 12–16.
- [ ] **Lazy object service and history:** batched reads, sizes without blobs, log, blame, grep and diff stats equal to git: Tasks 17–21.
- [ ] **Workspaces:** O(1) create, fenced snapshot chains, mount-less editing, push without re-upload: Tasks 22–27.
- [ ] **`loams-vfs`:** lazy reads, writes, saves, any-machine mounts, takeover and salvage, POSIX subset and real workloads: Tasks 28–35.
- [ ] **The shim:** passthrough within budget, native commands equal to git on the agent corpus, fallthrough round-trips: Tasks 36–45.
- [ ] **Interop:** the client matrix and the differential suite green, hidden workspace refs: Tasks 46–52.
- [ ] **Compaction, GC and integrity:** no object lost across forks, workspaces and in-flight pushes; scrubs clean: Tasks 53–55.
- [ ] **Agent scale:** sandboxes, agentd integration, and the million-workspace and thousand-agent benchmarks within the request model: Tasks 56–58.
- [ ] **Env:** no plaintext stored, sealing everywhere, redaction and audit, key-level review, promotion exactly once, rotation without commits, leak scanning, plain-git interop: Tasks 59–67.
- [ ] **Build cache and mirror:** Tasks 68–73.
- [ ] **Operations:** observability, quotas, the failure table, simulation and chaos, the restore drill twice green, security review closed, the performance gate, single node in one command, CLI, docs: Tasks 74–82.
- [ ] **Licences:** no GPL crate, no libgit2, no copied Sapling, EdenFS or git code (review checklist): Task 78.

## Self-review

- **Spec coverage.**

  | §36 section | Task(s) |
  |---|---|
  | §4 WAL core | 4–11, 14 |
  | §5 Traits | 6, 7, 9, 10, 17 |
  | §6 Access paths | 46–52 (§6.4 replaced by §19) |
  | §7 Compaction and GC | 11, 53–55 |
  | §8–§9 Cache and mirror | 68–73 |
  | §10–§11 Security and hooks | 16, 74, 78 |
  | §17 `Fs` and providers | 2, 70 |
  | §19.4 Workspaces | 22–25, 33, 54 |
  | §19.5 Objects | 17–19, 24, 29, 32, 34 |
  | §19.6 `loams-vfs` | 28–35, 56 |
  | §19.7 The shim | 36–45 |
  | §19.8 History | 20, 21, 42 |
  | §19.9 Interop | 46, 47, 51 |
  | §19.10 API | 12, 18, 25–27, 61 |
  | §19.11 Agent scale | 56–58 |
  | §19.12 GA and after | 79–82, 83–87 |
  | §19.13 Env | 59–67 (65 for §19.13.5; 87 for Kubernetes delivery) |

- **Types.** `WorkspaceStore`, `ObjectSource`, `GitGrant`, the control protocol, the API and the limits are defined once, in the shared contracts.
- **Review Focus.** Items 1–14 each name owning tests.
- **Carried from the superseded plans and the morning version.** Every test name is kept, except `first_push_creates_repo` (now `first_push_creates_repo_with_flag`, Ruling 7). The morning version's Tasks 12–13 (helper and gate) moved into Tasks 11 and 51, Tasks 19–27 became 46–52, Tasks 28–30 became 53–55, Tasks 31–36 became 68–73, and Tasks 37–45 became 74–82.

## Open questions

Plan-local numbers, with the design's numbers where they exist.

**Resolved on 2026-10-10** (coordinator's brief and defaults): GT-Q1 (start now; D825), GT-Q2 (Ruling 20), GT-Q3 (Ruling 21), GT-Q4 (LFS unnecessary; Task 83 after GA), GT-Q5 (webhooks after GA; Task 84), GT-Q6 (loopback and local-first GA before MT1; Ruling 30), GT-Q7 (Kubernetes later; Task 87), GT-Q9, GT-Q10, GT-Q11 and GT-Q12 (Rulings 29 and 50).

| # | Question | Default if unanswered | Owner | Needed by |
|---|---|---|---|---|
| GT-Q8 | **Credentials** for R2, S3 Standard and S3 Express One Zone (Q384's measurement, Task 80's R2 figure), and for live runs of the AWS, Azure and GCP vault adapters | RustFS, OpenBao and recorded fixtures only; cloud figures recorded "not measured" | Founder | Tasks 11, 60, 80 |
| GT-Q13 (Q750) | Workspace visibility: repository readers, or owner and grants only? | Repository readers read; owner writes | Founder | Task 25 |
| GT-Q14 (Q751) | The native command list at GA (merge, rebase, cherry-pick and stash fall through) | §36 §19.7.2, as refined by Task 0's corpus | Founder | Task 36 |
| GT-Q15 (Q752) | Install the shim as `git` by default (opt-out), or opt-in? | Default with a prompt; opt-out | Founder | Task 36 |
| GT-Q16 (Q753) | macOS: NFSv3 loopback or FSKit first? | NFSv3 | Eng | Task 85 |
| GT-Q17 (Q754) | Save cadence and snapshot retention | 2 s / 1 s; 7 d all, 30 d hourly, then daily | Eng | Task 32 |
| GT-Q18 (Q755) | Performance-gate fixtures | Synthetic 10M-file tree and Linux history; Chromium-sized nightly | Eng + Founder | Tasks 58, 80 |
| GT-Q19 (Q756) | Ignored build outputs never cross hosts | Confirmed; the build cache is the cross-host path | Founder | Task 31 |
| GT-Q20 (Q757) | Env: references only, or ciphertext envelopes in git? | References only | Founder | Task 61 |
| GT-Q21 (Q758) | Which files are env files | `env/**/*.env`; `.env*` elsewhere scanned as leaks | Founder | Task 61 |
| GT-Q22 (Q759) | Rotation providers at GA | random, Loams Postgres, Loams SQL, external, webhook; cloud-database IAM later | Founder | Task 66 |
| GT-Q23 (Q760) | Agents writing env values they cannot read | Allowed with `Write`, audited | Founder | Task 62 |

## Rulings made during execution

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| — | (Task 0 fills this table; later tasks append) | | |
