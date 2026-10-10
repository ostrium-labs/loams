# 51 — Loams Postgres: Our Hard Fork of Neon — Mirror, Build, Maintain, Extend

Status: **Proposed** · 2026-10-09, **amended the same day for the hard fork** (§1a). Sources, both binding: the owner's decision **"Mirror and fully fork neon postgres all repos we will maintain it add extension new pg 19 support etc."**, and the owner's later decision **"let the neon fork be hard fork, neon stopped open source after databricks acquisition and went silent over a repo, replace with our loams, let it be called loams-postgres not loams-neon."** Decisions **D800–D824** and questions **Q715–Q734** are recorded in the [decision log](13-decision-log.md). Plan: [NF1](../plans/2026-10-09-nf1-neon-fork.md).

This file was `51-neon-fork.md` until the amendment.

This is an addendum to [§28](28-loams-postgres.md) §10 (maintaining the fork) and to [§46](46-loams-postgres-production.md) (Loams Postgres in production, plan PG2). It keeps D231 (Loams Postgres is a fork of Neon, and Loams owns releases, images and Postgres patch rebases) and turns it from a direction into an operating model. It **supersedes D241's cadence** with D811–D814, **answers Q649** (D813), **answers Q651** in part (D812), and **amends D716's upgrade clause** (D815). It does not change D714 (`loams-wal` is the only WAL) or D708 (PgDog is the front door, unmodified).

**The hard fork (D820–D824) changes this document in place.** Where a section said that `main` mirrors upstream, that `loams/main` is the default branch, or that `neondatabase/neon` is merged weekly, it now says what §1a decides. The original wording of D802, D811 and D819 survives only in the decision log, marked superseded.

Markers: **(verify)** means not checked against a primary source; the plan task that depends on the fact checks it first. **(estimate)** means computed or judged, not measured. **(target)** is a number this document sets as a gate.

---

## 1. Summary

| # | Decision | Status |
|---|---|---|
| D800 | **Ostrium Labs owns the Neon fork and maintains it as a product.** "Owns" means: our images, our releases, our Postgres patches per major, our extension catalogue, our security response, and no dependence on Neon Inc. publishing anything. What is maintained, kept frozen, or dropped is listed per component (§2). *Since the amendment, a **hard fork** named Loams Postgres (D820)* | Approved direction (owner, 2026-10-09); scope Proposed; amended by D820 |
| D801 | **The fork inventory** is eight repositories under `ostrium-labs`: `loams-postgres` (was `neon`, renamed 2026-10-09), `postgres`, `rust-postgres`, `azure-sdk-for-rust`, `framed-websockets`, `tokio-epoll-uring`, `pg_session_jwt` and `autoscaling`. Each has a recorded licence, origin, pin and status (maintained, frozen or archived) (§3.1). Four upstream repositories are private and cannot be forked: `subzero`, `flux-fleet`, `cloud` and `infra` (§3.2) | Proposed; amended by D822 (the rename) |
| D802 | ~~**Branches:** `main` is a fast-forward-only mirror of upstream; `loams/main` (the default branch) holds our patches~~ *Superseded by D821:* `main` is ours and the default branch; topic work is on `loams/<topic>`. History is **merged, never rebased**, so every revision Loams pins stays reachable. Every pinned revision also carries a `loams-*` tag. Nothing is ever force-pushed. The Postgres fork uses `loams/REL_<N>_STABLE` per major (§3.3) | Superseded in part by D821 |
| D803 | **Patch queue:** every Loams commit carries a `Loams-Patch: LP-<nnnn>` trailer and a row in `LOAMS_PATCHES.md` (purpose, area, owner, drop condition). CI fails an unlisted patch. Since the hard fork, the queue is the Apache-2.0 statement of changes against the fork point `neon-fork-point` (`fa504217`), and in the Postgres fork it still tracks what differs from upstream Postgres. It is reviewed every quarter (§3.4) | Proposed; amended by D821 |
| D804 | **Governance and licence compliance:** CODEOWNERS with `ostrium-labs` teams and two required maintainers; DCO on every fork; protected `main` and `loams/*` branches; a GitHub App bot (`loams-postgres-bot`) for cross-repository pushes. Apache-2.0 §4 notices ("Based on Neon by Databricks/Neon Inc.") and a statement of changes; the PostgreSQL licence kept in every `vendor/` tree; corresponding source published for every GPL component in an image; **no CDDL in any binary we ship** (the owner's rule of 2026-10-08); a README stating independence from Neon Inc. and Databricks (§3.5, §3.6) | Proposed; amended by D820, D822 |
| D805 | **Security:** private vulnerability reporting and `SECURITY.md` on every fork, pointing at Loams' policy; a daily advisory job (RUSTSEC on the fork's lockfile, image CVE scans, the PostgreSQL security feed, GitHub advisories on the upstreams); response targets of 72 h for critical, 7 days for high and for Postgres security minors, 30 days otherwise (§3.7) | Proposed (Q724) |
| D806 | **The one-time mirror:** `skopeo copy --all --preserve-digests` (run through podman) copies the images Loams pins today, Neon's `neon` and `compute-node-v17` from its build `77e22e4b`, plus `compute-node-v16` of the same build for exports, to `ghcr.io/ostrium-labs/neon-archive/` with **every digest unchanged**. They keep Neon's names because they are Neon's binaries, not Loams Postgres. A licence audit of the mirrored binaries decides whether the packages are public; compose defaults move to the mirror for each package that is public (§4) | Proposed (Q715, answered: private until audited); amended by D822 (destination) |
| D807 | **Our own builds:** GitHub Actions in `ostrium-labs/loams-postgres` build multi-arch (amd64, arm64) `loams-postgres-storage`, `loams-postgres-compute-v<N>` and `loams-postgres-build-tools` images and publish them to `ghcr.io/ostrium-labs`. Builds run on hosted runners by default, selected through the same `vars.RUNNER_*` pattern Loams uses, with a registry layer cache and stage-split compute builds to fit the 6 h job limit (§5) | Proposed (Q721); amended by D822 (names) |
| D808 | **Build hygiene:** upstream's workflows are removed from `main` once (no sync brings them back); `pprof` and `jemalloc_pprof` are built without `flamegraph`, so `inferno` (CDDL-1.0) leaves every binary, and `deny.toml` drops CDDL-1.0; `subzero-core` stays a stub and `rest_broker` is off; every git dependency points at an `ostrium-labs` fork by `rev`; **no build input is fetched from `neondatabase/*`** (§5.2, §10) | Proposed; amended by D821 |
| D809 | **Supply chain:** each image is published by digest with SLSA provenance (`actions/attest-build-provenance`), an SPDX SBOM, and a keyless cosign signature from the release workflow identity of `ostrium-labs/loams-postgres` on `main`. Loams CI runs `cosign verify` on every pinned digest. Tags are tied to commits; bit-for-bit reproducibility is not promised at NF1 (§5.3, §5.4) | Proposed (Q726); amended by D822 (identity) |
| D810 | **Release cadence:** fork releases are `lp-YYYY.MM.N` (was `nf-`). The `loams-postgres-storage` image follows `main` and is released when the gate is green, at most weekly; compute images are released on every Postgres minor (within 7 days, the D805 target), on extension security updates, and in a monthly roll-up. Loams moves its pins by PR (§5.6) | Proposed; amended by D822 (names) |
| D811 | ~~**Upstream sync:** weekly for `neondatabase/neon`; monthly for the small forks~~ *Superseded by D820 and D821:* no sync from `neondatabase/*`. What stays: the nightly try-merge of `postgres/postgres` `REL_<N>_STABLE` into our Postgres branches, and a four-tier test gate on every PR (§6) | Proposed; supersedes D241's cadence; **superseded in part by D820, D821** |
| D812 | **The extension catalogue** is a manifest, `compute/loams-extensions.toml`, with a tier (`core`, `included`, `dropped`), a licence (SPDX) checked against an allowlist, the source with its sha256, and the majors it builds for. Core extensions block a release; included ones may lag a new major. Loams' own extensions live in `pgxn/loams*`. Updates are proposed monthly by a bot, respecting the 14-day age rule (§7) | Proposed (Q719); answers Q651 in part |
| D813 | **Postgres majors:** 17 is supported; 18 is supported once NF1e's gate passes; **19 is beta** until NF1f's gate passes, which cannot be before upstream's GA on 2026-10-29 (target date, verify); 16 is maintenance-only (minors, export, no new projects); 14 and 15 are removed from our build. **Answers Q649** (§8.1) | Proposed (Q717, Q718); amends D241 |
| D814 | **Porting a major** is a fixed checklist across eleven components, from the Postgres patch series through `postgres_ffi`, `wal_decoder`, the pageserver, `pgxn/neon`, `compute_ctl` and both images, to `loams-wal-decoder`, `loams-safekeeper`, `loams-postgres` and PgDog. It starts with a written WAL and on-disk format diff, and ends with a fixed test matrix (§8.2–§8.4) | Proposed |
| D815 | **Major upgrades of Neon branches:** first (NF1) through Neon's own `fast_import pgdata` into a new timeline of the same tenant, followed by an endpoint swap; then (spike, Q725) `pg_upgrade --link` on a materialised data directory, imported the same way. Dump and restore (D716) stays the fallback. Data is never upgraded without the user's action (§9) | Proposed; amends D716, answers Q648 in part |
| D816 | **Private dependencies are dropped, not replaced, in NF1:** the proxy's REST broker (`subzero`) is off and its CI steps go; `flux-fleet` is only cited in comments; the `cloud` and `infra` CI triggers go; Neon's `dev-actions`, its registry cache and `neondatabase/*` downloads are replaced. A Data API, if wanted, is a later separate service (§10) | Proposed (Q720) |
| D817 | **`tokio-epoll-uring`** declares `MIT OR Apache-2.0` in its `Cargo.toml` (its vendored `uring-common` declares MIT, from `tokio-uring`) but has no licence file, so the brief's "no licence at all" is corrected (§11). Our fork adds the licence texts as declared, with `tokio-uring`'s notice; with the owner's consent one upstream issue asks Neon to add them; a pageserver build without it is the fallback if counsel finds the declaration insufficient | Proposed (Q716) |
| D818 | **Integration with Loams:** one pin file, `deploy/pins/loams-postgres-images.toml`, is the only place digests and supported majors are written, and every consumer is checked against it; the decoder's git dependency follows a `rev` on `main` that carries a `loams-*` tag; the desktop runs one compute per timeline major and never attaches a compute of another major; PgDog is gated per major by the router inventories (§12) | Proposed (Q728); amended by D822, D823 (names) |
| D819 | ~~**Ownership is costed:** one-time work of 20–34 engineer-weeks and an ongoing 0.6–0.9 FTE in full mode; MVM about 0.2 FTE~~ *Re-estimated by D824 for the hard fork.* Two named maintainers are required, and fixed triggers move between the modes (§13) | Proposed (Q722); **numbers superseded by D824** |
| D820 | **Hard fork.** `ostrium-labs/neon` is renamed `ostrium-labs/loams-postgres` (done 2026-10-09; GitHub redirects the old URL) and no longer follows `neondatabase/neon`: no mirror branch, no weekly merge, no upstream-sync milestone. We own `main`. History is kept, and `NOTICE` keeps the Apache-2.0 attribution, "Based on Neon by Databricks/Neon Inc." An upstream commit, if one ever appears, enters only as a hand-picked patch with its own `Loams-Patch` id. Security and bug fixes are ours alone. Detaching the repository from GitHub's fork network is a support request, so it is an owner action (Q730) (§1a) | Approved direction (owner, 2026-10-09); details Proposed |
| D821 | **Branches and tracking after the hard fork.** `loams-postgres`: `main` is the default branch and trunk, PRs only; `loams/decoder-trim` (`1218fb7a`, one commit on `fa504217`) is fast-forwarded into it; `fa504217` is tagged `neon-fork-point`. The Postgres fork tracks `postgres/postgres` minors directly on `loams/REL_<N>_STABLE`; the `REL_<N>_STABLE_neon` branches are frozen as history. The small forks are frozen and take fixes from their original upstreams only on an advisory. Merge-never-rebase, no force-push and `loams-*` tags stay (§1a, §3.3, §6) | Proposed; supersedes D802's mirror and D811's Neon sync |
| D822 | **Rebrand to Loams Postgres where it is ours.** Now: the repository `ostrium-labs/loams-postgres`; the images `ghcr.io/ostrium-labs/loams-postgres-storage`, `loams-postgres-compute-v<N>` and `loams-postgres-build-tools`; release tags `lp-YYYY.MM.N`; patch ids `LP-<nnnn>`; the pin file `deploy/pins/loams-postgres-images.toml`; the bot `loams-postgres-bot` and the team `loams-postgres-maintainers`. Over time, by plan: binaries, docs and paths we own. **Compatibility names stay** until a migration task changes them with an alias and a test: wire protocols and HTTP APIs, on-disk formats (layer files, the `neon_rmgr` WAL records, control files), and SQL-visible names (`neon` and `neon_utils` extensions, `neon.*` GUCs, `neon_superuser`), plus `NEON_*` environment variables and binary names such as `neon_local`. Neon's own mirrored builds keep Neon's names (§1a). **Answers Q723** | Proposed; answers Q723; amends D801, D804, D806, D807, D809, D810, D818 |
| D823 | **The crate `loams-neon` becomes `loams-postgres`** (NF1 Task 1b), with its CI references, the decoder's git URL (`github.com/ostrium-labs/neon` → `github.com/ostrium-labs/loams-postgres`, same `rev`) and `deploy/neon` → `deploy/loams-postgres-dev` (Q731). The compose project name `loams-neon` and the desktop's copied `stacks/neon` directory stay, because they name users' Docker volumes and files. No name clash: no crate, package or path called `loams-postgres` exists in the workspace; `loams-pg-control`, `loams-pgdog-sync` and `deploy/loams-pg-bench` differ; the desktop's `@loams/plugin-postgres` and stack id `postgres` are TypeScript names; `loams.postgres.v1` generates `loams_proto::postgres`. `deploy/loams-postgres` is not used because `deploy/helm/loams-postgres` and `deploy/loams-postgres-single` already name the product's deployments (§12.6) | Proposed (Q731) |
| D824 | **Ownership re-costed for the hard fork:** one-time work of 21–35.5 engineer-weeks; ongoing **29–49 engineer-weeks a year, about 0.7–1.1 FTE** in full mode, staffed as one full-time maintainer plus a second at about 0.25 FTE (about 1.25 FTE), and MVM about **0.25–0.3 FTE**, because no upstream supplies security or bug fixes and the rebrand adds work; CI cost unchanged (§13). Supersedes D819's numbers; two named maintainers and the mode triggers stay | Proposed (Q722, Q734); supersedes D819's numbers |

