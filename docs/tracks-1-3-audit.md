# Tracks 1–3 Verification Audit (2026-10-07)

A verification of every issue in Tracks 1, 2 and 3 of
[`open-issues-analysis.md`](open-issues-analysis.md) against what is **actually
on `dev`** and what is **actually on `origin`**, rather than against the status
lines that document carries.

## Why this document exists

`docs/open-issues-analysis.md` is a point-in-time snapshot. Two of its status
lines are now wrong, and its branch inventory overstates the work remaining.
This audit records what was checked, how, and what the answer was.

**The method matters more than the conclusions.** Auditing by *commit hash* gives
a different — and wrong — answer from auditing by *content*. `git cherry` reports
R1's Task 12 commits as absent from `dev`; they are present, renamed from
`operon:` to `loams:` during the crate rename, so the patch-id no longer matches
and `git cherry` reports `+`. The deliverable check below is by file content.

Reproduce any row with:

```
git cat-file -e dev:<path>                      # content check
git merge-base --is-ancestor origin/<b> dev     # merge check
git cherry dev origin/<branch>                  # commit-level check (unreliable here)
```

## Corrections to the source document

| Issue | Document says | Verified state |
|---|---|---|
| **#207 RT1** | "Starter branch `origin/rt1-t0` exists; RT0 foundations landed" | **Fully merged.** `origin/rt1-t0` is an ancestor of `dev`; `crates/loams-sim` and `crates/loams-sqlrouter` are present. The work is done. |
| **#210 FL2** | "Branches `origin/fl2-t0-chdb-spike` and `origin/fl2-t3-errors` exist" | **Both merged.** Both are ancestors of `dev`; `deploy/tikv/tidb.toml` is present. Nothing to integrate. |
| **#201 R1** | "Tasks 11–17 already implemented on feature branches" | **Misleading.** Tasks 12, 13, 15 and 17 are already merged into `dev`. Only **Task 14** (22 files) and **Task 16** (3 files) remain unmerged. See below. |

## Track 1 — SDKs & Unified Connect API

| Issue | State | Evidence |
|---|---|---|
| #283, #285, #286, #292, #294, #296 | **Complete** | 13/13 SDK languages pass their conformance gates |
| #281 API1 | **4 of 11 tasks** | Tasks 0–3 done. Task 2 (`a5efc8d1`, `loams.collection.v1`, 11 RPCs); Task 3 (`d21261d8`, `loams.document.v1`, 6 RPCs). Tasks 4–10 remain |
| #282 SDK1 | **0 of 9 tasks** | `docs/plans/2026-10-02-sdk1-generation-pipeline.md`: all 9 tasks unchecked |

### API1 Tasks 2–3 as built

Both new proto packages generate through `loams-proto` and register in
`connect::routes()`. Handlers are thin: Task 3's convert each request back into
native REST JSON and call the REST route's own functions (`op_from_json`,
`read_consistency`, `backpressure_of`), so a rejected op is refused by one code
path with one message on both surfaces and the two cannot drift.

Neither package carries `loams.options.v1` `FacadeOptions` (ruling 2.5): a
`facade` option makes the generator emit calls that import stubs no SDK mirror
generates. Confirmed empirically — adding `loams.collection.v1` to the Go SDK's
hand-written `ProtoPackages` produced 9 conformance failures, because
`sdks/go/gen/loams/` has no stubs for either package.

**Unratified decision.** `WriteDocuments`' `idempotency_key` has no specification
behind it: there is no dedupe ledger anywhere in `loams-query`, and the REST
write carries no key, so "dedupe window equals the REST one" had nothing to
compare against. Task 3 shipped a per-process ledger (5-minute window, 1024
entries, canonical request fingerprint, per-key lock) and flagged it as invented.
It needs a ruling before Task 9 depends on it.

API1 and SDK1 are **strictly serial**: each task layers its proto package onto
`crates/loams-proto/build.rs`'s `FILES` and registers a service in
`connect::routes()`. They cannot be parallelised without colliding in those
files. SDK1's own plan header states it "Depends on API1 Tasks 1–2".

## Track 2 — Tooling, Upstream & Operations