## 1a. The hard fork, rebranded Loams Postgres (D820–D824)

**Why.** Neon stopped developing in the open after the Databricks acquisition: `neondatabase/neon` has had no commit since 2026-08-31 (`fa504217`), and before that only 11 in twelve months (§2). A soft fork that waits on upstream merges would wait for nothing. The owner therefore made it a hard fork with Loams' own name.

**What changes.**

| Area | Before (D802, D811) | After (D820, D821) |
|---|---|---|
| Repository | `ostrium-labs/neon`, a fork of `neondatabase/neon` | `ostrium-labs/loams-postgres` (renamed 2026-10-09; GitHub redirects `ostrium-labs/neon`). Still in Neon's fork network until GitHub support detaches it (Q730) |
| `main` | A fast-forward-only mirror of upstream | **Ours.** The default branch and trunk; PRs only |
| `loams/main` | The default branch | Not created. Its role is `main`'s |
| Upstream Neon | Merged weekly (Task 19, milestone NF1c's sync) | Not merged. A commit that appears upstream may be cherry-picked by hand as a patch with its own id and a `(cherry picked from neondatabase/neon <sha>)` line, under Apache-2.0 |
| Fork point | Implicit | Tag `neon-fork-point` on `fa504217` (Neon's last public commit), the base of the patch check and the statement of changes |
| Neon's Postgres branches | `REL_<N>_STABLE_neon` mirrored weekly from `neondatabase/postgres` | Frozen as history. `loams/REL_<N>_STABLE` takes `postgres/postgres` minors directly, nightly (§6) |
| Small forks | Monthly sync | Frozen at their pinned revisions. A fix comes from the original upstream (for example `sfackler/rust-postgres`, `tokio-rs/tokio-uring`) only when an advisory needs it |
| Security and bug fixes | Upstream might supply some | **Ours alone** (§3.7, §13) |
| Attribution | `NOTICE` keeps Neon's lines | Unchanged, plus the line "Based on Neon by Databricks/Neon Inc." and "Modifications Copyright 2026 Ostrium Labs and the Loams Authors" (§3.6) |

**What does not change.** The licence (Apache-2.0); merge-never-rebase and no force-push; `loams-*` tags on pinned revisions; the patch trailer (now `LP-<nnnn>`) as the statement of changes; the gate, the extension catalogue, the majors, the upgrade paths and the integration (§5–§12).

### 1a.1 Names: ours now, ours over time, and compatibility names (D822)

| Kind | Old | New | When |
|---|---|---|---|
| Repository | `ostrium-labs/neon` | `ostrium-labs/loams-postgres` | Done (2026-10-09) |
| Storage image (pageserver, storage controller, broker, safekeeper, proxy, tools) | `neon` | `ghcr.io/ostrium-labs/loams-postgres-storage` | First own build (NF1 Task 10) |
| Compute images | `compute-node-v<N>` | `ghcr.io/ostrium-labs/loams-postgres-compute-v<N>` | NF1 Task 11 |
| Build tools | `build-tools` | `ghcr.io/ostrium-labs/loams-postgres-build-tools` | NF1 Task 9 |
| Registry cache | — | `ghcr.io/ostrium-labs/loams-postgres-buildcache` | NF1 Task 10 |
| Neon's own mirrored builds | — | `ghcr.io/ostrium-labs/neon-archive/{neon,compute-node-v17,compute-node-v16,build-tools}`, Neon's names kept, private (verify that GHCR accepts the nested path; otherwise `neon-archive-<name>`) | NF1 Task 1 |
| Release tags | `nf-YYYY.MM.N` | `lp-YYYY.MM.N` | No release exists yet |
| Patch ids | `NF-<nnnn>` | `LP-<nnnn>` | No patch row exists yet |
| Pin file | `deploy/pins/neon-images.toml` | `deploy/pins/loams-postgres-images.toml` | NF1 Task 2 |
| Loams crate | `loams-neon` | `loams-postgres` | NF1 Task 1b (D823) |
| Loams dev stack | `deploy/neon` | `deploy/loams-postgres-dev` (Q731) | NF1 Task 1b |
| Bot, team | `ostrium-labs-fork-bot`, `neon-maintainers` | `loams-postgres-bot`, `loams-postgres-maintainers` | When created (owner actions) |
| Docs in images | `/usr/share/doc/loams-neon/` | `/usr/share/doc/loams-postgres/` | NF1 Task 10 |
| Fork docs, runbooks | `docs/runbooks/neon-fork/` | `docs/runbooks/loams-postgres-fork/` | NF1 Task 40 |
| Binaries we own, e.g. `neon_local`, `storage_scrubber` banners, `--version` strings | Neon's | Loams Postgres', with the old name kept as an alias | Over time, one task per rename after NF1 (Q732) |
| **Compatibility names (kept)** | Wire protocols (page service, safekeeper and walproposer protocol, `compute_ctl`, pageserver and storage controller HTTP APIs); on-disk formats (layer files, index parts, the `neon_rmgr` WAL records, tenant and timeline paths, control files); SQL-visible names (`neon`, `neon_utils`, `neon_rmgr`, `neon_walredo` extensions; `neon.*` GUCs; the `neon_superuser` role); `NEON_*` environment variables; the compose project `loams-neon` and desktop `stacks/neon` | Unchanged | Only through a migration task that adds an alias, keeps reading the old name, and tests both (Q732) |

A user-visible name that is stored somewhere (a GUC in a saved compute spec, a role in a dump, a WAL rmgr id, a Docker volume) is never renamed in place. That is the rule the migration tasks follow.

### 1a.2 Owner actions (2026-10-09)

- **Detach the fork network** (Q730). GitHub only does this on a support request from an organisation owner. Until then the repository still offers `neondatabase/neon` as the default PR base, so every PR must target `ostrium-labs/loams-postgres` `main` explicitly.
- **Tokens and the bot are still blocking.** The `gh` token has the scopes `repo` and `workflow` only: no `write:packages` (the mirror, Task 1; every image push before the bot exists) and no `admin:org` (the teams, Task 5). The GitHub App `loams-postgres-bot` is created by the owner later, or by the controller through GitHub's App manifest flow once the owner approves (Q733).

## 2. What "owning the fork" means (D800)

Upstream is frozen, which is why the fork became a hard fork (§1a). `neondatabase/neon` has had 11 commits between 2025-08-25 (`77e22e4b`, the build of Neon's last public images) and 2026-08-31 (`fa504217c`, the fork point), and none since. Neon's staff said in 2025 that engineering moved to Databricks' private infrastructure (§28 §10). The public images Loams pins are 13 months old, carry Postgres 17.5 (upstream is at 17.11, at least 39 CVEs later), and will never be rebuilt. Owning the fork therefore means doing everything Neon's release engineering did, for the parts Loams ships.

| Component | Where | Fate | Why |
|---|---|---|---|
| Pageserver, storage controller, storage broker, `pagectl`, `storage_scrubber`, `endpoint_storage` | `ostrium-labs/loams-postgres` | **Maintained** | Loams Postgres' storage plane (§46 §9.9) |
| Safekeeper | `ostrium-labs/loams-postgres` | **Frozen**: built, never deployed | D714: only the `sk` benchmark reference profile and the dev data migration use it (Q654) |
| `compute_ctl`, `fast_import`, `local_proxy`, `pgxn/neon` (smgr, LFC, walproposer, communicator), `neon_rmgr`, `neon_walredo`, `neon_utils` | `ostrium-labs/loams-postgres` | **Maintained** | Every compute; the walproposer talks to `loams-wal` |
| Postgres with Neon's patches, per major | `ostrium-labs/postgres` | **Maintained** for supported majors (§8.1) | The compute and the pageserver's WAL redo |
| `wal_decoder`, `postgres_ffi`, `pageserver_api`, `utils` | `ostrium-labs/loams-postgres` | **Maintained** | Also linked by `crates/loams-wal-decoder` (PG2 Task 31) |
| Proxy | `ostrium-labs/loams-postgres` | **Frozen**, REST broker removed | D709's last fallback for wake-on-connect; PgDog is the front door |
| `neon_local`, `test_runner` | `ostrium-labs/loams-postgres` | **Maintained as test tools** | The fork's gate (§6.4) |
| `vm_monitor`, NeonVM images, `autoscaling` | `ostrium-labs/loams-postgres`, `ostrium-labs/autoscaling` | **Frozen**, not built | D712 uses in-place Pod resize, not NeonVM (Q644) |
| Compute extensions | `compute/compute-node.Dockerfile` and the manifest | **Maintained** by catalogue tier (§7) | The extension allow-list (Q651) |
| Cloud integrations: deploy triggers, `cache.neon.build`, Neon's metrics fleet | upstream CI | **Dropped** | Private, and Loams has its own (§10) |

## 3. Fork inventory and governance (D801–D805)

### 3.1 Inventory

Checked on 2026-10-09 with `gh api repos/ostrium-labs/<repo>` and the Neon tree at `1218fb7a`.

| Fork | Upstream | Licence (as found) | Used by | Loams pin | Status |
|---|---|---|---|---|---|
| `loams-postgres` (was `neon`) | `neondatabase/neon` (origin only; **hard fork** at `fa504217`, D820) | Apache-2.0 (`LICENSE`, `NOTICE`: "Neon, Copyright 2022 - 2024 Neon Inc.") | Everything | `crates/loams-wal-decoder`: rev `1218fb7a` (branch `loams/decoder-trim`, tag `loams-decoder-trim-1`); images: Neon's `77e22e4b` build | Maintained. Renamed 2026-10-09. Default branch `main`; Actions are allowed but no workflow is enabled (0 active) |
| `postgres` | `neondatabase/postgres` (Neon's patches, frozen as history); `postgres/postgres` (minors, tracked directly) | PostgreSQL licence (`COPYRIGHT`); GitHub shows none | The `vendor/postgres-v<N>` submodules (relative URL `../postgres.git`) | Neon `main`'s submodule revs: 17.5 (`1e01fcea`), 16.9 (`a42351fc`) | Maintained. Has `REL_14…17_STABLE_neon` and `REL_18_STABLE_neon` (`a616eefe`, 2026-04-08, 18.2) |
| `rust-postgres` | `neondatabase/rust-postgres` (branch `neon`), itself from `sfackler/rust-postgres` | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) | The fork's `postgres`, `tokio-postgres` and friends; the decoder | rev `f3cf448f` | Frozen unless an advisory needs a fix (D821) |
| `azure-sdk-for-rust` | `neondatabase/azure-sdk-for-rust` (branch `neon`) | MIT | `remote_storage`'s Azure backend | none in Loams (the decoder trim removed it) | Frozen |
| `framed-websockets` | `neondatabase/framed-websockets` | Apache-2.0 | The proxy's WebSocket path | none | Frozen |
| `tokio-epoll-uring` | `neondatabase/tokio-epoll-uring` | **No licence file**; `Cargo.toml` declares `MIT OR Apache-2.0`, `uring-common` declares `MIT` (§11) | The pageserver's I/O engine | none (the decoder does not link it) | Maintained (licence work, D817) |
| `pg_session_jwt` | `neondatabase/pg_session_jwt` | Apache-2.0 | A compute extension (pgrx) | none | Maintained (catalogue tier `included`) |
| `autoscaling` | `neondatabase/autoscaling` | Apache-2.0 | NeonVM, the autoscaler agent | none | Archived (D712) |

Not forked, but fetched by upstream's build: `neondatabase/pgrag` (Apache-2.0, `LICENSE` reads "Copyright (C) 2024 Neon Inc."), `neondatabase/lcov` (GPL-2.0, coverage in `build-tools`), `neondatabase/dev-actions`, `neondatabase/gh-workflow-stats-action` and `neondatabase/proxy-bench`. D808 removes every such fetch: an input is either forked under `ostrium-labs` or dropped.

### 3.2 Private upstreams that cannot be forked

`gh api` answers 404 for each, which means private or deleted:

| Repository | What references it | What NF1 does |
|---|---|---|
| `neondatabase/subzero` | `proxy`'s `rest_broker` feature (`subzero-core`, the PostgREST-compatible Data API); `Dockerfile` lines 66–94 (`SUBZERO_ACCESS_TOKEN` secret, `cargo add` of the real crate); `.github/actions/prepare-for-subzero` and three workflows; `proxy/README.md` | Keep the in-tree stub `libs/proxy/subzero_core`; build without `rest_broker`; delete the secret steps and the action (§10) |
| `neondatabase/flux-fleet` | Two comments in `compute/vm-image-spec-*.yaml` and one in `pageserver/src/metrics.rs` | Nothing to build; the comments stay or go with the VM specs, which are not built |
| `neondatabase/cloud` | `build_and_test.yml` and `trigger-e2e-tests.yml` (`gh workflow --repo neondatabase/cloud`) | Removed with upstream's workflows |
| `neondatabase/infra` | `build_and_test.yml` deploy steps | Removed with upstream's workflows |

**A finding that matters for the mirror (verify in NF1 Task 1).** Neon's CI passes `SUBZERO_ACCESS_TOKEN` to its `neon` image build, which then swaps the stub for the private crate and adds `rest_broker` to the features. `subzero` was integrated in July 2025 (`0dbe5518`, `a70a5bcc`), before `77e22e4b`. So the `proxy` binary inside the image Loams pins was most likely built with code from a private repository whose licence is unknown. The images are built with `cargo auditable`, so the audit can read the dependency list out of the binary and settle it.

### 3.3 Branches and tags (D802, amended by D821)

**`ostrium-labs/loams-postgres`:**

| Ref | Meaning | Who writes it |
|---|---|---|
| `main` | **The default branch and trunk.** Neon's history to `fa504217`, then ours. Releases are cut from it | PRs only, behind the gate (§6.4) |
| `neon-fork-point` (tag) | `fa504217`, Neon's last public commit: the base of the patch check and the statement of changes | Task 3, once |
| `loams/<topic>` | Work in progress, e.g. `loams/pg18`, `loams/nf1-hygiene` | Maintainers |
| `loams/decoder-trim` | Historical (PG2 R31.12). Fast-forwarded into `main` (`1218fb7a` is one commit on `fa504217`); kept, never deleted | — |
| `loams-<YYYYMMDD>-<n>` and `lp-YYYY.MM.N` tags | Every revision Loams pins (Cargo `rev`, image source) has one, so no repository setting can garbage-collect it | The release workflow |

**`ostrium-labs/postgres`:** `REL_<N>_STABLE_neon` is frozen at Neon's last commit, as history; nothing mirrors it any more. `loams/REL_<N>_STABLE` is ours: Neon's patches, plus `postgres/postgres` minors merged in directly, plus our patches. `vendor/postgres-v<N>` in `main` points at a commit on it. Actions are disabled on this repository. Its CI runs from the `loams-postgres` repository, which checks it out (§6).

**The small forks** (`rust-postgres`, `azure-sdk-for-rust`, `framed-websockets`, `tokio-epoll-uring`, `pg_session_jwt`) are frozen at the branch Neon used (`neon` or `main`); nothing is synced. Our patches go on `loams/<that branch>`, and a fix from the original upstream comes in only when an advisory needs it. The `loams-postgres` repository refers to them by `rev`.

**Merge, never rebase.** A rebase would orphan revisions that `crates/loams-wal-decoder/Cargo.toml` and older releases pin, and Cargo fetches by revision. Neon itself merged Postgres minors into its `_neon` branches, and the Postgres fork keeps doing so from `postgres/postgres`. The `loams-postgres` repository has no upstream to merge after the hard fork.

### 3.4 Patch queue (D803)

- **Trailer.** Each commit after the fork point (`neon-fork-point`) carries `Loams-Patch: LP-<nnnn>` (several commits may share an id). In the Postgres fork, merge commits of upstream Postgres carry `Loams-Sync: <tag or sha>`.
- **Inventory.** `LOAMS_PATCHES.md` at the root of `main`, one row per id: title, area (`build`, `ci`, `licence`, `storage`, `compute`, `postgres`, `extension`, `pg18`, `pg19`, `rebrand`), owner, since, origin (`loams`, or `cherry-pick neondatabase/neon <sha>`), and the condition that drops it, if any. After the hard fork it is mainly the Apache-2.0 §4(b) statement of changes against Neon.
- **Check.** `scripts/loams/check-patches.py` lists `git log neon-fork-point..HEAD --no-merges` and fails when a commit has no trailer or a trailer has no row. It runs in the gate.
- **Review.** Every quarter, on the Postgres minor release day, a maintainer walks the inventory: Postgres-fork rows that upstream Postgres made unnecessary are dropped, and `loams-postgres` rows are kept current. Dropping is a revert commit, never history rewriting.
- **The first entries.** LP-0001 is the decoder trim (`1218fb7a`). LP-0002 to LP-0006 are D808's hygiene patches.

### 3.5 Governance (D804)

- **CODEOWNERS** in each fork, replacing Neon's teams with `@ostrium-labs/loams-postgres-maintainers` (storage, compute, CI), `@ostrium-labs/postgres-maintainers` (`vendor/`, `pgxn/`, `compute/`) and `@ostrium-labs/security` (`deny.toml`, `.github/`, `LOAMS_PATCHES.md`, `NOTICE`). **At least two people** are in each maintainer team. The teams are an owner action.
- **Branch protection** on `main` and `loams/*`: a PR, one CODEOWNER approval, the gate's required checks, DCO, no force-push, no deletion.
- **DCO.** The same `check-dco.sh` as Loams. Neon's history is exempt (the check starts at `neon-fork-point`).
- **The fork network.** The repository stays in `neondatabase/neon`'s fork network until GitHub support detaches it (Q730, an owner action). Until then PRs name the base `ostrium-labs/loams-postgres:main` explicitly.
- **The bot.** A GitHub App, `loams-postgres-bot`, with `contents: write` and `pull_requests: write` on the eight forks and `ostrium-labs/loams` (and `packages: write` for image pushes). It pushes Postgres branches from the `loams-postgres` repository's CI and opens pin PRs on Loams. The owner creates it, or approves the controller creating it through GitHub's App manifest flow (Q733).
- **Actions settings.** Actions are enabled on `loams-postgres` only. Upstream's workflow files are deleted from `main` once (LP-0002), so no upstream job queues on the self-hosted labels Neon used (`[self-hosted, large]`, `us-east-2`, and others).

### 3.6 Licences and notices (D804)

| Obligation | Source | How it is met |
|---|---|---|
| Keep `LICENSE` and `NOTICE`, and add a notice of our changes | Apache-2.0 §4(a), (b), (d) | `NOTICE` keeps Neon's lines and adds "Loams Postgres. Based on Neon by Databricks/Neon Inc. (https://github.com/neondatabase/neon), Apache License 2.0" and "Modifications Copyright 2026 Ostrium Labs and the Loams Authors"; `LOAMS_PATCHES.md` is the statement of changed files; images carry both under `/usr/share/doc/loams-postgres/` |
| Keep the PostgreSQL copyright notice | PostgreSQL licence | `vendor/postgres-v<N>/COPYRIGHT` is never touched; images carry it |
| Third-party Rust licences in binaries | MIT, Apache-2.0, BSD and others in about 790 crates | `cargo about` renders `THIRD_PARTY_LICENSES.html` per image from the lockfile, and the SBOM lists them |
| **No CDDL** | Owner ruling of 2026-10-08 (PG2 R31.12) | `inferno` (CDDL-1.0) comes in through `pprof` and `jemalloc_pprof`'s `flamegraph` feature into the pageserver, safekeeper, storage controller, proxy and `local_proxy` (through `http-utils`). LP-0003 turns the feature off (profiles stay available in pprof's protobuf format) and removes `CDDL-1.0` from the fork's `deny.toml` allow list |
| GPL components | PostGIS and pgRouting (GPL-2.0-or-later), postgresql-unit (GPL-3.0-or-later, verify) | Allowed in the compute image as separate programs loaded by Postgres, never linked into Loams' code. Each image digest gets an OCI `sources` artifact (`oras attach`) holding the exact tarballs it was built from, kept for 3 years |
| Forbidden in any image | D11, and the owner's CDDL rule | AGPL, SSPL, BUSL, ELv2, the Timescale License (TSL), Commons Clause, CDDL, and anything unlicensed. TimescaleDB is built with `-DAPACHE_ONLY=ON`, as Neon already does |
| Trademark | Apache-2.0 §6 grants no trademark rights | `README.md` and `LOAMS.md` name the project Loams Postgres and state that it is independent of and not endorsed by Neon Inc. or Databricks; "Neon" appears only in the attribution and in compatibility names (§1a.1). Our packages are renamed (D822, answering Q723); counsel still reads the attribution wording before Cloud GA |

### 3.7 Security (D805)

- **Reporting.** Private vulnerability reporting is enabled on every fork. `SECURITY.md` points at Loams' policy (`security@ostriumlabs.com`, acknowledgement within 3 business days).
- **Monitoring** (`loams-advisories.yml`, daily):
  - `cargo deny check advisories` on the fork's `Cargo.lock`;
  - Trivy (or Grype) scans of every pinned digest, covering Debian packages and extension binaries;
  - the PostgreSQL security page and its release feed;
  - GitHub advisories on `pgdogdev/pgdog`, each extension's upstream and the small forks' original upstreams; `neondatabase/neon` is watched as a signal only, since no fix is expected from it;
  - `cargo fuzz` targets for the WAL decoder and the page-service and proxy protocol parsers, run nightly for 30 minutes each (new with the hard fork: nobody else fuzzes this code).

  A finding opens an issue labelled with severity and a due date.
- **Targets (target, Q724):**

  | Finding | Images published by |
  |---|---|
  | Postgres security minor | 7 days after upstream's release (D241 said "within a week") |
  | CVSS ≥ 9.0 in a shipped component | 72 h |
  | CVSS 7.0–8.9 | 7 days |
  | Others | The next monthly roll-up |

- **Fork-specific issues** are published as GitHub advisories on `ostrium-labs/loams-postgres` and announced with Loams' own.
- **Debian.** The base is Debian 12 `bookworm`, pinned by digest; its standard security support ended in June 2026 and LTS runs to 2028-06-30 (verify). Images are rebuilt monthly for Debian updates. The move to `trixie` is Q727.

## 4. The one-time mirror (D806)

**What.** The images Loams pins today (PG2 Task 2, R2.1) are Neon's build of 2025-08-26 (git `77e22e4b`):

| Image | Index digest (unchanged by the mirror) |
|---|---|
| `ghcr.io/neondatabase/neon` | `sha256:7a4f124917bb929964b2d696d710f19584f80bb9bd51b2af4a6e2425434c761f` |
| `ghcr.io/neondatabase/compute-node-v17` | `sha256:13ab146d3e7bbabb25a8532f315ac443e7512351d1ede0bab586def5c70e26c3` |
| `ghcr.io/neondatabase/compute-node-v16` | the same build's digest, found by NF1 Task 1 (for exports of desktop data created at 16; never a default) |
| `ghcr.io/neondatabase/build-tools` | the `pinned` digest that build used, for parity rebuilds (§5.6) |

**How.** `podman run quay.io/skopeo/stable@sha256:<pinned> copy --all --preserve-digests docker://ghcr.io/neondatabase/<image>@<digest> docker://ghcr.io/ostrium-labs/neon-archive/<image>:mirror-77e22e4b`. `--all` copies every platform manifest and the BuildKit attestation manifests. `--preserve-digests` fails rather than re-encode. Verification compares the raw index and every child manifest byte for byte. The copies keep Neon's image names under `neon-archive/`, because they are Neon's binaries and not Loams Postgres (D822).

**The audit, before anything points at the mirror.** For each mirrored binary built with `cargo auditable` (`pageserver`, `safekeeper`, `storage_controller`, `storage_broker`, `proxy`, `compute_ctl`, `local_proxy`, `fast_import`), `rust-audit-info` lists the packages and their sources. Two findings are expected (verify):

1. **`inferno` (CDDL-1.0)** in the pageserver, storage controller, safekeeper and proxy of Neon's `neon` image, and in `local_proxy` of its `compute-node-v17`. Redistributing these binaries breaks the owner's "never ship CDDL" rule if it extends to images Loams re-hosts.
2. **`subzero-core` from `git+https://github.com/neondatabase/subzero`** in `proxy` (§3.2). It is code of unknown licence.

**Visibility (Q715, answered 2026-10-09: private until audited).** GHCR packages are created private. Pushing them needs a token with `write:packages`, which the current `gh` token lacks (§1a.2). Making one public is an owner action in the GitHub UI (verify that the API cannot do it). The proposed default:
- a package whose audit is clean is made public, and compose defaults move to it at once;
- a package with either finding stays **private**, a preserved copy only, and the defaults for it move straight to our first own build (`lp-2026.10.0`, NF1 Task 14), which is CDDL-free and subzero-free.

Loams' consumers either way are the two compose files, three workflow `PG_HEADERS_IMAGE` envs and, later, the single-node compose and the Helm values. All of them read the pin file (§12.1).

## 5. Our own builds (D807–D810)

### 5.1 Images

| Image | Contents | Platforms | Built from |
|---|---|---|---|
| `ghcr.io/ostrium-labs/loams-postgres-storage` | `pageserver`, `storage_controller`, `storage_broker`, `safekeeper` (bench reference only), `proxy` (no REST broker), `pagectl`, `storage_scrubber`, `endpoint_storage`, `neon_local`; **Postgres binaries for every supported major** (the pageserver's WAL redo and `initdb` need them); the `neon` extension per major | amd64, arm64 | `Dockerfile` at a `main` commit |
| `ghcr.io/ostrium-labs/loams-postgres-compute-v<N>` for `N` in the supported, maintenance and beta majors | Postgres `N` with Neon's patches, `compute_ctl`, `fast_import`, `local_proxy`, the catalogue's extensions, pgbouncer, the exporters | amd64, arm64 (Q729) | `compute/compute-node.Dockerfile` |
| `ghcr.io/ostrium-labs/loams-postgres-build-tools` | The Rust toolchain (`rust-toolchain.toml`: 1.88.0 at `fa504217`) and the build dependencies | amd64, arm64 | `build-tools/Dockerfile`; the two Dockerfiles take it by digest |

The test-extension images and the NeonVM images (`vm-compute-node-*`) are not published.

### 5.2 Workflows, runners and cost

All CI lives in `ostrium-labs/loams-postgres` on `main` (`.github/workflows/loams-*.yml`):

| Workflow | Trigger | Job shape |
|---|---|---|
| `loams-build-tools.yml` | a change under `build-tools/`, or monthly | One job per arch |
| `loams-build-storage.yml` | push to `main`, PRs (build only), `workflow_call` from the release | Per arch: cargo-chef cook (cached), build, `cargo auditable`; then the index |
| `loams-build-compute.yml` | a change under `compute/`, `pgxn/`, `vendor/`, the manifest; the release | Matrix major × arch. Heavy stages (`plv8`, `rdkit`, `postgis`, `pg_duckdb` while still in the catalogue) are built as separate jobs that push only the registry cache, so the final job stays under the 6 h hosted limit |
| `loams-gate.yml` | every PR into `main` or `loams/*`; nightly | §6.4's tiers |
| `loams-release.yml` | manual, or the minor-tracking job | Gate, build, sign, attest, notes, a pin PR on Loams |
| `loams-postgres-minors.yml`, `loams-extensions-update.yml`, `loams-advisories.yml`, `loams-fuzz.yml`, `loams-patches.yml` | schedules (§6, §7, §3.7) | Light (the fuzz job about 2 h nightly) |

- **Runners.** `runs-on: ${{ vars.RUNNER_HEAVY || 'ubuntu-24.04' }}`, plus `vars.RUNNER_HEAVY_ARM || 'ubuntu-24.04-arm'` for arm64. Standard hosted runners are free for public repositories (verify the arm64 terms on the day). If a cold compute build does not fit, `RUNNER_HEAVY` points at a larger runner (Blacksmith, which Loams already uses for `RUNNER_MAIN`, or GitHub's larger runners), within Q721's budget. The `loam-bench` self-hosted runner never builds: it produces gate numbers only (§46 §16.3).
- **Cache.** BuildKit `type=registry,ref=ghcr.io/ostrium-labs/loams-postgres-buildcache:<image>-<arch>[-pg<N>],mode=max`. The GitHub Actions cache (10 GB per repository) is too small for this tree. cargo-chef's dependency layer is already in the `Dockerfile`. The compute Dockerfile's per-extension `-src` and `-build` stages make each extension a cache unit, so a minor release rebuilds Postgres and its dependants, and a version bump rebuilds one extension.
- **Disk.** Hosted runners have about 14 GB free on `/` and more on `/mnt` (verify). BuildKit's root moves to `/mnt`.

**Costs (estimate; measured in NF1 Task 11):**

| Build | Cold, 4 vCPU | Warm cache | Larger runner (16 vCPU), cold |
|---|---|---|---|
| `loams-postgres-storage`, one arch | 60–120 min | 15–30 min | 25–45 min |
| `loams-postgres-compute-v<N>`, one arch, all tiers | 3–5 h (`plv8`'s V8, `rdkit`, `pg_duckdb` dominate) | 20–45 min | 60–100 min |
| A full release, three majors × two arches + storage | about 20–34 runner-hours | about 3–5 runner-hours | about 7–12 runner-hours |

At GitHub's larger-runner rate of about $0.064 per minute for 16 vCPU (verify), a cold full release costs about $27–46 and a warm one $5–10. With at most weekly storage builds, monthly compute roll-ups, a nightly 2-hour fuzz job (§3.7) and three or four minor releases a year, that is **$0 on standard runners, or about $50–150 a month on larger ones** (estimate). GHCR storage is free for public packages.

### 5.3 Provenance, SBOM and signing (D809)

- `docker/build-push-action` with `provenance: mode=max` and `sbom: true` (BuildKit's Syft scanner), as Neon did. Plus `actions/attest-build-provenance` and `actions/attest-sbom` on the index digest, so `gh attestation verify` works.
- **cosign keyless** (`cosign sign --yes <image>@<digest>`), through GitHub OIDC and Sigstore's public Fulcio and Rekor. No key to hold, which keeps Q282's offline key for the Loams CLI separate.
- **Verification in Loams** (`.github/workflows/nf1-pins.yml`): for every digest in the pin file, `cosign verify --certificate-oidc-issuer https://token.actions.githubusercontent.com --certificate-identity-regexp '^https://github.com/ostrium-labs/loams-postgres/\.github/workflows/loams-(release|sign)\.yml@refs/heads/main$'`. Signing runs in the reusable `loams-sign.yml`, whose `job_workflow_ref` Fulcio records (verify). Only releases and `build-tools` are signed; `edge` builds are never pinned. A mirrored digest (§4) is exempt and marked `source = "mirror:…"`.
- **GPL sources.** `oras attach --artifact-type application/vnd.loams.sources.v1` adds the exact source tarballs to each compute digest (§3.6).
- Every action is pinned by commit SHA, as Loams' own workflows are.

### 5.4 Tags and reproducibility

- **Tags tie to commits:**
  - `loams-postgres-storage:<sha12>` and `loams-postgres-storage:lp-YYYY.MM.N`;
  - `loams-postgres-compute-v17:17.11-<sha12>` and `loams-postgres-compute-v17:17.11-lp-YYYY.MM.N`;
  - `loams-postgres-build-tools:<sha12>`.

  Release tags are immutable by policy: the release job refuses to push a tag that exists. The moving tags are `edge` (the last `main` build) and, for a beta major, `beta`. Loams never pins a tag, only a digest.
- **Labels.** Each image carries `org.opencontainers.image.{source,revision,version,licenses}`, `dev.loams.fork-release` and `dev.loams.pg-version`. `pageserver --version` and `compute_ctl --version` print the commit (`GIT_VERSION`).
- **Reproducibility.** Base images are pinned by digest (Neon already does this for Debian), and so are toolchains, the sources' sha256 and `SOURCE_DATE_EPOCH`. **Bit-for-bit identical rebuilds are not promised at NF1**: apt resolves packages at build time. A monthly job rebuilds the last release and reports the layer diff for information only. Q726 decides whether to go further (snapshot.debian.org, a pinned apt state).

### 5.5 What the first own build proves

`lp-2026.10.0` is a **parity release**. It is built from `main` with the hygiene patches and the **same Postgres revisions as the mirror** (17.5, 16.9), so any behaviour difference is a pipeline defect, not a Postgres change. It passes when `loams-postgres`'s recorded fixtures still match byte for byte, `pg2-e2e.yml` is green, and `it-pageserver-loams-wal.sh` passes. The catch-up to 17.11 follows as `lp-2026.11.0` (§6.3).

### 5.6 Release cadence (D810)

| Train | When | Contents |
|---|---|---|
| Storage | When `main` has changed and the gate is green, at most weekly | Storage binaries, all majors' Postgres |
| Compute, minor | Postgres release day (the second Thursday of Feb, May, Aug and Nov; next: 2026-11-12, 2027-02-11, 2027-05-13, 2027-08-12) and out-of-cycle releases, **within 7 days** | `loams-postgres-compute-v<N>` for every supported major, and the `loams-postgres-storage` image (its WAL-redo binaries) |
| Compute, roll-up | Monthly, first Tuesday | Extension updates, Debian updates |
| Security | Per D805's targets | Whatever is affected |

Each release produces release notes listing the digests, the Postgres minors, the extension versions and the patches added or dropped. The bot then opens a pin PR on `ostrium-labs/loams` (§12.1). **Support window:** the two latest fork releases. Loams' pins move at least monthly.

## 6. Postgres minors (D811, amended by D820–D821)

The hard fork removed the upstream sync: nothing is merged from `neondatabase/*` (D820). What remains is tracking upstream **Postgres** directly.

### 6.1 What is tracked

| Source | Into | Cadence |
|---|---|---|
| `postgres/postgres` `REL_16_STABLE`, `REL_17_STABLE`, `REL_18_STABLE`, `REL_19_STABLE` | `ostrium-labs/postgres` `loams/REL_<N>_STABLE` (try-merge; a PR when it merges cleanly, an issue when it does not) | Nightly (D241's nightly job); release tags (`REL_17_12` and so on) start a release |
| `neondatabase/neon`, `neondatabase/postgres` | **Not tracked.** Watched as a signal by the advisory job (§3.7); a commit worth having is cherry-picked by hand as an `LP-` patch | — |
| The small forks' original upstreams | Not tracked; a fix only when an advisory needs it | On advisory |

### 6.2 (Removed) The weekly `neon` sync

D811's weekly merge of `neondatabase/neon` and the monthly small-fork sync are dropped by D820. NF1 Task 19 and its workflow `loams-sync-upstream.yml` are removed. Upstream's workflow files are deleted once by LP-0002 and nothing brings them back, so `strip-upstream-ci.sh` runs once.

### 6.3 Postgres minors

- **The catch-up first** (D241's step 1, 2–3 engineer-weeks, now NF1 Task 17). Move `vendor/postgres-v17` from `1e01fcea` (17.5) to `loams/REL_17_STABLE`, which starts from `REL_17_STABLE_neon` (17.8, Neon's last) and has 17.11 merged in from `postgres/postgres`. Do the same for 16 (16.12, then 16.15). Fix `pgxn/neon` and the extensions. This is the first non-parity release, `lp-2026.11.0`, and it must be out before 17.12 ships on 2026-11-12, or together with it.
- **Every minor after that:** the nightly try-merge has already resolved most conflicts. On release day the release job merges the tag, runs the full gate and publishes within 7 days (D805).
- **Conflicts** in Neon's patched areas (smgr, WAL redo, SLRU, the walproposer hooks; 27 sections in `docs/core_changes.md`) are resolved on `loams/REL_<N>_STABLE` with the conflict and its resolution recorded in the PR. A conflict that cannot be resolved within the security target escalates under D819's triggers.

### 6.4 The test gate

| Tier | What runs | When | Blocks |
|---|---|---|---|
| G0 | `cargo fmt --check`, clippy on the touched crates, `cargo deny check` (licences without CDDL, sources, advisories), `check-patches.py`, `check-no-private-sources.sh`, actionlint | Every PR | Merge |
| G1 | `cargo nextest` for `postgres_ffi`, `wal_decoder`, `pageserver_api`, `compute_api`, `compute_tools` and the pageserver's unit tests (with `NEON_PAGESERVER_UNIT_TEST_VIRTUAL_FILE_IOENGINE` set to both engines), for each supported major | Every PR | Merge |
| G2 | Neon's `test_runner` subset on `neon_local`, per supported major: `pg_regress` through Neon, branching, `compute_ctl` spec and reconfigure, `import_pgdata`, LFC, the walproposer against a safekeeper (names fixed in NF1 Task 16) | PRs touching `pgxn/`, `vendor/`, `pageserver/`, `libs/`, `compute*`; nightly | Merge and release |
| G3 | **Loams integration:** the release candidate's digests are sent by `workflow_dispatch` to `ostrium-labs/loams` `nf1-candidate.yml`, which runs `pg2-e2e.yml` (`loams-postgres` against `deploy/loams-postgres-dev`, `it-pageserver-loams-wal.sh`), `loams-wal-decoder`'s golden tests and the extension smoke, with the pin file overridden | Release candidates | Release |

A failing tier blocks. A known upstream flake is skipped only by a listed skip with an issue, never by deleting the test.

## 7. Extensions (D812)

### 7.1 The catalogue

Neon's compute Dockerfile has 98 stages, which is 38 third-party extensions built from source (a `-src` and a `-build` stage each), plus Postgres' contrib and the `neon` extensions (which keep their name, §1a.1). The tiers:

- **`core`:** built and smoke-tested on every supported major; a failure blocks the release; first in line for security updates.
- **`included`:** built when it supports the major; may lag a new major for up to one roll-up after upstream supports it.
- **`dropped`:** not built.

| Tier | Extensions (declared licence; Task 20 verifies each against its tarball) |
|---|---|
| **core** | Postgres contrib (`pg_stat_statements`, on by default per D717; `pgcrypto`, `uuid-ossp`, `hstore`, `citext`, `pg_trgm`, `btree_gin`, `btree_gist`, `ltree`, `intarray`, `unaccent`, `fuzzystrmatch`, `tablefunc`, `cube`, `earthdistance`, `postgres_fdw`, `dblink`, `pg_buffercache`, `pg_prewarm`, `pgstattuple`, `amcheck`, `pg_walinspect`, and the rest Neon allows) (PostgreSQL); `neon`, `neon_utils` (Apache-2.0); **`loams`** (Apache-2.0, §7.4); pgvector (PostgreSQL); PostGIS (GPL-2.0-or-later); pg_cron (PostgreSQL); pg_partman (PostgreSQL); pgaudit (PostgreSQL); pg_hint_plan (BSD-3-Clause); hypopg (PostgreSQL); pg_repack (BSD-3-Clause); wal2json (BSD-3-Clause); pg_ivm (PostgreSQL); pg_session_jwt (Apache-2.0) |
| **included** | pgRouting (GPL-2.0-or-later); h3-pg (Apache-2.0); postgresql-unit (GPL-3.0-or-later, verify); pgjwt (MIT); online_advisor (verify); pg_hashids (MIT); rum (PostgreSQL); pgTAP (PostgreSQL); ip4r (PostgreSQL); prefix (PostgreSQL); hll (Apache-2.0); plpgsql_check (MIT); TimescaleDB, Apache-only build (Apache-2.0); rdkit (BSD-3-Clause); pg_uuidv7 (MPL-2.0; Postgres 18 has `uuidv7()` built in, so it is dropped from 18 onward); pg_roaringbitmap (Apache-2.0); pg_semver (PostgreSQL); pg_jsonschema (Apache-2.0); pg_graphql (Apache-2.0); pg_tiktoken (Apache-2.0); pgx_ulid (MIT); postgresql_anonymizer (PostgreSQL); pgauditlogtofile (PostgreSQL); plv8 (verify) |
| **dropped** (proposed, Q719) | **pgrag** (Neon's experimental RAG extension, Apache-2.0; heavy model downloads); **pg_mooncake** and **pg_duckdb** (MIT; each vendors DuckDB, a C++ build of 30–60 min per major and arch (estimate); fast-moving; overlap with Loams House) |

The pgrx-based extensions (`pg_jsonschema`, `pg_graphql`, `pg_tiktoken`, `pgx_ulid`, `pg_session_jwt`, and others with an `-pgrx12`/`-pgrx14` stage) need a pgrx release that supports the major. pgrx usually lags a new major by weeks to months (verify for 19). That is why most of them are `included`, and why `pg_session_jwt`, the one in `core`, is in Loams' own fork and can be patched.

### 7.2 Manifest and licence check

`compute/loams-extensions.toml` (schema in the plan's shared contracts) lists each extension's name, Dockerfile stage, version, source URL, sha256, SPDX licence, tier, majors and whether `neon_superuser` may create it (`trusted`).

`scripts/loams/check-extensions.py` checks four things:
- the Dockerfile's `wget` URL and `sha256sum --check` value match the manifest;
- the licence file in the tarball matches the declared SPDX id (using `askalono` or a fixed hash of the licence text);
- the SPDX id is on the allowlist (§3.6);
- every built extension is in the manifest, and every non-dropped manifest entry is built.

`/usr/share/doc/loams-postgres-compute/EXTENSIONS.md` in the image lists each extension with its licence and upstream, generated from the manifest.

### 7.3 The build matrix per major

`majors = [16, 17, 18, 19]` in the manifest is the truth. The Dockerfile's `case "${PG_VERSION}"` blocks that pick versions per major (Neon's pattern) are kept, and the check cross-references them. The beta major (19) builds `core` only until its pgrx and C extensions catch up. A `core` extension that does not support a new major blocks that major's GA, not the other majors' releases.

### 7.4 Our own extensions

- **Where:** `pgxn/loams` (and `pgxn/loams_*` later), Apache-2.0, plain C against the server API, so a new major does not wait for pgrx. Each is built by the same Makefile path as `pgxn/neon` and ships in `core`.
- **The first one, `loams`** (NF1 Task 22), exists mainly to prove the path:
  - `loams.version()`;
  - `loams.compute_info()`, which returns the project, branch, endpoint and compute ids that `pg-control` sets as `loams.*` GUCs in the compute spec, so SQL clients and agents can tell which branch they are on.
  - It is `trusted`, has `pg_regress` tests per major, and needs no superuser.
- **Adding another:** a directory under `pgxn/`, a manifest row with `source = "in-tree"`, a `pg_regress` suite, and a `LOAMS_PATCHES.md` row.

### 7.5 Update policy

- `loams-extensions-update.yml` runs monthly. For each manifest entry it reads the upstream's releases.
- It proposes a PR (version, URL, sha256, manifest) for any release **at least 14 days old** (Loams' pinning rule), after G2's extension smoke passes.
- Security fixes skip the 14 days.
- A major version of an extension (PostGIS 3 → 4, for example) goes into a roll-up with a release note, never a minor release.
- An extension whose upstream has no release for 24 months, or changes to a forbidden licence, moves to `dropped` at the next roll-up, announced one roll-up in advance.

## 8. Postgres 18 and 19 (D813, D814)

### 8.1 Support policy (answers Q649)

| Major | Upstream EOL (verify) | Loams state | What we build |
|---|---|---|---|
| 14, 15 | 2026-11-12, 2027-11-11 | **Removed** from the fork's build (`vendor/postgres-v14`, `-v15` stay as submodules until NF1 Task 41 removes them) | Nothing |
| 16 | 2028-11-09 | **Maintenance:** minors; no new projects; images exist so data can be exported or upgraded | `loams-postgres-compute-v16`, its WAL redo in `loams-postgres-storage` |
| 17 | 2029-11-08 | **Supported.** Default for new projects until 18 is supported | All tiers |
| 18 | 2030-11-14 | **Supported** after NF1e's gate. Then the default for new projects (Q717) | All tiers |
| 19 | 2031-11 | **Beta** until NF1f's gate (Q718). Upstream: Beta 4 tagged 2026-09-21, RC1 on 2026-10-15 and GA on 2026-10-29 (targets). Projects need an explicit `allow_beta_major` | `core` tier only while beta |

**PG2 at GA:** 17 is supported; 18 too if NF1e has passed by then, but PG2's GA does not wait for it. This replaces PG2 Task 0's ruling 7 (17 only) without making PG2 slower.

### 8.2 Where the work is (D814)

Neon's Rust tree supports 14–17 (`PgMajorVersion::{PG14..PG17}`, `pg_constants_v14…v17.rs`); `pgxn/neon` has no Postgres 18 conditionals. Neon's last published 18 work is the Postgres-side branch `REL_18_STABLE_neon` (18.2, 106 commits over upstream, §28 §10) and a benchmarking branch. Nothing for 19 exists anywhere public. Each new major touches:

| # | Component | Work |
|---|---|---|
| 1 | `ostrium-labs/postgres` `loams/REL_<N>_STABLE` | 18: `REL_18_STABLE_neon` plus 18.6 merged. 19: Neon's patch series ported onto `REL_19_STABLE`, in `docs/core_changes.md` order, one `Loams-Patch` id per change |
| 2 | `vendor/postgres-v<N>`, `vendor/revisions.json`, `postgres.mk`, `Makefile`, `build-tools` | Submodule, the build, header installation |
| 3 | `libs/postgres_versioninfo` | `PG18`, `PG19` in `PgMajorVersion` and `ALL` |
| 4 | `libs/postgres_ffi` | bindgen for the new headers; `pg_constants_v<N>.rs`; `for_all_postgres_versions!`; the control file, `XLOG_PAGE_MAGIC`, `xlog_utils` |
| 5 | `libs/wal_decoder` | Every record the decoder interprets (heap, heap2, btree, smgr, dbase, xact, multixact, clog, relmap, standby, the logical messages), compared with the new major's `*_xlog.h` |
| 6 | Pageserver | `walingest.rs` (2,441 lines; SLRU, relation size and multixact bookkeeping), `basebackup.rs`, `import_datadir.rs`, WAL redo (`neon_walredo` per major), `pgdatadir_mapping.rs` |
| 7 | `pgxn/neon`, `neon_rmgr`, `neon_walredo`, the communicator | smgr callbacks (18's AIO read path), LFC, `neon_walreader`, the walproposer and its hooks, the custom rmgr for Neon's heap records |
| 8 | `compute_ctl` and `compute/` | Spec handling, `pg_hba`, extension paths, `compute/etc` configs, the compute Dockerfile's per-major `case` blocks |
| 9 | Images | `loams-postgres-compute-v<N>`; the `loams-postgres-storage` image gains the major's binaries |
| 10 | Loams: `crates/loams-wal-decoder`, `crates/loams-safekeeper`, `scripts/pg2/pg-headers.sh` | The decoder's fork rev; golden fixtures at the major; the greeting's `pg_version` (`180000`, `190000`); headers for the new major |
| 11 | Loams: `crates/loams-postgres`, `loams-pg-control`, the desktop, PgDog | The supported-majors list from the pin file; the per-major compute; the router inventory (§12.4) |

### 8.3 Format and behaviour changes to check first (verify each)

The port starts with a written diff, `docs/loams/pg<N>-format-diff.md` in the fork. It covers `XLOG_PAGE_MAGIC`, `rmgrlist.h`, every `*_xlog.h`, `pg_control.h`, the SLRU layouts, `xl_running_xacts`, checkpoint records and the smgr API. Each item is classified as "decoder", "walingest", "redo", "compute only" or "none". Items already known to need a ruling:

| Major | Change | Why it matters here |
|---|---|---|
| 18 | A new `XLOG_PAGE_MAGIC` | The decoder, the pageserver and `loams-wal-decoder` refuse unknown magic; a new arm is needed |
| 18 | Asynchronous I/O (`io_method`, default `worker`) and the smgr read-stream path | Neon replaces `md.c`; `pgxn/neon`'s smgr must implement the new callbacks or force synchronous reads |
| 18 | `initdb` enables data checksums by default | The pageserver bootstraps timelines with `initdb`; pages it reconstructs through WAL redo must carry valid checksums, or bootstrap must pass `--no-data-checksums`. Decide from evidence |
| 18 | Protocol 3.2 (longer cancel keys) in libpq and the server | PgDog, Neon's proxy, `local_proxy` and pgbouncer must accept or negotiate it (§12.4) |
| 18 | `uuidv7()` built in; MD5 password deprecation warnings | Catalogue (§7.1); D711 already refuses MD5 |
| 19 | The multixact offset width (a 64-bit `MultiXactOffset` was proposed for 19) | It changes the multixact SLRU and its WAL records, which `walingest.rs` and the decoder handle directly |
| 19 | Anything else in 19's release notes touching WAL, SLRU, the control file or smgr | Read at RC1 (2026-10-15), recorded in the diff |

### 8.4 The test matrix (per supported or beta major)

| Layer | Test |
|---|---|
| Postgres | `make check` and `installcheck-world` on `loams/REL_<N>_STABLE` |
| Fork Rust | G1 with the major's constants; `wal_craft` fixtures generated at the major, decoded by `wal_decoder` with every record type covered |
| Neon integration | G2 at the major: `pg_regress` through Neon, branching at LSN and timestamp, `import_pgdata`, restart from the pageserver, read replicas, LFC resize |
| `loams-wal` | The walproposer at the major against `loams-wal` (both stores); the interpreted sender's golden fixtures at the major (`fork-sender-<N>-*.bin`); `it-pageserver-loams-wal.sh` at the major |
| Extensions | Each `core` extension: `CREATE EXTENSION`, a smoke query, survival of a branch and a restart |
| PgDog | `conformance/router` inventories at the major (§12.4) |
| Upgrade | 17 → 18 and 18 → 19 through §9's path, with table checksums before and after |
| Desktop | The local stack with a timeline of each supported major |

### 8.5 Timeline (estimate, with D824's staffing of about 1.25 engineers)

| Date | Event |
|---|---|
| 2026-10-15 | Postgres 19 RC1: `loams/REL_19_STABLE` created and built nightly; the 18→19 format diff starts |
| 2026-10-29 | Postgres 19 GA (upstream target). **Loams has no 19 images yet; 19 is labelled beta in every Loams document** |
| 2026-11-12 | Minors 17.12, 18.7, 16.16 (and 14's last); our images by 2026-11-19 |
| 2026-12 | `loams-postgres-compute-v18` preview images |
| 2027-01 | PG18 supported (NF1e gate) |
| 2027-02 | `loams-postgres-compute-v19` beta images, `core` tier, after the 2027-02-11 minors |
| No earlier than 2027-03 | PG19 GA in Loams, by Q718's conditions |

## 9. Upgrade paths (D815)

**What upstream offers.** Neon's open tree has no `pg_upgrade` integration. It does have two pieces that compose into one:
- the pageserver's `TimelineCreateRequestMode::ImportPgdata`, which builds a timeline's layers from a data directory in object storage (`ImportPgdataLocation::AwsS3`; `LocalFs` only under the `testing` feature);
- `compute_tools`' `fast_import`, which, in `pgdata` mode, dumps a source over a connection string into a fresh data directory of **its own major** and uploads it.

**Path 1, at NF1 (dump into a new-major data directory, import, swap):**
1. Create a read-only endpoint on the source branch at the current LSN (the source keeps serving).
2. Run `fast_import pgdata` from the **target** major's compute image, with the read-only endpoint as its source.
3. `POST /v1/tenant/{t}/timeline/` with `import_pgdata` into the **same tenant**, as a new root timeline (`pg_version` = target), and create its `loams-wal` timeline at the import's end LSN (verify that `loams-wal` accepts a start LSN, as R2.10's idempotent create suggests).
4. `pg-control` records a new branch `<name>-pg<N>`. On the user's confirmation it swaps the endpoints, as `RestoreBranch` does (§46 §10). The old branch stays as the backup branch with a TTL.

Downtime is the dump's duration if writes must stop (an optional write freeze), or zero with a final short delta via logical replication (a later option). The bulk load bypasses the WAL path entirely, which also sidesteps `loams-wal`'s open `bulk` throughput gap (§46 §9.4). On self-hosts and the desktop, the import's S3 location is RustFS (S3-compatible; verify that the pageserver honours a custom endpoint there).

**Path 2, a spike (Q725): `pg_upgrade --link`.**
1. Materialise the source branch's data directory from a pageserver `basebackup` at a quiesced LSN.
2. Run `pg_upgrade --link` (or 18's `--swap`) to the target major.
3. Upload the result and import it as in path 1.

It is much faster for large databases, if a Neon basebackup can be brought to the clean shutdown that `pg_upgrade` requires (verify). NF1 Task 38 measures both paths.

**Rules:**
- Data is **never upgraded without an explicit user action** (`UpgradeProject` or the desktop's prompt).
- A compute never attaches to a timeline of another major (`compute_major_mismatch_refused`).
- Dump and restore into a new project (D716) remains the fallback for anything the import refuses.

PG2 Task 51 implements `UpgradeProject` on top of NF1 Task 37's pieces.

## 10. Dropping private dependencies (D816)

| Dependency | Needed by | Removed how | Loss |
|---|---|---|---|
| `subzero` (private) | `proxy` feature `rest_broker`: the embedded Data API (PostgREST-compatible REST over the proxy) | LP-0004: drop the `SUBZERO_ACCESS_TOKEN` steps from `Dockerfile` and `.github/actions/prepare-for-subzero`; keep the stub crate so the workspace builds; `rest_broker` never enabled; `check-no-private-sources.sh` fails on `neondatabase/subzero` in `Cargo.lock` or any Dockerfile | Neon's Data API. Loams never offered it. A REST API over Loams Postgres, if wanted, is a later separate service, for example PostgREST (MIT) as its own process (Q720) |
| `flux-fleet` (private) | Comments only (compute metrics scrape configs) | Nothing to remove for the build. Loams' scrape configs are in `deploy/observability/loams-postgres/` (PG2 Task 46) | None |
| `cloud`, `infra` (private) | Upstream CI triggers for Neon's deploys and e2e tests | LP-0002 removes upstream's workflows | None |
| `cache.neon.build`, `neondatabase/dev-actions`, `gh-workflow-stats-action`, `proxy-bench` | Upstream CI | Not used by `loams-*` workflows | None |
| `neondatabase/lcov` | `build-tools` (coverage) | Debian's `lcov` package, or dropped | None |
| `neondatabase/pg_session_jwt`, `neondatabase/pgrag` downloads | The compute Dockerfile | `pg_session_jwt` from `ostrium-labs/pg_session_jwt`'s tag tarball (same sha256 if unchanged); `pgrag` dropped (Q719) or forked | None, or pgrag |
| `neondatabase/autoscaling` `vm-builder` | NeonVM images | Not built (D712) | None |

## 11. `tokio-epoll-uring` (D817)

**The facts, corrected (checked 2026-10-09):**
- `ostrium-labs/tokio-epoll-uring` (and upstream) has **no `LICENSE` file**; GitHub reports no licence.
- `tokio-epoll-uring/Cargo.toml` declares `license = "MIT OR Apache-2.0"`, at `main` and at the revision Neon pins (`781989bb`).
- `uring-common/Cargo.toml` declares `license = "MIT" # the same as tokio-uring at the time we forked it`; its `IoBuf`/`IoBufMut` code was vendored from `tokio-uring` (MIT) in upstream PR #24.
- So the code is **licensed by declaration**: Neon's authors stated SPDX terms in the published manifests. The licence texts and the copyright lines that MIT and Apache-2.0 expect to travel with the code are missing. That is a compliance gap, not "all rights reserved".

**Who uses it:**
- the pageserver only, unconditionally on Linux (the `TokioEpollUring` I/O engine, the default; `StdFs` is the alternative at run time);
- its `IoBuf` traits are used across `virtual_file/owned_buffers_io`.

Loams' own lockfiles do not contain it (`crates/loams-wal-decoder` trimmed it, PG2 R31.12).

**Resolution, in order:**
1. **In our fork** (LP-0007): add `LICENSE-MIT` and `LICENSE-APACHE` with the declared terms, attributed to "the tokio-epoll-uring authors (Neon Inc.)", and `tokio-uring`'s MIT notice in `uring-common/`. Add a `LICENSING.md` that cites the manifests and the upstream commits. The image's third-party licence file and SBOM then carry the texts.
2. **Ask upstream once**, if the owner allows it as an exception to D231's "nothing is posted upstream" (Q716): an issue asking Neon to add the licence files matching the manifests.
3. **Fallback**, only if counsel finds the declaration insufficient: a pageserver feature `io-uring` (default on). With it off, the `TokioEpollUring` engine is compiled out and `uring-common`'s small trait set is vendored into the pageserver with `tokio-uring`'s MIT notice. That build uses `StdFs`. Estimate: 1–2 engineer-weeks plus a performance check, because the traits are used across `owned_buffers_io`. The gate's G1 already runs both engines.

## 12. Integration with Loams (D818)

### 12.1 One pin file

`deploy/pins/loams-postgres-images.toml` (schema in the plan) holds:
- the fork release and commit;
- per image: the repository, the index digest, the source (`ostrium-labs/loams-postgres@<sha>`, or `mirror:<origin>@<digest>`) and the Postgres minor;
- the majors: `supported`, `beta`, `maintenance` and `default`.

`scripts/nf1/pins.py check` fails when any consumer differs from it. The consumers are:
- the compose defaults of `deploy/loams-postgres-dev` and `deploy/loams-pg-bench`;
- `PG_HEADERS_IMAGE` in `pg2.yml`, `pg2-e2e.yml` and `loams-pg-bench.yml`;
- later, `deploy/loams-postgres-single` and the Helm chart's values.

`pins.py set` rewrites them all. The fork's release bot opens the PR. `nf1-pins.yml` runs `cosign verify` (§5.3). `loams-postgres` and `pg-control` read the majors from a generated Rust constant, not from the TOML at run time.

### 12.2 The decoder's dependency

- `crates/loams-wal-decoder/Cargo.toml` keeps Cargo `rev =` pins (a branch dependency would float), and moves from `1218fb7a` (`loams/decoder-trim`) to a commit on `main` that carries a `loams-*` tag. Its URL becomes `https://github.com/ostrium-labs/loams-postgres` in NF1 Task 1b (D823), with the same `rev`.
- `scripts/nf1/check-pinned-revs.sh` checks that every `ostrium-labs` rev in any Loams manifest is reachable from `main` or a `loams-*` tag. A rebase that would orphan a rev fails the check.
- The decoder's `deny.toml` keeps its `ostrium-labs` sources. Bumping the rev is part of every fork release that touches `libs/{wal_decoder,postgres_ffi,pageserver_api,utils}`, with the golden fixtures re-recorded when the format changes.

### 12.3 The desktop's stack and Postgres majors

- **Today** (PG2 R2.2): `deploy/loams-postgres-dev` runs 17. A local stack whose timelines were created at 16 needs a fresh volume, and PG2 Task 58's local-stack backend says so.
- **NF1 generalises this** (after PG2 Task 58; AP1e's `neon.ts` and `pg.ts` are built, PG2 Task 0 ruling 8):
  - The stack keeps one compute service per major present in the pin file (`compute-v17`, `compute-v18`, and `compute-v19` under a `beta` compose profile). The `loams-postgres-storage` image already carries every major's WAL redo.
  - The local-stack backend reads each timeline's `pg_version` from the pageserver (`GET /v1/tenant/{t}/timeline/{tl}`) and starts the compute of that major. It refuses to start another major's compute on it.
  - New tenants use `default`. Existing timelines keep their major.
  - When `default` moves (17 → 18), the desktop **prompts** to upgrade a branch through §9's path 1, run locally against RustFS. It never upgrades by itself (Q728).
  - 16 timelines get the same prompt, using `loams-postgres-compute-v16` (or, before our first 16 build, Neon's mirrored `neon-archive/compute-node-v16`) pulled on demand.
- The stack's version marker (`.loams-stack-version`) gains the fork release, so a pin change refreshes the copied stack files without touching the volumes. The copied directory stays `stacks/neon` and the compose project stays `loams-neon`, because Docker names the volumes after it (D823).

### 12.4 PgDog compatibility

PgDog stays unmodified (D708). Per major, NF1 Task 36 runs these through the pinned PgDog against a compute of that major:
- the `conformance/router` inventory;
- driver connects with protocol 3.0 and 3.2;
- cancel requests with 18's longer keys;
- SCRAM;
- the new syntax that PgDog's parser must route: 18's `RETURNING OLD/NEW`, virtual generated columns and `WITHOUT OVERLAPS`, and 19's from its release notes.

The results go in `conformance/router/pgdog-pg<N>.tsv`. A major is "supported through PgDog" only with no `error` or `differs` row in a component Loams uses. Otherwise the gap is documented with its workaround (session mode, or a client setting such as `max_protocol_version=3.0`) and reported to PgDog upstream with the owner's approval (§46 §8.3).

### 12.5 What changes in PG2

- **Task 0, ruling 7** (Q649: 17 only at GA): replaced by D813. 17 at GA, and 18 if NF1e has passed.
- **Task 2, R2.1** (Neon's images; "when the fork publishes images, change the four places"): done by NF1 Tasks 2 and 14 through the pin file.
- **Task 51** (`UpgradeProject` by dump and restore): built on NF1 Task 37 (§9's path 1), with dump and restore as the fallback.
- **Task 55** (extensions, Q651): uses D812's catalogue and the `core` smoke.
- **§46 §12** ("Fork releases follow D241"): now D810, and D811 as amended by D820–D821 (no upstream sync).
- **§46 Risk 6:** now covered by D805's targets and D824's staffing and modes.
- **Names** (D822, D823): `crates/loams-neon` is `crates/loams-postgres`, `deploy/neon` is `deploy/loams-postgres-dev`, and the images are `loams-postgres-storage` and `loams-postgres-compute-v<N>` once NF1 Task 1b and Task 14 land. PG2's text keeps the old names until then; its header carries the mapping.

### 12.6 The crate and path rename (D823)

NF1 Task 1b renames the Loams side in one PR, after Task 0 and before Task 2 writes the pin file's consumers:

| What | From | To | Notes |
|---|---|---|---|
| Crate | `crates/loams-neon`, package `loams-neon` | `crates/loams-postgres`, package `loams-postgres` | `publish = false`, so crates.io is not involved. Its description already says "Loams Postgres's client" |
| CI | `pg2.yml`, `pg2-e2e.yml` path filters, `-p loams-neon`, cache keys `pg2-loams-neon` | The new names | Job names keep their meaning |
| Decoder | `git = "https://github.com/ostrium-labs/neon"` (four crates), `[patch."https://github.com/ostrium-labs/neon"]`, `deny.toml` `allow-git` | `https://github.com/ostrium-labs/loams-postgres` | Same `rev` `1218fb7a`. Cargo treats a new URL as a new source, so `Cargo.lock` is regenerated (`cargo update -p` on the four crates, in CI or with the owner's leave to fetch) and the golden tests must stay byte-equal |
| Dev stack | `deploy/neon` | `deploy/loams-postgres-dev` (Q731) | `deploy/loams-postgres` is avoided: `deploy/helm/loams-postgres` and `deploy/loams-postgres-single` already name the product's deployments |
| Kept | Compose `name: loams-neon`, the desktop's copied `stacks/neon`, the compose anchors `x-neon-image`, `x-compute-image` and the overrides `NEON_IMAGE`, `COMPUTE_IMAGE` | Unchanged | They name users' volumes, files and scripts. A later migration task may rename them with a volume move |
| Desktop | `electron-builder.config.cjs` (`from: "../../deploy/neon"`), `stacks.ts` comment, `sql/neon.ts`, `sql/pg.ts`, `test/sql-integration.test.ts` | The new source path; the file `neon.ts` keeps its name for now | Only paths that point into the repository change |
| Loams `NOTICE` | "the fork https://github.com/ostrium-labs/neon", "deploy/neon/ is derived from" | The new names, still crediting Neon | |
| Docs | §23, §28, §31, §37, §46, the PG2, RT and AP1e plans | A mapping note, not a rewrite of history | Historical rulings keep the names they used |

**Name clashes checked (2026-10-09):** no workspace crate, package or directory is named `loams-postgres` (`crates/` has `loams-neon` only for Postgres; the root workspace is `crates/*` with `crates/loams-wal-decoder` excluded). The planned `loams-pg-control` and `loams-pgdog-sync` crates and `deploy/loams-pg-bench` use the `loams-pg-` prefix and stay distinct. The desktop's `web/plugins/postgres` (`@loams/plugin-postgres`) and stack id `postgres` are TypeScript and do not meet Cargo names. `proto/loams/postgres/v1` (`loams.postgres.v1`) generates the module `loams_proto::postgres`, not a crate. The CLI's future `loams postgres` command is a subcommand name. The one real risk is reading: `loams-postgres` the crate is the **client of the storage and compute components**, not the whole product, so its `Cargo.toml` description and `lib.rs` header say so.

## 13. Ownership, staffing and minimum viable maintenance (D819, re-costed by D824)

Maintaining a hard fork of Neon is a large, open-ended commitment. Neon Inc. ran this with dedicated storage, compute and release teams. Loams inherits all of it, without an upstream that fixes storage bugs or ships security fixes. The hard fork removes the upstream sync, which was cheap because upstream was dormant, and adds work that a soft fork could have hoped to share: security review of Neon's own code, fuzzing, answering reports against a public repository under our name, and the rebrand. The numbers below are estimates; NF1 Task 40 records actuals after the first two quarters.

### 13.1 One-time work (NF1)

| Milestone | Before (D819) | After (D824) | Why |
|---|---|---|---|
| NF1a governance, mirror, pins, **the rename (Task 1b)**, advisories and fuzzing | 1–1.5 | 2–3.5 | +0.5–1 for Task 1b; +0.5–1 for the fuzz targets |
| NF1b own builds, hygiene, supply chain, parity release | 3–5 | 3–5 | The new image names cost nothing: nothing is published yet |
| NF1c ~~sync,~~ gate, the catch-up to 17.11 and 16.15, minor tracking | 3–4 | 2.5–3.5 | Task 19 (weekly sync) removed |
| NF1d extensions | 1.5–2.5 | 1.5–2.5 | |
| NF1e Postgres 18 | 4–7 | 4–7 | |
| NF1f Postgres 19 (no Neon branch to start from) | 5–9 | 5–9 | |
| NF1g upgrades, desktop, PgDog, runbooks | 3–5 | 3–5 | |
| **Total (engineer-weeks)** | **20.5–34** | **21–35.5 (about 5–9 engineer-months)** | |

### 13.2 Ongoing, full mode

| Work | Per year, before (D819) | Per year, after (D824) |
|---|---|---|
| Postgres minors: 4 regular releases a year plus out-of-cycle ones, for 3 majors, with conflicts in Neon's patched areas resolved by us alone | 2.5–5 | 3–5.5 |
| A new major every year (Postgres 20 in late 2027), with no Neon branch | 5–9 | 5–9 |
| ~~Weekly sync,~~ the Rust toolchain and about 790 crates (RUSTSEC, deprecations) | about 5 | 4–5 |
| Extensions: updates, new-major lag, 35+ upstreams | 3–5 | 3–5 |
| Security: triage, response, **our own fixes to Neon's code**, the fuzz findings | 3–5 | 5–8 |
| CI, runners, images, Debian, the fuzz job | about 2.5 | 2.5–3 |
| Storage bugs Loams finds in production (no upstream to rely on) | 4–8, highly uncertain | 4–8, highly uncertain |
| Rebrand and compatibility migrations (binaries, aliases, docs; §1a.1) | — | 1–3 in the first year, then about 0.5 |
| Issues and PRs from others on a public repository under the Loams name | — | 1–2 |
| **Total** | **25–40 engineer-weeks, about 0.6–0.9 FTE** | **29–49 engineer-weeks, about 0.7–1.1 FTE**, with peaks of 1.2–1.7 FTE from September to January around each new major |

**Staffing (D824, Q734).** One full-time maintainer for storage and Rust, plus a second maintainer at about 0.25 FTE for Postgres and C, compute, reviews and verifying the runbooks, plus help at the September–January peak. That is about 1.25 FTE in full mode. The two named maintainers of D804 are still the minimum; with the hard fork, the second one is also the only other person who knows the code.

**Money.** CI stays as §5.2 estimates: $0 on standard hosted runners for a public repository, or about $50–150 a month on larger runners (Q721). The fuzz job runs on standard runners. Engineering is the cost that matters: about 1.25 FTE, against 0.75–0.9 before the hard fork.

### 13.3 Minimum viable maintenance (MVM)

MVM is a switch, `vars.MAINTENANCE_MODE = mvm` on the `loams-postgres` repository. It is about **0.25–0.3 FTE** (estimate; was 0.15–0.2 before the hard fork, because security fixes to Neon's code now always fall on us).

**MVM keeps:**
- Postgres security minors for supported majors within D805's targets (the nightly try-merge stays);
- critical and high RUSTSEC and image CVEs, and critical or high fuzz findings;
- the monthly Debian rebuild;
- the gate;
- the advisory job.

**MVM stops:**
- extension updates except security;
- new majors: the newest supported major stays, and beta majors freeze;
- new extensions;
- the pg_upgrade spike;
- rebrand and migration tasks;
- any feature work in the fork.

**Triggers.** Full mode is the default while NF1 runs and while Loams Postgres is in beta (Q722; the owner chose full mode on 2026-10-09).
- **To MVM:** fewer than two maintainers are available for more than 30 days, or the owner decides it.
- **Back to full:** a supported major is within 12 months of EOL, a new major is needed by Loams' roadmap, or a security fix needs more than 5 engineer-days.
- **Escalation to the owner:** a missed security target, or a storage bug with data-loss potential. It is never handled in MVM silently.

### 13.4 Risks to ownership

- **Bus factor:** two named maintainers are required (D804). Runbooks for each release type are in `docs/runbooks/loams-postgres-fork/`, and each is exercised once by the second maintainer.
- **Knowledge:** Neon's storage internals are deep. Loams keeps `loams-postgres` free of Neon code dependencies (§46 §6.1), so the control plane survives a fork rewrite. The archive restore path (§46 §9.6) does not depend on Neon's layer format.
- **No upstream to compare with:** after the hard fork, a behaviour that looks wrong cannot be checked against a newer Neon. The fuzz targets, the gate's `test_runner` subset and Loams' own integration tests are the only reference, which is why they block releases.
- **Upstream reappears:** if Neon publishes code again, it is Apache-2.0 and can be cherry-picked as patches (D820). Nothing obliges Loams to follow it.
- **The exit:** if ownership becomes unaffordable, the fallback is MVM on 17 and 18 only, with CloudNativePG (§28 P1) for users who need a new major sooner. That is a product decision for the owner, recorded here so it is not discovered late.

## 14. As built on 2026-10-09

| Item | State |
|---|---|
| `ostrium-labs/loams-postgres` | Renamed from `ostrium-labs/neon` on 2026-10-09 (GitHub redirects the old URL); still in `neondatabase/neon`'s fork network (Q730). Default branch `main` at `fa504217`; branch `loams/decoder-trim` with one commit `1218fb7a` on `fa504217`; tag `loams-decoder-trim-1` (still present). Actions allowed, 0 workflows active. No images published. The local read-only clone stays at `~/Documents/Ostriumlabs/neon`, its `origin` pointing at the new URL |
| `ostrium-labs/postgres` | Fork with 322 branches, including `REL_14…18_STABLE_neon`; no `loams/*` branches; not cloned locally |
| Upstream Postgres | 18.6, 17.11 and 16.15 are the latest minors; `REL_19_STABLE` exists and `REL_19_BETA4` was tagged on 2026-09-21 |
| Loams pins | `dev` (PG2 Task 2 merged, NF1 Task 0 ruling 1): Neon's `77e22e4b` images by digest in `deploy/neon`, `deploy/loams-pg-bench` and three workflows |
| `crates/loams-wal-decoder` | `rev = 1218fb7a` for `pageserver_api`, `postgres_ffi`, `utils` and `wal_decoder`; `ostrium-labs/rust-postgres` at `f3cf448f`; no `tokio-epoll-uring`, no Azure, no CDDL |
| Loams names | The crate is still `crates/loams-neon` and the stack `deploy/neon` (compose project `loams-neon`); NF1 Task 1b renames them (D823). The decoder still names `https://github.com/ostrium-labs/neon`, which GitHub redirects |
| Tokens | The `gh` token has `repo` and `workflow` scopes only: no `write:packages`, no `admin:org`. No GitHub App exists (§1a.2) |
| Desktop | `apps/desktop-electron/src/main/stacks/stacks.ts` copies `deploy/neon` (to `stacks/neon`) with a version marker; `src/main/sql/{neon.ts,pg.ts}` drive it (AP1e Tasks 21–23, PG2 Task 0 ruling 8). There is no major handling yet (PG2 Task 58 adds the 16 → 17 guard) |

## 15. Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | Cold compute builds exceed hosted runners' 6 h limit or 14 GB of disk | Stage-split jobs pushing the registry cache; BuildKit on `/mnt`; `RUNNER_HEAVY` for larger runners within Q721's budget; heavy extensions dropped (Q719) |
| 2 | The 17.5 → 17.11 catch-up breaks `pgxn/neon` or extensions, and 17.12 arrives on 2026-11-12 | The nightly try-merge starts in NF1c at once; the parity release separates pipeline faults from Postgres changes |
| 3 | The PG18/19 port finds a WAL or SLRU change that the pageserver handles subtly wrongly | The format diff comes first; `wal_craft` fixtures cover every record type; checksums of a pgbench database are compared after a pageserver restart and a branch |
| 4 | PG19 is expected "at GA" by users | Every Loams document labels 19 beta until Q718's conditions hold |
| 5 | The mirror redistributes CDDL code or private `subzero` code | Task 1's audit decides visibility; own builds replace the mirror within weeks (Q715) |
| 6 | `tokio-epoll-uring`'s licence is challenged | Licence texts as declared, an upstream ask (Q716), and the fallback build (§11) |
| 7 | Staffing falls below what full mode needs | MVM, with explicit triggers (§13.3); escalation instead of silent lag |
| 8 | A force-push or rebase on the fork orphans a pinned revision, and Loams cannot build an old release | D802's and D821's merge-only rule, branch protection, `loams-*` tags, and `check-pinned-revs.sh` |
| 9 | pgrx lags 19, so `pg_session_jwt` (`core`) blocks 19's GA | It is our fork; patch it, or move it to `included` for 19 with a release note |
| 10 | GitHub's free-runner terms for public repositories change | `vars.RUNNER_*` makes the switch a setting; the cost table bounds it |
| 11 | The trademark "Neon" in package names | **Resolved by the rebrand** (D822): our packages are `loams-postgres-*`; "Neon" remains only in the attribution, the private `neon-archive/` copies and compatibility names. Counsel reads the attribution wording before Cloud GA |
| 12 | Security and bug fixes are ours alone, and nobody else reviews or fuzzes this code (D820) | D805's targets; the fuzz job (§3.7); D824's staffing; MVM never drops security |
| 13 | The fork network is not detached, so PRs and forks default to `neondatabase/neon` and the repository looks like a mirror | Owner support request (Q730); until then PRs name the base explicitly and `LOAMS.md` states the hard fork |
| 14 | The rename (Task 1b) loses users' local data, because Docker names volumes after the compose project | The compose project `loams-neon` and the copied `stacks/neon` stay (D823); a test checks the project name |
| 15 | The decoder's new git URL changes Cargo's source ids and a stale lockfile or cache resolves the old one | Task 1b regenerates `Cargo.lock` with the same `rev` and re-runs the golden tests; `check-pinned-revs.sh` reads either URL until the old one is gone |

## 16. Open questions (Q715–Q734)

| # | Question | Needed by |
|---|---|---|
| Q715 | The mirror's visibility. If Task 1's audit finds `inferno` (CDDL) or `subzero-core` in a mirrored binary, does the "never ship CDDL" rule extend to re-hosting Neon's images? Default: such a package stays private (preservation only), and compose defaults move straight to our first own build | NF1 Task 1 |
| Q716 | `tokio-epoll-uring`: accept the `Cargo.toml` declarations as the licence, with texts added in our fork (default: yes)? And may Loams file one upstream issue asking Neon to add licence files, as an exception to D231 (default: yes)? | NF1 Task 8 |
| Q717 | Majors: is 18 the default for new projects once supported (default: yes), and does 16 stay in maintenance (minors and exports, no new projects) until PG2 GA plus 6 months, then drop (default)? | NF1 Task 30 |
| Q718 | When is Postgres 19 GA in Loams? Default: NF1f's matrix green twice, upstream 19.1 released, and 30 days of the beta on Loams' own test cluster with no data-loss bug | NF1 Task 35 |
| Q719 | The catalogue: drop `pgrag`, `pg_mooncake` and `pg_duckdb` (default: yes); keep `plv8` and `rdkit` as `included` (default: yes); ship GPL extensions (PostGIS, pgRouting, postgresql-unit) with an attached source artifact (default: yes) | NF1 Task 21 |
| Q720 | The Data API: drop the `subzero` REST broker with no replacement in NF1 (default), and consider PostgREST as a separate service after PG2 GA? | NF1 Task 7 |
| Q721 | The CI budget: standard hosted runners first (free for public repositories), with a cap of $150 a month for larger runners or Blacksmith when cold builds do not fit (default)? Who holds the billing? | NF1 Task 11 |
| Q722 | The two named maintainers, the CODEOWNERS teams, and the mode: full during NF1 and Loams Postgres beta, then reviewed (default)? | NF1 Task 5 |
| Q723 | ~~Package names: keep `neon` and `compute-node-v<N>` for continuity, or rename before Cloud GA?~~ **Answered by D822** (owner, 2026-10-09): renamed to `loams-postgres-storage` and `loams-postgres-compute-v<N>` | Answered |
| Q724 | Security targets: 72 h critical, 7 days high and Postgres security minors, 30 days others (default)? | NF1 Task 6 |
| Q725 | The `pg_upgrade --link` path: spike after path 1 lands (default), or skip and rely on `fast_import`? | NF1 Task 38 |
| Q726 | Bit-for-bit reproducible images: not at NF1 (default), or invest in snapshot apt and rebuild verification? | NF1 Task 12 |
| Q727 | The Debian base: move every image to Debian 13 `trixie` with the PG18 work (default), or keep `bookworm` (LTS to 2028-06) for 16 and 17? | NF1 Task 28 |
| Q728 | The desktop when the default major changes: prompt per branch, never automatic (default)? | NF1 Task 39 |
| Q729 | arm64 compute images: build them for every supported major (default: yes; doubles build time), and amd64 only for a beta major? | NF1 Task 11 |
| Q730 | Detach `ostrium-labs/loams-postgres` from `neondatabase/neon`'s fork network through GitHub support (default: yes, the owner files it now)? Do the same for `ostrium-labs/postgres` and the small forks (default: no; they stay forks, frozen or tracking `postgres/postgres`)? | Before NF1 Task 3 |
| Q731 | The dev stack's new path: `deploy/loams-postgres-dev` (default), plain `deploy/loams-postgres` (which reads like the product next to `deploy/helm/loams-postgres` and `deploy/loams-postgres-single`), or keep `deploy/neon`? | NF1 Task 1b |
| Q732 | Compatibility names (`neon` extension, `neon.*` GUCs, `neon_superuser`, `NEON_*`, `neon_local`, wire and on-disk names): keep them through NF1 and plan aliases after PG2 GA, one migration task per name (default), or start sooner? | After NF1 |
| Q733 | The GitHub App `loams-postgres-bot`: may the controller create it through GitHub's App manifest flow once the owner approves (default), or does the owner create it? Either way the owner installs it on the organisation. Until then, the owner refreshes the `gh` token with `write:packages`, `read:packages` and `admin:org` for Tasks 1 and 5 | NF1 Task 1 |
| Q734 | Staffing for the hard fork: one full-time maintainer plus a second at about 0.25 FTE, about 1.25 FTE in full mode (default, D824)? Who are they (with Q722)? | NF1 Task 5 |

## 17. Contradictions with earlier decisions, and how they are resolved

| Earlier | Conflict | Resolution |
|---|---|---|
| D241: quarterly rebase; support 17 and 18, 16 until EOL; images within a week | D802 merges, never rebases; D811 sets weekly, nightly and monthly syncs; D813 sets the majors | **Superseded** by D810–D814. D241's "within a week" survives as D805's 7-day target; its estimates are restated in §13 |
| §28 §10 step 3: publish to `ghcr.io/dina-kar/compute-node-v17` | The organisation is `ostrium-labs` | **Superseded:** `ghcr.io/ostrium-labs` (D807) |
| D231: "nothing is posted upstream" | Q716 asks to file one licence issue upstream; §12.4 may report PgDog gaps upstream | **Kept**, with owner-approved exceptions only (Q716; PgDog under §46 §8.3) |
| D716: "Fork releases follow D241"; major upgrades by dump and restore | D810, D811; D815 | **Amended:** releases per D810; upgrades through `fast_import` and import first, dump and restore as the fallback |
| PG2 Task 0 ruling 7 (Q649 default: 17 only at GA) | D813 | **Answered:** 17 at GA; 18 when NF1e passes; PG2 does not wait |
| PG2 R2.1: Neon's images at `ghcr.io/neondatabase` | D806, D818 | **Amended:** the mirror, then our own builds, through the pin file |
| D802, D811: `main` mirrors `neondatabase/neon`; weekly sync | The owner's hard-fork decision | **Superseded** by D820 and D821: `main` is ours; no Neon sync; Postgres minors tracked directly |
| D819: 0.6–0.9 FTE ongoing, MVM 0.2 | No upstream fixes after the hard fork | **Superseded** by D824: 0.7–1.1 FTE, MVM 0.25–0.3 |
| Q723: keep `neon` package names | The owner's "let it be called loams-postgres" | **Answered** by D822 |
| PG2 R31.12 trimmed CDDL from the decoder only | The shipped images still contain `inferno` | **Extended:** D808 removes it from every binary we build |
| Q110: base the catch-up on the `REL_1x_STABLE_neon` heads or on `main`'s pins | §6.3 | **Answered:** the `_neon` heads (17.8, 16.12), then upstream's latest minors |

## 18. Sources

- Loams: §28 §10–§11; §46 (all); the PG2 plan, Task 0 rulings, Task 2 rulings R2.1–R2.10 (branch `backend/pg2`) and Task 31 rulings; `crates/loams-wal-decoder/{Cargo.toml,deny.toml}`; `deploy/neon/compose.yaml`; `apps/desktop-electron/src/main/stacks/stacks.ts`; `.github/workflows/dco.yml`; `SECURITY.md`; `NOTICE`. Read on 2026-10-09.
- Neon, at `1218fb7a` (`~/Documents/Ostriumlabs/neon`): `Cargo.toml` (git dependencies, `pprof` features), `Cargo.lock` (791 packages), `deny.toml` (allows CDDL-1.0), `NOTICE`, `CODEOWNERS`, `.gitmodules`, `vendor/revisions.json`, `Dockerfile` (lines 59–124), `compute/compute-node.Dockerfile` (98 stages), `.github/workflows/build_and_test.yml`, `.github/actions/prepare-for-subzero`, `proxy/Cargo.toml`, `libs/pageserver_api/src/models.rs` (`ImportPgdata`), `libs/postgres_versioninfo`, `libs/postgres_ffi`, `pageserver/src/virtual_file/io_engine.rs`, `compute_tools/src/bin/fast_import.rs`, `docs/core_changes.md`; `git log 77e22e4b..fa504217c`.
- GitHub metadata (`gh api`, 2026-10-09): the eight `ostrium-labs` forks (default branches, licences, branches, tags, Actions state); `neondatabase/{subzero,flux-fleet,cloud,infra}` (404); `neondatabase/postgres` branches; `postgres/postgres` tags (`REL_18_6`, `REL_17_11`, `REL_16_15`, `REL_19_BETA4`); `neondatabase/tokio-epoll-uring` `Cargo.toml` files and commit history; `neondatabase/pgrag` `LICENSE`.
- Postgres 19's RC1 and GA dates and the EOL dates are from the owner's brief and the PostgreSQL versioning policy (verify on the day).