| Issue | State | Note |
|---|---|---|
| #375 | **Complete** | `089bcaba` |
| #148 loam-wal TLS | **Actionable** | Self-contained; not yet started |
| #253 SignPath | Partial | Blocked on release packaging pipelines |
| #237 backlog tracking | Process issue | Not code |
| #314 Cloudflare secrets | **Blocked on owner** | Requires production credentials only the repository owner can set. Not agent-actionable. |
| #118 WeSQL upstream sync | **Blocked** | Requires push access to `ostrium-labs/wesql`. Not agent-actionable. |

## Track 3 — Core Storage, Metastore, Query & Routing

### #201 R1 — TiKV Metastore & Reactive Core

Audited by content, Task by Task:

| Task | Deliverable | On `dev`? |
|---|---|---|
| 12 Sessions + sync service | `crates/loams-live/src/session.rs`, live server wiring | **Yes** |
| 13 QuickJS + `Deploy` | `crates/loams-live/src/deploy.rs`, `crates/loams-quickwit/` | **Yes** |
| 14 TypeScript reactive client | `sdks/live-typescript/` (22 files) | **No** — branch only |
| 15 TiDB SQL playground | `deploy/tikv/tidb.toml` | **Yes** |
| 16 Gates + exit report | `reactive_checker.rs`, `txn_checker.rs`, `r1-exit-report.md` | **No** — branch only |
| 17 Documentation | `docs/design/20-reactive-database-on-tikv.md` | **Yes** |

**The branch inventory in the source document is wrong.** `r1-t16` and `r1-t17`
are **byte-identical** to `r1-t14` (0 files differ); they are stale pointers left
by GitHub PR merges, not separate work. `r1-t15` has no branch at all, because
Task 15 is already merged. So the "remaining branch stack" is **one** branch,
`origin/r1-t14` — not six.

`d1-t8` is likewise byte-identical to `d1-t7`.

### #202 D1 — Embedded Durable Execution

`origin/d1-t9`'s tip commit is titled **`wip: D1 Task 9, in progress`**. This is
unfinished work, not a completed task. Merging it as-is would land a WIP commit
as though it were Task 9. It needs finishing first.

### Remaining Track 3 issues

| Issue | State |
|---|---|
| #208 RT2 | Blocked on RT1 — but RT1 is **merged**, so this dependency is now satisfiable and the blocker is stale |
| #209 FL1 | **Not started.** Greenfield: Iggy/Fluss engines + CloudEvents envelopes |
| #274 FL3 | Design complete (§42 §6). Implementation not started |
| #200 PG1 | Spike merged; implementation pending |
| #198 M1.6 | MCP server tools remain to be unified |
| #199 M1.7 | Awaits remaining M1 tasks |

## The rename problem (affects R1, D1, and any old Track 4 branch)

`origin/r1-t14` carries **23 `operon-*` crates**; `dev` carries **zero**. The
branch predates the Operon → Loams rename.

A real merge of `origin/r1-t14` into `dev` produces **31 conflicts, 16 of them
Rust source**. All 8 rename-specific conflicts are mechanical (apply the
`operon-live/*` changes onto their `loams-live/*` counterparts). The genuinely
hard ones are four files:

```
16 conflict regions  crates/loams/src/server.rs
13 conflict regions  crates/loams-live/src/service.rs
10 conflict regions  crates/loams-live/src/session.rs
 7 conflict regions  crates/loams-live/tests/service.rs
```

**Sequencing constraint:** `crates/loams/src/server.rs` is both the single worst
conflict *and* the file API1's Connect server wiring runs through. The R1 merge
must therefore happen **after** API1, or it will have to be re-done.

> **Method note.** An earlier `git merge-tree` simulation reported "10
> conflicts, 0 Rust". That was wrong: the legacy `git merge-tree` form does not
> do rename detection, so it never paired the deleted `operon-live/*` paths with
> their renamed `loams-live/*` targets. The real merge was run in a scratch
> worktree to get the number above.

## Blockers requiring the owner

These cannot be completed by an agent and are the honest reason "finish
everything" has a ceiling:

1. **#314** — Cloudflare production secrets for `console.loams.dev`.
2. **#118** — WeSQL upstream sync needs push access to `ostrium-labs/wesql`.
3. **#201 Task 16** — the R1 exit gates must actually pass, which needs TiKV
   running; the checkers cannot be merged on faith.
4. **#202 Task 9** — the branch tip is WIP and must be completed before merge.
