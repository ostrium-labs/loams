# NF1 — Loams Postgres: Our Hard Fork of Neon, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact names, paths, branches, tags or defaults, use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Planned** (2026-10-09; **amended the same day for the hard fork**, see below). Track NF1, design [§51](../design/51-loams-postgres-fork.md) (D800–D824, Q715–Q734), an addendum to [§28](../design/28-loams-postgres.md) §10 and [§46](../design/46-loams-postgres-production.md). **Owner decision, 2026-10-09: "Mirror and fully fork neon postgres all repos we will maintain it add extension new pg 19 support etc."** NF1 supersedes D241's cadence and answers Q649. It amends [PG2](2026-10-08-pg2-postgres-production.md) Task 0 ruling 7, Task 2 ruling R2.1, Task 51 and Task 55 (§51 §12.5). **Postgres 19 is beta in every Loams artifact and document until Task 35's gate passes**; upstream's GA is 2026-10-29.
>
> **Amended 2026-10-09 for the hard fork (D820–D824).** Owner decision: **"let the neon fork be hard fork, neon stopped open source after databricks acquisition and went silent over a repo, replace with our loams, let it be called loams-postgres not loams-neon."** The repository `ostrium-labs/neon` is now `ostrium-labs/loams-postgres` (renamed by the controller; GitHub redirects the old URL; tag `loams-decoder-trim-1` kept). What changed in this plan:
> - **No upstream sync.** Task 19 (the weekly sync) is removed, and NF1c is now "Postgres minors and the gate". `main` is ours: there is no `loams/main`. `fa504217` is tagged `neon-fork-point`.
> - **New Task 1b**: rename the crate `loams-neon` → `loams-postgres`, the decoder's git URL, `deploy/neon` → `deploy/loams-postgres-dev`, and their references (D823).
> - **Names** (D822): images `loams-postgres-storage`, `loams-postgres-compute-v<N>`, `loams-postgres-build-tools`; Neon's mirrored builds under `neon-archive/`; release tags `lp-YYYY.MM.N`; patch ids `LP-<nnnn>`; pin file `deploy/pins/loams-postgres-images.toml`; bot `loams-postgres-bot`; team `loams-postgres-maintainers`. Compatibility names (`neon` extension, `neon.*` GUCs, wire and on-disk formats) are unchanged.
> - **Security** is ours alone: Task 6 adds the fuzz job. **Staffing** is re-costed (D824, §51 §13).
> - **Still blocked on the owner:** the `gh` token has `repo` and `workflow` only (no `write:packages`, no `admin:org`); the GitHub App; detaching the fork network (Q730, Q733).

**Goal:** Ostrium Labs runs Loams Postgres, a hard fork of Neon, as a maintained product:
- every fork inventoried, licensed, protected and patch-tracked, and renamed where it is ours;
- Neon's last public images mirrored with unchanged digests;
- our own signed, attested, multi-arch images built from `ostrium-labs/loams-postgres` and pinned by Loams by digest;
- Postgres minors within 7 days, behind a test gate;
- a licence-checked extension catalogue with a Loams extension;
- Postgres 18 supported, and Postgres 19 in beta then supported;
- a major-upgrade path for Neon branches;
- private dependencies and CDDL removed, and `tokio-epoll-uring`'s licence resolved;
- a costed staffing model with a minimum viable maintenance mode.

The exit is the checklist at the end of this plan.

**Architecture** (§51 §3, §5, §12):
- **Three kinds of repositories:**
  - `ostrium-labs/loams-postgres` holds the code we build and **all CI** (`.github/workflows/loams-*.yml` on `main`);
  - `ostrium-labs/postgres` holds branches only (`loams/REL_<N>_STABLE`);
  - `ostrium-labs/loams` (this repository) holds the pin file, the consumers, the verification and the integration tests.
- **Flow:** a PR into `main` behind the gate (and, for Postgres, `postgres/postgres` minors merged into `loams/REL_<N>_STABLE`) → release `lp-YYYY.MM.N` → images by digest, signed and attested → a bot PR here updates `deploy/pins/loams-postgres-images.toml` → every consumer is regenerated from it and `cosign verify` runs.

**Tech stack:**
- GitHub Actions; Docker Buildx with BuildKit (registry cache, `provenance: mode=max`, `sbom: true`); `skopeo` (run through podman, pinned by digest); `cosign` (keyless, Sigstore); `oras`; `actions/attest-build-provenance` and `actions/attest-sbom`; Syft; Trivy; `rust-audit-info` (cargo-auditable); `cargo deny`; `cargo about`; `askalono`; `actionlint`; Python 3 (stdlib only) for scripts; `gh`.
- The fork's toolchain is its own (`rust-toolchain.toml`, 1.88.0 at `fa504217`); Loams' crates stay on 1.97.
- Postgres 16, 17, 18 and 19 trees from `ostrium-labs/postgres`.

**Spec:**
- [§51](../design/51-loams-postgres-fork.md) (all), and D800–D824, Q715–Q734 in the [decision log](../design/13-decision-log.md).
- [§28](../design/28-loams-postgres.md) §10 (the fork's history and estimates).
- [§46](../design/46-loams-postgres-production.md) §8.3 (the PgDog licence boundary), §9 (`loams-wal`), §12 (upgrades), §16.1 (extensions), §18 (the desktop contract).
- [PG2](2026-10-08-pg2-postgres-production.md): Task 2 and its rulings R2.1–R2.10 (branch `backend/pg2`), Task 31 rulings (R31.3, R31.6, R31.12), Tasks 51, 55 and 58.

## Global Constraints

- **Repositories, worktrees and branches.**
  - Loams: work in `~/Documents/Ostriumlabs/loams-wt/nf1-<milestone>`, one branch per milestone (`feat/nf1a-governance`, `feat/nf1b-builds`, `feat/nf1c-minors`, `feat/nf1d-extensions`, `feat/nf1e-pg18`, `feat/nf1f-pg19`, `feat/nf1g-integration`), each from `dev`, with stacked PRs into `dev`.
  - The `loams-postgres` repository: worktrees of the local clone `~/Documents/Ostriumlabs/neon` (its `origin` is now `ostrium-labs/loams-postgres`; the directory keeps its name unless the owner moves it) under `~/Documents/Ostriumlabs/neon-wt/<topic>`, on `loams/<topic>` branches, with PRs into `main`. Until the fork network is detached (Q730), every `gh pr create` names `--repo ostrium-labs/loams-postgres --base main`, or GitHub offers `neondatabase/neon` as the base.
  - The `postgres` fork: **ask the owner before cloning** it (Task 0). Once approved, clone blobless (`--filter=blob:none`) to `~/Documents/Ostriumlabs/postgres`, with worktrees under `~/Documents/Ostriumlabs/postgres-wt/<topic>`.
  - Never use `git stash`.
- **Commits.** `git commit -s` (DCO) everywhere. Every commit in a fork after its fork point carries a `Loams-Patch: LP-<nnnn>` trailer (§51 §3.4). Commit areas: here `deploy`, `ci`, `wal`, `pg` (which replaces the old `neon` area), `desktop`, `docs`; in the fork `loams-build`, `loams-ci`, `loams-licence`, `loams-pg18`, `loams-pg19`, `loams-ext`, `loams-rebrand`, and Neon's areas (`pageserver`, `compute`, …) for code.
- **Never rewrite fork history.** No force-push or rebase on `loams/*` or `main`, and no deletion of a `loams-*`, `lp-*` or `neon-fork-point` tag. Postgres minors come in by merge (D821). Nothing is merged from `neondatabase/*`; an upstream commit, if any, is cherry-picked by hand as an `LP-` patch (D820).
- **Rust builds.** Loams crates use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR` and never build in `/tmp`. Run one cargo build at a time. Build touched crates only (`cargo test -p loams-wal-decoder`). **The Neon workspace is not built locally in full:** local work is `cargo check -p <crate>` on the crates touched, with headers from `scripts/pg2/pg-headers.sh`. Images, `neon_local` and `test_runner` run in the fork's CI.
- **No container builds on the build machine** while a cargo build runs. Image builds happen in CI. The mirror (Task 1) is a registry-to-registry copy, with no local build.
- **Pins.**
  - Every image is referenced by digest; every git dependency by `rev` with a `loams-*` tag on it; every downloaded source by sha256.
  - A new dependency, action or extension version must be at least 14 days old, except security fixes. Record each pin in the commit message.
  - Actions are pinned by commit SHA.
- **Licences.** No CDDL in any binary or image we publish (owner ruling, 2026-10-08). No AGPL, SSPL, BUSL, ELv2, TSL, Commons Clause or unlicensed code in an image. GPL components only with an attached source artifact (§51 §3.6). PgDog stays unmodified and outside every image (§46 §8.3).
- **Names** (D822). New things we own use the Loams Postgres names (§51 §1a.1). Compatibility names (the `neon` extension, `neon.*` GUCs, `neon_superuser`, `NEON_*`, `neon_local`, wire protocols and on-disk formats) are never renamed in NF1; Q732 plans that later.
- **No `neondatabase/*` fetch at build time** after Task 7: no git dependency, Dockerfile download, action or registry from that organisation. `scripts/loams/check-no-private-sources.sh` enforces it.
- **Owner actions are named, not assumed.** Creating teams, the GitHub App, package visibility changes, billing and counsel are listed in the tasks that need them. A task that needs one records "waiting on owner" in its rulings and continues with what does not depend on it.
- **Postgres 19 is labelled beta** in every image tag (`-beta`), pin entry (`beta = [19]`), compose profile (`beta`) and document until Task 35.

## Review Focus

1. **A mirrored digest differs from its source**, or a mirror is published before its audit. Expected: never. Tests: Task 1 `mirror_digests_match`, `audit_runs_before_visibility`.
2. **A published image contains CDDL code or code from a private repository.** Expected: never. Tests: Task 7 `deny_rejects_cddl`, `no_private_sources`; Task 10 `storage_image_audit_clean`; Task 11 `compute_image_audit_clean`; Task 12 `sbom_has_no_forbidden_licence`.
3. **Loams pins an image that is unsigned, unattested, or signed by another identity.** Expected: CI fails. Tests: Task 12 `unsigned_digest_fails_verify`, `wrong_identity_fails_verify`; Task 2 `pins_match_consumers`.
4. **A pinned fork revision becomes unreachable.** Expected: CI fails before merge. Tests: Task 3 `pinned_revs_reachable`, `force_push_rejected` (branch protection probe).
5. **A patch enters `main` untracked.** Expected: the gate fails. Tests: Task 4 `unlisted_patch_fails`, `trailer_without_row_fails`.
6. **A Postgres security minor misses the 7-day target unnoticed.** Expected: an issue with a due date opens on release day and escalates. Tests: Task 6 `pg_security_release_opens_issue`, `overdue_issue_escalates`; Task 18 `new_minor_tag_starts_release`.
7. **A WAL record of a new major is decoded wrongly or skipped silently.** Expected: an unknown record or magic is an error; every record type has a fixture. Tests: Task 25 `unknown_page_magic_is_error`; Task 26 `decode_all_rmgrs_pg18`; Task 29 `golden_pg18_matches_fork_sender`; Task 33 `decode_all_rmgrs_pg19`.
8. **A compute of one major attaches to a timeline of another.** Expected: refused. Tests: Task 37 `compute_major_mismatch_refused`; Task 39 `local_stack_starts_timeline_major_only`.
9. **A major upgrade loses or corrupts data, or runs without the user asking.** Expected: never. Tests: Task 37 `upgrade_17_to_18_preserves_checksums`, `upgrade_failure_keeps_source_serving`; Task 39 `upgrade_never_automatic`.
10. **An extension with a forbidden licence, or a source with the wrong hash, enters the image.** Expected: the build fails. Tests: Task 20 `forbidden_licence_rejected`, `manifest_matches_dockerfile`, `sha_mismatch_fails`.
11. **The rename loses a user's local data or breaks a pinned build.** Expected: never. Tests: Task 1b `compose_project_name_unchanged`, `decoder_golden_unchanged_after_url_move`, `no_old_fork_url_left`.
12. **A security finding in Neon's own code waits for an upstream that will not come.** Expected: our fix within D805's targets. Tests: Task 6 `fuzz_crash_opens_issue`, `due_dates_follow_targets`.

---

## File structure

```
ostrium-labs/loams-postgres (branch main; tag neon-fork-point)
  LOAMS.md, README.md                        Task 3   charter, hard-fork and independence statement, release map
  LOAMS_PATCHES.md                           Task 4   the patch inventory
  NOTICE, SECURITY.md, CODEOWNERS            Task 5
  deny.toml                                  Task 7   CDDL-1.0 removed
  Cargo.toml                                 Task 7   pprof/jemalloc_pprof without flamegraph; git deps -> ostrium-labs by rev
  Dockerfile, compute/compute-node.Dockerfile   Tasks 7, 9, 11, 21, 28, 34
  build-tools/Dockerfile                     Task 9
  compute/loams-extensions.toml              Task 20
  pgxn/loams/                                Task 22
  vendor/postgres-v18, vendor/postgres-v19, vendor/revisions.json, postgres.mk, Makefile   Tasks 17, 24, 31
  libs/postgres_versioninfo, libs/postgres_ffi, libs/wal_decoder   Tasks 25, 26, 33
  pageserver/src/{walingest.rs,basebackup.rs,import_datadir.rs,walredo*,pgdatadir_mapping.rs}   Tasks 26, 33
  pgxn/{neon,neon_rmgr,neon_walredo}/        Tasks 27, 33
  compute_tools/                             Tasks 27, 33, 37
  docs/loams/{release.md,extensions.md,pg18-format-diff.md,pg19-format-diff.md,LICENSING-tokio-epoll-uring.md}
  fuzz/                                      Task 6   cargo-fuzz targets (WAL decoder, page service, proxy protocol)
  scripts/loams/{check-patches.py,check-no-private-sources.sh,strip-upstream-ci.sh,check-extensions.py,
                 postgres-minors.py,release-notes.py,audit-image.sh,sla.py}
  .github/workflows/loams-{patches,gate,build-tools,build-storage,build-compute,release,
                           postgres-minors,extensions-update,advisories,fuzz,rebuild-diff}.yml
  .github/workflows/<upstream>.yml           Task 7   deleted (LP-0002)
ostrium-labs/postgres                        branches loams/REL_16_STABLE … loams/REL_19_STABLE (Tasks 15, 17, 24, 31, 32)
ostrium-labs/tokio-epoll-uring (loams/main)  LICENSE-MIT, LICENSE-APACHE, uring-common/LICENSE-tokio-uring, LICENSING.md   Task 8
ostrium-labs/pg_session_jwt, rust-postgres, azure-sdk-for-rust, framed-websockets, autoscaling   Task 3 (branches, protection, archive)

ostrium-labs/loams (this repository)
  crates/loams-postgres/ (was crates/loams-neon)   Task 1b   the rename; Tasks 29, 37 below
  deploy/pins/loams-postgres-images.toml     Tasks 2, 14 (and every release after)
  deploy/pins/audit/<digest>.json            Tasks 1, 10, 11 (cargo-auditable package lists)
  scripts/nf1/{mirror-images.sh,pins.py,check-pinned-revs.sh,verify-pins.sh}
  deploy/loams-postgres-dev/{compose.yaml,README.md}   Task 1b (from deploy/neon); Tasks 1, 2, 14, 39
  deploy/loams-pg-bench/compose.yaml         Tasks 2, 14
  .github/workflows/{pg2.yml,pg2-e2e.yml,loams-pg-bench.yml}   Task 1b (names); Task 2 (PG_HEADERS_IMAGE from the pin file)
  .github/workflows/nf1-pins.yml             Tasks 2, 12
  .github/workflows/nf1-candidate.yml        Task 16 (the gate's G3, dispatched by the fork)
  crates/loams-wal-decoder/{Cargo.toml,Cargo.lock,deny.toml,tests/fixtures/}   Task 1b (fork URL); Tasks 3, 29, 34
  NOTICE, apps/desktop-electron/{electron-builder.config.cjs,src/main/stacks/stacks.ts}   Task 1b
  crates/loams-safekeeper/                   Task 29 (version handling, if any change is needed)
  crates/loams-postgres/src/{majors.rs,pageserver.rs}   Tasks 29, 37
  scripts/pg2/pg-headers.sh                  Task 29 (v18, v19)
  conformance/router/pgdog-pg{17,18,19}.tsv  Task 36
  apps/desktop-electron/src/main/stacks/stacks.ts, …/sql/backend/local-stack.ts   Task 39 (after PG2 Task 58)
  docs/runbooks/loams-postgres-fork/{release.md,minor-release.md,security.md,new-major.md,mvm.md}   Task 40
  docs/design/51-loams-postgres-fork.md, 13-decision-log.md, README.md; docs/plans/README.md   Tasks 0, 41
```

## Shared contracts (all tasks use these names)

### The pin file: `deploy/pins/loams-postgres-images.toml`

```toml
# Written by scripts/nf1/pins.py (set); checked by pins.py (check). Do not edit by hand.
schema = 1
fork_release = "lp-2026.10.0"            # or "mirror-77e22e4b" while only mirrored images are pinned
fork_commit = "<40-hex>"                 # ostrium-labs/loams-postgres main commit (or the upstream commit for a mirror)

[majors]
supported = [17]                          # Postgres majors new projects may use
beta = []                                 # need allow_beta_major; 19 goes here first
maintenance = [16]                        # minors and exports only, no new projects
default = 17                              # for new projects and new desktop tenants

[images.storage]
repository = "ghcr.io/ostrium-labs/loams-postgres-storage"
digest = "sha256:<64-hex>"                # multi-arch index
source = "ostrium-labs/loams-postgres@<40-hex>"   # or "mirror:ghcr.io/neondatabase/neon@sha256:<64-hex>"
pg_minors = { "16" = "16.9", "17" = "17.5" }

[images.compute-v17]
repository = "ghcr.io/ostrium-labs/loams-postgres-compute-v17"
digest = "sha256:<64-hex>"
source = "ostrium-labs/loams-postgres@<40-hex>"
pg_minor = "17.5"
```

`pins.py check` fails unless:
- every consumer's default equals the file (the `x-neon-image` and `x-compute-image` anchors of both compose files, names kept for compatibility (D823), `PG_HEADERS_IMAGE` in three workflows, and later the single-node compose and the Helm values);
- every `digest` is 64 hex characters;
- a `source` is either a fork commit or a `mirror:` reference.

`pins.py set --from-release <url>` rewrites the file and all consumers. `pins.py rust` writes `crates/loams-postgres/src/majors.rs` (`SUPPORTED`, `BETA`, `MAINTENANCE`, `DEFAULT`).

### Fork refs

| Ref | Rule |
|---|---|
| `main` (loams-postgres) | **Ours.** Default branch and trunk; PRs only; required checks `loams-gate / g0`, `g1`, `g2` (when triggered), `dco`, `patches` |
| Tag `neon-fork-point` (loams-postgres) | `fa504217`, Neon's last public commit; the base of `check-patches.py` and the DCO check |
| `loams/<topic>` | Work branches |
| `loams/REL_<N>_STABLE` (postgres) | Ours, per major, taking `postgres/postgres` minors directly; `REL_<N>_STABLE_neon` is frozen history |
| Tags `loams-<YYYYMMDD>-<n>` | Any revision pinned outside a release |
| Tags `lp-YYYY.MM.N` | Fork releases; immutable |

### Patch trailer and inventory

- The trailer is `Loams-Patch: LP-0007`. Merges of upstream Postgres in `ostrium-labs/postgres` carry `Loams-Sync: <tag or sha>`. A hand-picked Neon commit carries its own `LP-` id and a `(cherry picked from neondatabase/neon <sha>)` line.
- `LOAMS_PATCHES.md` rows: `| LP-0007 | Licence texts for tokio-epoll-uring | licence | @owner | 2026-10-20 | loams-only | Neon adds LICENSE files upstream |` (columns: id, title, area, owner, since, origin, drop when).

### Image tags

- `loams-postgres-storage`: `<sha12>`, `lp-YYYY.MM.N`, `edge`.
- `loams-postgres-compute-v<N>`: `<pgminor>-<sha12>`, `<pgminor>-lp-YYYY.MM.N`, `edge`; a beta major adds `-beta` to each, plus `beta`.
- `loams-postgres-build-tools`: `<sha12>`.
- Mirror (Neon's own builds, Neon's names): `neon-archive/<name>:mirror-77e22e4b`.
- Loams pins digests only.

### Extension manifest: `compute/loams-extensions.toml`

```toml
schema = 1
[[extension]]
name = "pgvector"                  # the CREATE EXTENSION name(s) in `provides`
provides = ["vector"]
stage = "pgvector"                 # Dockerfile stages <stage>-src and <stage>-build
version = "0.8.1"                  # per-major overrides in [extension.majors."16"] when the Dockerfile's case block differs
source = "https://github.com/pgvector/pgvector/archive/refs/tags/v0.8.1.tar.gz"
sha256 = "<64-hex>"
license = "PostgreSQL"             # SPDX; checked against the tarball and the allowlist
tier = "core"                      # core | included | dropped
majors = [16, 17, 18, 19]
trusted = true                     # neon_superuser may CREATE EXTENSION
preload = false                    # needs shared_preload_libraries
```

The licence allowlist: `PostgreSQL`, `MIT`, `BSD-2-Clause`, `BSD-3-Clause`, `Apache-2.0`, `ISC`, `Zlib`, `MPL-2.0`, `LGPL-2.1-or-later`, and `GPL-2.0-or-later` and `GPL-3.0-or-later` (the latter two only with `sources = true`).

### cosign verification identity

```
--certificate-oidc-issuer https://token.actions.githubusercontent.com
--certificate-identity-regexp '^https://github.com/ostrium-labs/loams-postgres/\.github/workflows/loams-(release|sign)\.yml@refs/heads/main$'
```

Signing runs in the reusable workflow `loams-sign.yml`, called by `loams-release.yml`. Fulcio puts the called workflow's `job_workflow_ref` in the certificate (verify in Task 12), which is why the regex accepts both names. Only `loams-release.yml` and `loams-build-tools.yml` call it; `edge` builds are unsigned and never pinned.

---

## Execution order

1. Task 0.
2. **NF1a** (Tasks 1–6, and 1b). Task 1 first: the mirror protects against Neon deleting its packages. Task 1 is waiting on the owner's `write:packages` token (Q733), so **Task 1b runs as soon as Task 0 is done**, before Task 2 writes the pin file's consumers; Task 1 then writes into `deploy/loams-postgres-dev/`.
3. **NF1b** (Tasks 7–14), after Task 4. Task 14 (the parity release and the switch to our images) is the milestone's exit.
4. **NF1c, Postgres minors and the gate** (Tasks 15–18; Task 19 removed by D820) after Task 13. **Task 17 (the catch-up to 17.11) is urgent**: it must ship by 2026-11-19 at the latest, with 17.12 (released 2026-11-12) merged.
5. **NF1d** (Tasks 20–23) after Task 11. Task 21 answers Q651 for PG2 Task 55.
6. **NF1e** (Tasks 24–30) after Tasks 16 and 17.
7. **NF1f**: Task 31 starts on 2026-10-15 (Postgres 19 RC1), beside everything else. Tasks 32–35 follow Task 27.
8. **NF1g**: Task 36 with Task 28. Task 37 after Task 28 and PG2 Task 2. Task 38 after Task 37. Task 39 after PG2 Task 58. Tasks 40 and 41 last.

Target dates (estimate, about 1.25 engineers in full mode, D824, §51 §8.5 and §13):

| Milestone | Window |
|---|---|
| NF1a | 2026-10-12 → 10-20 (Task 1b first; Task 1 when the token exists) |
| NF1b | 10-19 → 11-06 |
| NF1c | 10-26 → 11-19 |
| NF1d | 11-09 → 12-04 |
| NF1e | 11-23 → 2027-01-22 |
| NF1f | port 2027-01 → 03; beta images 2027-02; GA no earlier than 2027-03 |
| NF1g | alongside, ending 2027-03 |

---

### Task 0: Reconcile with the repositories as they are

**Files:** this plan's "Rulings made during execution" only.

Steps:
1. Answer each of the following and record the answer, with paths, commits or `gh api` output, as a ruling:
   - Has PG2 Task 2 (branch `backend/pg2`: pinned digests in `deploy/loams-postgres-dev`, `deploy/loams-pg-bench` and three workflows) merged into `dev`? If not, Tasks 2 and 14 build on `backend/pg2` and say so in their PRs.
   - Is §51 §14's as-built table still true? Check `ostrium-labs/loams-postgres`'s default branch, branches, tags and active workflows; `ostrium-labs/postgres`'s `REL_*_neon` branches; the eight forks' licences.
   - **`tokio-epoll-uring`'s licence.** The brief said "no licence file and no Cargo licence field". §51 §11 found `license = "MIT OR Apache-2.0"` in `tokio-epoll-uring/Cargo.toml` and `"MIT"` in `uring-common/Cargo.toml`, at `main` and at `781989bb`, and no licence file. Re-check and record.
   - Upstream Postgres: the latest minors (expected 18.6, 17.11, 16.15), `REL_19_STABLE`'s latest tag, and the RC1 and GA dates from postgresql.org.
   - Owner approval to clone `ostrium-labs/postgres` (memory rule: never clone without asking). Until it is given, Tasks 15, 17, 24, 31 and 32 work through `gh api` and the fork's CI only.
   - Owner actions outstanding: the `loams-postgres-bot` GitHub App; teams `loams-postgres-maintainers`, `postgres-maintainers` and `security`; a token with `write:packages` for the mirror; package visibility (Q715); the runner budget (Q721).
   - Are D800–D819 and Q715–Q729 still free in `docs/design/13-decision-log.md`?
   - *(Added by the hard-fork amendment; recorded as Task 0 addendum rulings.)* The rename of `ostrium-labs/neon` to `ostrium-labs/loams-postgres`, the redirect, the tag `loams-decoder-trim-1`, the fork-network state, and the token's scopes.
2. Commit `docs(nf1): task 0 rulings`.

## NF1a — Governance and the mirror (Tasks 1–6)

### Task 1: Mirror Neon's last public images, and audit them

**Files:** create `scripts/nf1/mirror-images.sh`, `scripts/nf1/audit-image.sh`, `deploy/pins/audit/<digest>.json` (one per mirrored index) and `scripts/nf1/tests/test_mirror.py`. Modify `deploy/loams-postgres-dev/README.md` (a "Mirror" section; Task 1b has moved the directory from `deploy/neon`).

**Interfaces:**
- `mirror-images.sh [--dry-run] <src-ref@digest> <dst-repo> <tag>` runs `podman run --rm quay.io/skopeo/stable@sha256:<pinned> copy --all --preserve-digests --retry-times 3 docker://<src> docker://<dst>:<tag>`, with credentials from `REGISTRY_AUTH_FILE`. It then verifies: `skopeo inspect --raw` of source and destination are byte-identical for the index and for every child manifest digest it lists.
- `audit-image.sh <ref@digest>` extracts each binary that has a `.dep-v0` section, for every platform, with `rust-audit-info`. It writes `{binary: [{name, version, source}]}` and exits 3 when it finds a forbidden licence package (`inferno`, or any crate whose licence in the embedded data or the crates.io index is CDDL) or a `neondatabase/subzero` source.

Images:
- `ghcr.io/neondatabase/neon@sha256:7a4f1249…` → `ghcr.io/ostrium-labs/neon-archive/neon:mirror-77e22e4b`;
- `ghcr.io/neondatabase/compute-node-v17@sha256:13ab146d…` → `ghcr.io/ostrium-labs/neon-archive/compute-node-v17:mirror-77e22e4b`;
- Neon's `compute-node-v16` of the same build → `ghcr.io/ostrium-labs/neon-archive/compute-node-v16:mirror-77e22e4b`: find its tag by listing tags whose `neon` label or `compute_ctl --version` names `77e22e4b`, and record the digest;
- Neon's `build-tools`, at the digest the `77e22e4b` build used (from the image's provenance attestation) → `ghcr.io/ostrium-labs/neon-archive/build-tools:mirror-77e22e4b`.

The copies keep Neon's names under `neon-archive/` because they are Neon's binaries, not Loams Postgres (D822). Verify that GHCR accepts the nested path; if not, use `neon-archive-<name>` and record it.

Tests:
- `mirror_digests_match`: runs the verification against a local `registry:2` with a two-platform fixture index (CI).
- `mirror_refuses_unpinned_source`: a tag instead of a digest exits 2.
- `audit_flags_cddl` and `audit_flags_subzero`: fixture `.dep-v0` payloads.
- `audit_runs_before_visibility`: the README's runbook orders "audit" before "make public", and `pins.py check` refuses a `mirror:` source whose audit file is missing.

Steps:
1. Write the tests (FAIL).
2. Write the scripts (PASS).
3. Run the mirror for real and the audits. **Waiting on owner:** the `gh` token has `repo` and `workflow` only; the push needs `write:packages` (the owner runs `gh auth refresh -h github.com -s write:packages,read:packages`) or the bot's token (Q733).
4. Record the findings and both digests per image in the README table and in this plan's rulings.
5. Apply Q715. If the owner has not answered, keep the default: a package with a finding stays private.
6. Commit `deploy(pg): mirror Neon's 77e22e4b images to ghcr.io/ostrium-labs/neon-archive, with audits`.

### Task 1b: Rename `loams-neon` to `loams-postgres`, the fork's URL and the dev stack (D823)

Runs right after Task 0 (Task 1 waits on the owner's token), in one PR on `feat/nf1a-governance`, before Task 2 writes the consumers. Numbered 1b so the other task numbers stay stable.

**Files:**
- `git mv crates/loams-neon crates/loams-postgres`. In its `Cargo.toml`: `name = "loams-postgres"`, and the description "Loams Postgres's client of the storage and compute components (design §46 §6): the pageserver and storage controller, loams-wal, compute_ctl and the compute spec. A plain HTTP client; no Neon code." Every `loams_neon::` path in its tests and `lib.rs` docs. The root `Cargo.lock` package entry (`cargo metadata --locked` after `cargo update -p loams-postgres --offline` or an edit; no version changes).
- `.github/workflows/pg2.yml` and `pg2-e2e.yml`: the path filters `crates/loams-neon/**` and `deploy/neon/**`, `cargo test -p loams-postgres` and `cargo clippy -p loams-postgres`, the job names, and the cache `shared-key` (`pg2-loams-postgres`). The test target `it_deploy_neon` keeps its name.
- `git mv deploy/neon deploy/loams-postgres-dev` (Q731's default; if the owner picks another path, use it). `compose.yaml` **keeps `name: loams-neon`**, with a comment that Docker names users' volumes after it; its header comment names the new path. Update the references in `deploy/loams-pg-bench/{compose.yaml,pageserver.toml,README.md,compute/start.sh}`, `crates/loams-postgres/tests/{it_deploy_neon.rs,fixtures/capture.sh,fixtures/compute-jwt.py,fixtures/README.md}`, `conformance/router/README.md` and `.github/workflows/*`.
- The decoder: `crates/loams-wal-decoder/Cargo.toml` (`git = "https://github.com/ostrium-labs/loams-postgres"` for `pageserver_api`, `postgres_ffi`, `utils` and `wal_decoder`, **the same `rev`, now `80356b2998710722020134d81102d810f921fe6b` (tag `loams-decoder-trim-2`; was `1218fb7a6a37e1b4c268bad5c2952c238d81d368`, tag `loams-decoder-trim-1`)**; the section `[patch."https://github.com/ostrium-labs/loams-postgres"]`), its `Cargo.lock` (regenerated: Cargo treats a new URL as a new source; `cargo update -p pageserver_api -p postgres_ffi -p utils -p wal_decoder` in CI, or locally once the owner allows the fetch), and `deny.toml`'s `allow-git`.
- The desktop: `apps/desktop-electron/electron-builder.config.cjs` (`from: "../../deploy/loams-postgres-dev", to: "stacks/loams-postgres-dev"`), and `src/main/stacks/stacks.ts`: `StackDef` gains `source` (the directory under `deploy/` in dev and under `resources/stacks` when packaged, now `loams-postgres-dev`), while `dir` (the per-user copy under the stacks directory) **stays `neon`**, so existing users keep their copied files and their volumes. Comments in `src/main/sql/{neon.ts,pg.ts}`, `test/sql-integration.test.ts` and `README.md` name the new path. The file `neon.ts` keeps its name for now.
- Loams' `NOTICE`: the Neon entry names `https://github.com/ostrium-labs/loams-postgres` (Loams Postgres, a hard fork of Neon) and `deploy/loams-postgres-dev/`, and still credits Neon by Databricks/Neon Inc. under Apache-2.0.
- Docs: a one-line mapping note at the top of §23, §28, §31, §37 and the RT0–RT2 and AP1e plans where they name `deploy/neon` or `loams-neon`; the PG2 plan and §46 already carry it. Historical rulings are not rewritten.

**Interfaces:**
- No behaviour change. The package's public API, its fixtures and its HTTP requests are byte-identical.
- `scripts/nf1/check-names.sh`: fails if `crates/loams-neon`, `-p loams-neon`, `deploy/neon/` or `github.com/ostrium-labs/neon"` appears outside `docs/` history sections and the allowlisted compatibility names (`name: loams-neon`, `dir: "neon"`, `stacks/neon`). It runs in `nf1-pins.yml`.

**Name clashes (checked 2026-10-09; recheck in this task):** no crate, package or directory called `loams-postgres` exists (`crates/*` is the workspace; `crates/loams-wal-decoder` is excluded). `loams-pg-control`, `loams-pgdog-sync` and `deploy/loams-pg-bench` keep their `loams-pg-` prefix. The desktop's `web/plugins/postgres` (`@loams/plugin-postgres`) and stack id `postgres` are TypeScript names. `proto/loams/postgres/v1` generates `loams_proto::postgres`, a module, not a crate. `deploy/helm/loams-postgres` and `deploy/loams-postgres-single` are why the stack is not plain `deploy/loams-postgres`.

Tests:
- `cargo test -p loams-postgres --locked` and `cargo clippy -p loams-postgres --all-targets --locked -- -D warnings` (the fixture tests, unchanged).
- `decoder_golden_unchanged_after_url_move`: `cargo test --locked` in `crates/loams-wal-decoder` with the new URL; the golden fixtures are byte-equal, and `cargo tree` shows `git+https://github.com/ostrium-labs/loams-postgres?rev=1218fb7a…`.
- `no_old_fork_url_left`: `check-names.sh` passes, and fails on a fixture that names the old crate.
- `compose_project_name_unchanged`: `docker compose -f deploy/loams-postgres-dev/compose.yaml config` reports the project `loams-neon`, so the volume names are those of today.
- `copied_stack_dir_unchanged`: a desktop unit test; `STACKS.postgres.dir` is `"neon"` and the source resolves to `deploy/loams-postgres-dev` in dev and `resources/stacks/loams-postgres-dev` when packaged.
- `pg2.yml` and `pg2-e2e.yml` green.

Commit `pg: rename loams-neon to loams-postgres; deploy/neon to deploy/loams-postgres-dev; the fork's new URL`.

### Task 2: The pin file, and every consumer generated from it

**Files:** create `deploy/pins/loams-postgres-images.toml`, `scripts/nf1/pins.py`, `scripts/nf1/tests/test_pins.py` and `.github/workflows/nf1-pins.yml`. Modify `deploy/loams-postgres-dev/compose.yaml`, `deploy/loams-pg-bench/compose.yaml`, `.github/workflows/{pg2.yml,pg2-e2e.yml,loams-pg-bench.yml}` and `deploy/loams-postgres-dev/README.md`.

**Interfaces:**
- The pin file as in the shared contract. Its first content is the **mirror** for each package Task 1 made public, and Neon's original reference (`source = "mirror:…"` pointing at `ghcr.io/neondatabase`, repository unchanged) for each package that stays private.
- `pins.py check | set | rust`.
- `nf1-pins.yml` runs `pins.py check` on every PR that touches the file or a consumer. Task 12 adds `verify-pins.sh`.

Tests:
- `pins_match_consumers`: a consumer edited by hand fails.
- `every_pin_has_digest`
- `set_rewrites_all_consumers`: golden files.
- `override_envs_still_work`: `NEON_IMAGE` and `COMPUTE_IMAGE` still override the compose defaults (R2.1).
- `pg2-e2e.yml` green on the new defaults. The digests are unchanged, so the behaviour is too.

Commit `deploy(pg): one pin file for the Loams Postgres images; consumers generated from it`.

### Task 3: Fork settings, branch layout and reachable pins

**Files:**
- In `ostrium-labs/loams-postgres` (branch `loams/nf1-governance`): `LOAMS.md`, and a `README.md` banner: Loams Postgres, a hard fork of Neon at `fa504217`, independent of and not endorsed by Neon Inc. or Databricks (§51 §1a, §3.6).
- Here: `scripts/nf1/check-pinned-revs.sh`, `scripts/nf1/tests/test_pinned_revs.py`, and a job in `nf1-pins.yml`.

**Interfaces:**
- **`loams-postgres`** (D821; no `loams/main` is created):
  - tag `neon-fork-point` (annotated: "Neon's last public commit; Loams Postgres is a hard fork from here") on `fa504217`;
  - fast-forward `main` to `loams/decoder-trim` (`1218fb7a` is one commit on `fa504217`, so `1218fb7a` becomes reachable from `main`); `main` stays the default branch;
  - protect `main` and `loams/*` as in the shared contract;
  - tag `loams-20261012-1` on `1218fb7a`;
  - Q730: until the fork network is detached, record in `LOAMS.md` that PRs must target `ostrium-labs/loams-postgres:main`.
- **The other forks:** create `loams/<upstream branch>` and protect it. Nothing syncs them (D821). `autoscaling` and `framed-websockets` are archived once their `loams/` branch exists (`azure-sdk-for-rust` stays unarchived, frozen).
- **`postgres`:** disable Actions (`PUT /repos/ostrium-labs/postgres/actions/permissions` `{enabled:false}`). Protect `REL_*_STABLE_neon` (frozen history) and `loams/*`.
- `check-pinned-revs.sh` finds every `github.com/ostrium-labs/<repo>` rev in any `Cargo.toml` here. For each, it asks `gh api repos/ostrium-labs/<repo>/compare/<rev>...main` for `loams-postgres` (or the repository's `loams/` branch for the others). It accepts both `github.com/ostrium-labs/neon` and `github.com/ostrium-labs/loams-postgres` (GitHub redirects the old name), so it works on either side of Task 1b. The status must be `behind` or `identical`, or the rev must be the target of a `loams-*` tag.

Tests:
- `pinned_revs_reachable`: today's decoder rev passes after the fast-forward.
- `unreachable_rev_fails`: a fixture rev.
- `force_push_rejected`: a probe push with `--force` to a throwaway `loams/probe` branch under the same protection is refused; the branch is then deleted. The result is recorded.

Owner actions: the teams and the GitHub App (Task 0; Q733); detaching the fork network (Q730). Branch protection needs repository admin, which the `repo` scope gives on `ostrium-labs` repositories (verify); the teams need `admin:org`.

Commits:
- fork: `loams-ci: Loams Postgres charter; main as our trunk` (`Loams-Patch: LP-0000`, the charter's row);
- here: `ci(pg): check that pinned fork revisions stay reachable`.

### Task 4: The patch queue

**Files** (fork, `loams/nf1-governance`): `LOAMS_PATCHES.md`, `scripts/loams/check-patches.py`, `scripts/loams/tests/test_check_patches.py` and `.github/workflows/loams-patches.yml`.

**Interfaces:**
- `check-patches.py --base neon-fork-point --head HEAD` lists the non-merge commits in `neon-fork-point..HEAD`. It fails when a commit has no `Loams-Patch` trailer, a trailer's id has no row, or a row has no commit. In `ostrium-labs/postgres`, merge commits with `Loams-Sync` are exempt and the base is the `_neon` head the branch started from.
- The first rows: LP-0000 (charter), LP-0001 (decoder trim, `1218fb7a`).

Tests:
- `unlisted_patch_fails`
- `trailer_without_row_fails`
- `row_without_commit_fails`
- `neon_history_exempt`: commits before `neon-fork-point` are not checked.
- `postgres_sync_merge_exempt`
- `cherry_pick_needs_origin_line`: an `LP-` row with origin `cherry-pick` needs the `(cherry picked from neondatabase/neon <sha>)` line.

All run on a temporary git repository built in the test.

Commit `loams-ci: the patch queue and its check` (LP-0000).

### Task 5: CODEOWNERS, DCO, SECURITY and NOTICE

**Files:**
- Fork `loams-postgres`: `CODEOWNERS`, `SECURITY.md`, `NOTICE`, `.github/workflows/loams-dco.yml`, `scripts/loams/check-dco.sh` (copied from Loams' `scripts/ci/check-dco.sh`, with the base at `neon-fork-point`).
- `SECURITY.md` and `CODEOWNERS` also go into `postgres` (on `loams/REL_17_STABLE`, created in Task 15; until then on a `loams/meta` branch), `tokio-epoll-uring` and `pg_session_jwt`.

**Interfaces:**
- CODEOWNERS as §51 §3.5.
- `SECURITY.md` points at Loams' policy and private reporting.
- `NOTICE` keeps Neon's lines and appends: "Loams Postgres. Based on Neon by Databricks/Neon Inc. (https://github.com/neondatabase/neon), licensed under the Apache License 2.0. Modifications Copyright 2026 Ostrium Labs and the Loams Authors. Changes from Neon are listed in LOAMS_PATCHES.md."

Tests:
- `codeowners_valid`: `gh api repos/ostrium-labs/loams-postgres/codeowners/errors` returns `[]`.
- `dco_rejects_unsigned`: the copied script's own test.
- `notice_retains_upstream`: the first three lines equal upstream's, and the file contains "Based on Neon by Databricks/Neon Inc."

Owner actions: name the two maintainers (Q722, Q734); create `loams-postgres-maintainers`, `postgres-maintainers` and `security` (needs `admin:org`, which the token lacks).

Commit `loams-licence: CODEOWNERS, DCO, SECURITY, NOTICE` (LP-0000).

### Task 6: Advisories and response targets

**Files** (fork): `.github/workflows/loams-advisories.yml`, `.github/workflows/loams-fuzz.yml`, `fuzz/` (cargo-fuzz targets), `scripts/loams/sla.py`, `scripts/loams/pg-security-feed.py` and `scripts/loams/tests/test_sla.py`.

**Interfaces:**
- **Daily:**
  - `cargo deny check advisories` on `Cargo.lock`;
  - `trivy image --severity HIGH,CRITICAL --format json` on every digest in Loams' pin file (fetched from `ostrium-labs/loams`);
  - `pg-security-feed.py` reads postgresql.org's release feed and opens an issue when a release names security fixes for a supported or maintenance major;
  - GitHub advisories on `pgdogdev/pgdog`, each manifest extension's repository and the small forks' original upstreams; `neondatabase/neon` as a signal only (no fix is expected from it, D820).
- **Nightly fuzzing** (`loams-fuzz.yml`, new with the hard fork: nobody else fuzzes this code): `cargo fuzz` targets for `wal_decoder` record decoding, the pageserver's page-service request parser and the proxy's startup and SCRAM parsers, 30 minutes each on a standard runner. A crash opens a private security issue with the reproducer, labelled for `sla.py`.
- `sla.py` labels issues `sev:critical|high|pg-minor|other` and sets the due date from §51 §3.7's table (Q724). After the due date it escalates: an `@ostrium-labs/security` mention and the `overdue` label.

Tests:
- `pg_security_release_opens_issue`: a fixture feed.
- `due_dates_follow_targets`
- `overdue_issue_escalates`
- `duplicate_finding_not_reopened`
- `fuzz_crash_opens_issue`: a fixture crash artifact opens a private issue with a due date.
- `fuzz_targets_build`: `cargo fuzz build` in CI.

Commit `loams-ci: advisories, fuzzing and response targets` (LP-0008).

## NF1b — Our own builds (Tasks 7–14)

### Task 7: Build hygiene patches

**Files** (fork, `loams/nf1-hygiene`):
- delete `.github/workflows/*.yml` except `loams-*.yml`, and delete `.github/actions/prepare-for-subzero/`;
- modify `Cargo.toml` and `Cargo.lock`, `deny.toml`, `Dockerfile`, `compute/compute-node.Dockerfile`, `build-tools/Dockerfile`, `proxy/README.md` and `.config/hakari.toml` (only if hakari needs it);
- add `scripts/loams/strip-upstream-ci.sh`, `scripts/loams/check-no-private-sources.sh` and `scripts/loams/tests/test_hygiene.sh`.

**Interfaces** (one `Loams-Patch` id each):
- **LP-0002, upstream CI removed.** `strip-upstream-ci.sh` deletes every workflow without the `loams-` prefix. It is idempotent and runs once; with no upstream sync (D820), nothing brings the files back, and G0's `upstream_ci_absent` keeps it so.
- **LP-0003, no CDDL.** In the workspace `Cargo.toml`: `pprof` features `["criterion", "frame-pointer", "prost-codec"]` and `jemalloc_pprof` features `["symbolize"]`, both without `flamegraph`. Remove `CDDL-1.0` from `deny.toml`'s allow list. `http-utils`' profiling routes answer 400 `{"error":"svg flamegraphs are not built; use format=pprof"}` for `format=svg`.
- **LP-0004, no private Data API.** Delete the `SUBZERO_ACCESS_TOKEN` blocks from `Dockerfile` (keep `cargo chef prepare` and `cook` unconditional). Keep `libs/proxy/subzero_core` (the stub) so the workspace builds. Remove `rest_broker` from every CI feature list. Replace `proxy/README.md`'s instructions with a note that the REST broker is not supported in this fork (Q720).
- **LP-0005, git dependencies to `ostrium-labs` by rev.** `framed-websockets`, `tokio-epoll-uring`, `rust-postgres` (four crates and the `[patch.crates-io]` entry) and the `azure_*` crates use `git = "https://github.com/ostrium-labs/<repo>", rev = "<sha>"`, at the revisions `Cargo.lock` has today, each tagged `loams-<date>-1` in its fork.
- **LP-0006, no `neondatabase` downloads.**
  - `pg_session_jwt` comes from `https://github.com/ostrium-labs/pg_session_jwt/archive/refs/tags/v0.3.1.tar.gz`, after Task 3 pushes the tag (sha256 checked; equal to upstream's if the tag is identical).
  - `pgrag` is dropped (Q719 default) or comes from a fork if the owner keeps it.
  - `build-tools`' `lcov` comes from Debian or is removed.
  - The Dockerfiles' `ARG REPOSITORY=ghcr.io/ostrium-labs`.
- `check-no-private-sources.sh` fails when `Cargo.lock`, any `Dockerfile` or any workflow contains `neondatabase/` or `cache.neon.build`. Comments are allowed, and an allowlist names the files. The `flux-fleet` comments are left untouched.

Tests:
- `deny_rejects_cddl`: `cargo deny check licenses` passes. A fixture crate with `license = "CDDL-1.0"` added to a scratch workspace fails.
- `no_private_sources`: passes on the branch, fails on `fa504217`.
- `cargo tree -e features -i inferno` returns nothing.
- `cargo check --locked -p pageserver -p proxy -p compute_tools -p storage_controller` (in CI).
- `upstream_ci_absent`: no non-`loams-` workflow remains.

Commit one per patch id, e.g. `loams-licence: build pprof without flamegraph; CDDL out of deny.toml` (LP-0003).

### Task 8: `tokio-epoll-uring`'s licence

**Files:**
- In `ostrium-labs/tokio-epoll-uring` (`loams/main`): `LICENSE-MIT`, `LICENSE-APACHE`, `uring-common/LICENSE-tokio-uring` and `LICENSING.md`.
- In the `loams-postgres` repository: `docs/loams/LICENSING-tokio-epoll-uring.md`, and the `about.toml` used by `cargo about` (Task 10).

**Interfaces:**
- **The licence texts as declared** (§51 §11): MIT and Apache-2.0 for `tokio-epoll-uring`, attributed to "the tokio-epoll-uring authors (Neon Inc.)". MIT plus `tokio-uring`'s copyright notice for `uring-common`'s vendored buffer code (taken from the `tokio-uring` release the vendoring commit names).
- `LICENSING.md` cites the manifest lines and upstream commits (`781989bb`, PR #24).
- Move LP-0005's `tokio-epoll-uring` rev to the commit with the texts.
- **If Q716 allows it:** file one issue on `neondatabase/tokio-epoll-uring` asking for licence files that match the manifests. Record the link.
- **Fallback, only if Q716 or counsel says the declaration is insufficient:** a pageserver feature `io-uring`, on by default. Off, the `TokioEpollUring` engine is compiled out and `uring-common`'s traits are vendored under `pageserver/src/virtual_file/owned_buffers_io/buf/` with the MIT notice. G1 then runs both builds. Estimate: 1–2 engineer-weeks. It goes on a separate branch, `loams/nf1-no-uring`, and is not merged unless needed.

Tests:
- `third_party_licences_include_tokio_epoll_uring`: Task 10's generated licence file contains both texts.
- With the fallback only: `cargo check -p pageserver --no-default-features --features <rest>` and the G1 unit tests on `StdFs`.

Commit `loams-licence: licence texts for tokio-epoll-uring as its manifests declare` (LP-0007).

### Task 9: Our `loams-postgres-build-tools` image

**Files** (fork): `.github/workflows/loams-build-tools.yml`, `build-tools/Dockerfile` (LP-0006's edits), and the `Dockerfile` and compute Dockerfile `ARG TAG`, which becomes a digest (`ARG BUILD_TOOLS=ghcr.io/ostrium-labs/loams-postgres-build-tools@sha256:…`).

**Interfaces:**
- Multi-arch and cached as in Task 10. Signed and attested through Task 12's `loams-sign.yml` (Task 9 lands first with the attestation steps inline, and switches to the reusable workflow when Task 12 merges).
- Triggered by changes under `build-tools/`, monthly, and manually.
- The digest is written into both Dockerfiles by a bot PR.

Tests:
- The workflow builds on PR (no push).
- `build_tools_pinned_by_digest`: a grep check in G0.

Commit `loams-build: build-tools image from the fork` (LP-0009).

### Task 10: The `loams-postgres-storage` image workflow

**Files** (fork): `.github/workflows/loams-build-storage.yml`, `scripts/loams/audit-image.sh` (the same contract as Task 1's, vendored), `about.toml`, and `Dockerfile` (labels; `/usr/share/doc/loams-postgres/{LICENSE,NOTICE,LOAMS_PATCHES.md,THIRD_PARTY_LICENSES.html}`).

**Interfaces:**
- Jobs `build (amd64)` and `build (arm64)` on `vars.RUNNER_HEAVY` and `vars.RUNNER_HEAVY_ARM` (defaults `ubuntu-24.04` and `ubuntu-24.04-arm`):
  - BuildKit's root on `/mnt`;
  - `cache-from` and `cache-to` `type=registry,ref=ghcr.io/ostrium-labs/loams-postgres-buildcache:storage-<arch>,mode=max` (cache-to only from `main`);
  - `cargo auditable build` (already in the Dockerfile);
  - push by digest only;
  - build args `GIT_VERSION=<sha>` and `BUILD_TAG=lp-…|<sha12>`.
- Job `index`: `docker buildx imagetools create` with tags `<sha12>` and `edge` (release tags come from Task 13).
- Job `smoke`:
  - `pageserver --version` names the commit;
  - start a minimal `deploy/loams-postgres-dev`-shaped stack (pageserver, broker, one stock safekeeper as the reference, compute from the pinned compute image), create a tenant and timeline, and run `SELECT 1` through the compute;
  - `audit-image.sh` exits 0.
- `THIRD_PARTY_LICENSES.html` comes from `cargo about generate` with `about.toml`'s accepted licences (no CDDL).

Tests:
- `storage_image_audit_clean`
- `smoke_select_1`
- `labels_present` (`org.opencontainers.image.revision` equals the commit)
- The measured build times go into the rulings and §51 §5.2's table (Q721).

Commit `loams-build: loams-postgres-storage image workflow` (LP-0010).

### Task 11: The compute image workflow

**Files** (fork): `.github/workflows/loams-build-compute.yml`, `compute/compute-node.Dockerfile` (labels; `/usr/share/doc/loams-postgres-compute/`; `ARG BUILD_TOOLS`), and `scripts/loams/compute-heavy-stages.txt`.

**Interfaces:**
- Matrix `pg ∈ pins.supported ∪ maintenance ∪ beta` (today 16 and 17) × `arch ∈ {amd64, arm64}` (Q729).
- Heavy stages (`plv8-build`, `rdkit-build`, `postgis-build`, `pgrouting-build`, and while catalogued `pg_duckdb-build`) build in separate jobs with `--target <stage>` that push only the cache (`loams-postgres-buildcache:compute-v<N>-<arch>`). The final job then builds the image from a warm cache, within 6 h.
- Index tags as in the shared contract.
- Job `smoke`: start the compute against the Task 10 stack. For every `core` extension in the manifest (before Task 20: the list in §51 §7.1), run `CREATE EXTENSION`; then run `pg_stat_statements` and `vector` smoke queries. `audit-image.sh` on `compute_ctl`, `local_proxy` and `fast_import`.

Tests:
- `compute_image_audit_clean`
- `core_extensions_create`
- `heavy_stage_jobs_under_limit` (the run's timings recorded)
- Cold and warm durations per major and arch recorded in the rulings; Q721's budget checked against them.

Commit `loams-build: compute image workflow` (LP-0011).

### Task 12: Provenance, SBOM, signing, sources and verification

**Files:**
- Fork: `.github/workflows/loams-release.yml` (the signing steps, in a reusable workflow `loams-sign.yml` that `loams-release.yml` and Task 9's `loams-build-tools.yml` call).
- Here: `scripts/nf1/verify-pins.sh`, `scripts/nf1/tests/test_verify_pins.sh`, and `nf1-pins.yml` (a `verify` job).

**Interfaces:**
- For each index digest:
  - `actions/attest-build-provenance` and `actions/attest-sbom` (SPDX from Syft);
  - `cosign sign --yes <repo>@<digest>` (keyless; `id-token: write`);
  - for compute images, `oras attach --artifact-type application/vnd.loams.sources.v1 <repo>@<digest> sources.tar`, where `sources.tar` holds every tarball the build downloaded, listed from the manifest with sha256s;
  - every downloaded source also gets an SBOM entry.
- `verify-pins.sh`:
  - for each pin whose `source` is a fork commit, `cosign verify` with the shared identity, and `gh attestation verify --owner ostrium-labs`;
  - the SBOM attestation's licence list contains no `CDDL-*` and nothing outside the allowlist, except Debian's own packages, which are listed separately;
  - `mirror:` pins are skipped with a notice.
- **Not in NF1:** bit-for-bit reproducibility (Q726). `loams-rebuild-diff.yml` (monthly) rebuilds the last release and posts the layer diff as an issue comment, for information.

Tests:
- `unsigned_digest_fails_verify`
- `wrong_identity_fails_verify`: a fixture signed from a branch other than `main`, and one signed by the old repository name `ostrium-labs/neon`.
- `sbom_has_no_forbidden_licence`
- `gpl_sources_attached`: `oras discover` lists the artifact on a compute digest.
- `mirror_pin_skipped_with_notice`

Commits:
- fork: `loams-ci: sign, attest and attach sources` (LP-0012);
- here: `ci(pg): verify the pinned Loams Postgres images' signatures and attestations`.

### Task 13: Releases, tags and cadence

**Files** (fork): `.github/workflows/loams-release.yml`, `scripts/loams/release-notes.py`, `docs/loams/release.md` and `scripts/loams/tests/test_release.py`.

**Interfaces:**
- `workflow_dispatch(inputs: kind = storage|compute|full, version = lp-YYYY.MM.N)`, also called by Task 18.
- **Steps:**
  1. Refuse if the `lp-` tag exists.
  2. Run the gate (G0–G2, and G3 through Task 16).
  3. Build or reuse the digests for the commit.
  4. Tag the images with the release.
  5. Sign and attest (Task 12).
  6. Create the git tag and a GitHub Release whose notes list the digests, Postgres minors, extension versions, and patches added or dropped since the last release.
  7. The bot opens a PR on `ostrium-labs/loams` that runs `pins.py set --from-release <url>`.
- The cadence follows §51 §5.6. A scheduled check opens an issue when a monthly roll-up is due and none has shipped.

Tests:
- `release_tag_immutable`
- `notes_list_digests_and_patches`
- `pin_pr_matches_release`: the bot's PR body and diff carry the same digests as the release.

Commit `loams-ci: releases lp-YYYY.MM.N with pin PRs to Loams` (LP-0013).

### Task 14: The parity release, and Loams on our images

**Files:**
- Fork: none beyond running Task 13.
- Here: `deploy/pins/loams-postgres-images.toml` (by the bot PR), the consumers, `deploy/loams-postgres-dev/README.md`, and `crates/loams-postgres/tests/fixtures/README.md` (a note that the fixtures were re-checked).

Steps:
1. Cut `lp-2026.10.0` from `main` with **the same Postgres revisions as the mirror** (16.9, 17.5; §51 §5.5).
2. Merge the bot PR here.
3. Run:
   - `loams-postgres`'s fixture tests (byte-equal);
   - `pg2-e2e.yml` (`it_deploy_neon_tenant_timeline_branch`, `it-pageserver-loams-wal.sh`);
   - `loams-wal-decoder`'s golden tests with `PG_HEADERS_IMAGE` on the new `loams-postgres-storage` digest (headers identical, R31.6);
   - the desktop's stack copy on a scratch profile (start, create a branch, `SELECT 1`).
4. Any difference is a pipeline defect: fix it and re-release `lp-2026.10.1`.
5. Once green, compose defaults point at `ghcr.io/ostrium-labs`, which closes Q715's default path.

Tests: the above, all green. `pins.py check` shows no `mirror:` source left except as history in the README.

Commit `deploy(pg): pin Loams Postgres builds lp-2026.10.0`.

## NF1c — Postgres minors and the gate (Tasks 15–18)

The hard fork dropped this milestone's upstream sync (D820): Task 19 is removed, and nothing here merges from `neondatabase/*`. The milestone keeps the Postgres branches, the gate, the catch-up and the nightly minor tracking from `postgres/postgres`.

### Task 15: The Postgres fork's branches

**Files:** `ostrium-labs/postgres` branches. In the `loams-postgres` repository: `docs/loams/postgres-branches.md`.

**Interfaces:**
- For each major 16, 17 and 18: `loams/REL_<N>_STABLE` created from `REL_<N>_STABLE_neon`'s head (`a616eefe` for 18), unchanged.
- `REL_<N>_STABLE_neon` is frozen as history; nothing mirrors `neondatabase/postgres` any more (D821).
- Protection as Task 3.
- Without a local clone (Task 0), branches are created through `gh api repos/ostrium-labs/postgres/git/refs`.

Tests:
- `branches_exist_and_protected` (`gh api` probe)
- `submodule_url_relative`: `.gitmodules` in `main` still uses `../postgres.git`, so CI resolves to `ostrium-labs/postgres`.

Commit (fork) `loams-ci: postgres branch layout` (LP-0014).

### Task 16: The test gate (G0–G3)

**Files:**
- Fork: `.github/workflows/loams-gate.yml` and `scripts/loams/gate-subset.txt` (G2's `test_runner` selection, with each name checked to exist).
- Here: `.github/workflows/nf1-candidate.yml`.

**Interfaces:**
- **G0 to G2** as §51 §6.4.
  - G1 runs `cargo nextest run --locked` on the listed crates, for each supported major, with `NEON_PAGESERVER_UNIT_TEST_VIRTUAL_FILE_IOENGINE` set to `std-fs` and to `tokio-epoll-uring`.
  - G2 runs `./scripts/pytest` (Neon's runner) on `gate-subset.txt` with `neon_local`, one job per major. The subset: `test_pg_regress`, branching, `compute_ctl` spec and reconfigure, `import_pgdata`, LFC, and the walproposer against a safekeeper. Names are confirmed in this task.
- **G3:**
  - the fork calls `gh workflow run nf1-candidate.yml --repo ostrium-labs/loams -f storage=<digest> -f compute17=<digest> …` with the bot's token;
  - here, `nf1-candidate.yml` overrides the pin file in the job and runs `pg2-e2e.yml`'s jobs, the decoder's golden tests and the extension smoke;
  - it reports a commit status back to the fork's commit (`loams-gate / g3`).

Tests:
- `gate_fails_on_red_tier`: a fixture PR with a failing G1 test is blocked.
- `required_checks_configured`: `gh api …/branches/main/protection` lists `g0`, `g1`, `dco`, `patches`.
- `g3_reports_status`: a dry-run candidate with the current digests reports success.

Commits:
- fork: `loams-ci: the four-tier gate` (LP-0015);
- here: `ci(pg): candidate run for fork releases`.

### Task 17: The catch-up: 17.11 and 16.15 (then 17.12 and 16.16)

**Files:**
- `ostrium-labs/postgres` `loams/REL_17_STABLE` and `loams/REL_16_STABLE`.
- In the `loams-postgres` repository: `vendor/postgres-v17`, `vendor/postgres-v16`, `vendor/revisions.json`, `pgxn/neon` and `compute/` (fixes as needed).

**Interfaces:**
- Merge the upstream tags (`REL_17_11`, `REL_16_15`) into `loams/REL_<N>_STABLE`. Each conflict's resolution is recorded in the merge commit's message.
- Move the submodules to the merge commits and update `revisions.json` (`"v17": ["17.11", "<sha>"]`).
- Fix what G1 and G2 find in `pgxn/neon`, the extensions and `compute_ctl`.
- Release `lp-2026.11.0` (compute 16 and 17, and `loams-postgres-storage` for its WAL-redo binaries).
- When 17.12 and 16.16 ship on 2026-11-12, Task 18's job merges them, and the release `lp-2026.11.1` ships by 2026-11-19.

Tests:
- G1 and G2 at 16 and 17.
- G3: Loams' integration, plus `loams-postgres` fixtures re-recorded only if a response shape changed, with the diff reviewed.
- `select version()` on the compute answers 17.11 (then 17.12).
- `deploy/loams-postgres-dev`'s README smoke.

Commits:
- postgres: `Merge REL_17_11 into loams/REL_17_STABLE` (`Loams-Sync: REL_17_11`);
- loams-postgres: `loams-pg: Postgres 17.11 and 16.15` (LP-0016).

### Task 18: Minor-release tracking

**Files** (fork): `.github/workflows/loams-postgres-minors.yml`, `scripts/loams/postgres-minors.py` and `scripts/loams/tests/test_postgres_minors.py`.

**Interfaces:**
- **Nightly**, for each major in the pin file's supported, maintenance and beta lists:
  1. fetch `postgres/postgres` `REL_<N>_STABLE` into a scratch clone of `ostrium-labs/postgres` (blobless, inside the CI job);
  2. try to merge it into `loams/REL_<N>_STABLE`;
  3. if it merges cleanly, push `loams/minor-<N>-<date>` and keep a draft PR updated; if not, open or update the issue `postgres <N>: upstream merge conflict` with the files;
  4. on a new `REL_<N>_<m>` tag, open the release issue with the 7-day due date (Task 6's `sla.py`) and dispatch Task 13 with `kind=compute` once the merge PR is green.
- Pushes use the GitHub App token, because the target is another repository.

Tests (against temporary repositories built in the test):
- `clean_merge_updates_draft`
- `conflict_opens_issue_with_files`
- `new_minor_tag_starts_release`
- `no_change_no_noise`

Commit `loams-ci: nightly Postgres minor tracking` (LP-0017).

### Task 19: (Removed) The weekly upstream sync

Removed by the hard fork (D820, D821). There is no `loams-sync-upstream.yml`, `sync-upstream.sh` or `docs/loams/sync.md`. `main` is ours, and Neon's history ends at `neon-fork-point`. If `neondatabase/neon` ever publishes a commit worth having, a maintainer cherry-picks it by hand under a new `LP-` id (Task 4's `cherry_pick_needs_origin_line`). The number stays retired so the later task numbers do not move. Patch id LP-0018 is unused.

## NF1d — Extensions (Tasks 20–23)

### Task 20: The extension manifest and licence check

**Files** (fork): `compute/loams-extensions.toml`, `scripts/loams/check-extensions.py`, `scripts/loams/tests/test_check_extensions.py`, `compute/compute-node.Dockerfile` (comments that name the manifest), and `compute/etc/` (the generated `EXTENSIONS.md`).

**Interfaces:**
- **The manifest:** one entry per extension, with §51 §7.1's tiers (dropped entries kept, with `tier = "dropped"`). The contrib set is one entry per contrib module, with `source = "postgres"` and `license = "PostgreSQL"`.
- **`check-extensions.py`:**
  - URL, version and sha256 equal the Dockerfile's (parsed from `wget … -O` and `echo "<sha> …" | sha256sum --check`), per major `case` block;
  - each tarball's licence is detected (`askalono crawl` on the extracted tree) and equals `license`;
  - the licence is on the allowlist, and a GPL entry has `sources = true`;
  - every built stage is in the manifest, and nothing `dropped` is built;
  - `EXTENSIONS.md` is generated.

Tests:
- `forbidden_licence_rejected` (a fixture `AGPL-3.0` entry)
- `manifest_matches_dockerfile`
- `sha_mismatch_fails`
- `dropped_not_built`
- `gpl_requires_sources`
- `declared_vs_detected_mismatch_fails`: a fixture tarball with an MIT text declared as Apache-2.0.

Record each extension's detected licence in the rulings; that replaces §51 §7.1's "verify" marks.

Commit `loams-ext: extension manifest and licence check` (LP-0019).

### Task 21: The catalogue applied (answers Q651)

**Files** (fork): the manifest, `compute/compute-node.Dockerfile` (the `extensions-all` stage builds only `core` and `included`, through a generated stage list), `compute/manifest.yaml` (`neon_superuser`'s trusted list from `trusted`) and `test_runner` smoke additions (or `compute/tests/loams-extensions/*.sql`).

**Interfaces:**
- Q719's answer, or its default: drop `pgrag`, `pg_mooncake` and `pg_duckdb`.
- `pg_uuidv7` is limited to `majors = [16, 17]`.
- A `core` smoke per extension: `CREATE EXTENSION`, one function call, survival of a branch and a compute restart.
- An `included` smoke: `CREATE EXTENSION` only.

Tests:
- `core_extensions_survive_branch` (G2, every supported major)
- `included_extensions_create`
- `dropped_extension_absent`: `CREATE EXTENSION pg_duckdb` fails with "not available".

Then update PG2 Task 55 with a note (in its plan) that §51 §7.1 and this manifest are its allow-list.

Commit `loams-ext: the catalogue's tiers; dropped extensions removed` (LP-0020).

### Task 22: The `loams` extension

**Files** (fork): `pgxn/loams/{Makefile,loams.control,loams--1.0.sql,loams.c,README.md,sql/loams.sql,expected/loams.out}`, `pgxn/Makefile`, the root `Makefile` (`neon-pg-ext` builds it), the manifest entry (`tier = "core"`, `source = "in-tree"`, `license = "Apache-2.0"`), and `docs/loams/extensions.md` (how to add our own).

**Interfaces:**
- `loams.version() returns text` (the fork release, from a compile-time define).
- `loams.compute_info() returns table(project_id text, branch_id text, endpoint_id text, compute_id text)`, read from the GUCs `loams.project_id`, `loams.branch_id`, `loams.endpoint_id` and `loams.compute_id`; null when unset.
- `trusted = true`, no superuser functions, and no shared memory or preload.
- PG2's `spec::ComputeSpecBuilder` sets the four GUCs (a note for PG2 Task 10, recorded here, not implemented here).

Tests:
- `pg_regress` `loams` on every supported major (G2).
- `compute_info_reports_spec_ids`: a compute started with the four settings in its spec.
- `loams_extension_is_trusted`: `neon_superuser` can create it.

Commit `loams-ext: the loams extension` (LP-0021).

### Task 23: Extension updates

**Files** (fork): `.github/workflows/loams-extensions-update.yml`, `scripts/loams/extensions-update.py` and `scripts/loams/tests/test_extensions_update.py`.

**Interfaces:**
- **Monthly:** for each non-dropped, non-contrib entry, query the upstream's releases or tags. Propose the newest one that is at least 14 days old by opening one PR per extension, which updates the manifest and the Dockerfile's URL, sha256 and per-major `case`, and runs Task 21's smoke.
- **On an advisory** (Task 6) the 14-day rule is skipped and the PR is labelled `security`.
- A major-version bump of an extension is labelled `roll-up-only`.
- An upstream with no release in 24 months, or a licence change, opens a `deprecate` issue.

Tests:
- `respects_14_day_age`
- `pr_changes_manifest_and_dockerfile_together`
- `security_skips_age`
- `stale_upstream_opens_deprecation`

Commit `loams-ci: monthly extension updates` (LP-0022).

## NF1e — Postgres 18 (Tasks 24–30)

> **Superseded by [NF2](2026-10-18-nf2-postgres-18-19.md)** (D1107, 2026-10-18): this milestone's tasks moved to NF2, with the corrections of [§62](../design/62-postgres-major-versions.md) (the 18 base is `REL_18_6`, 19's GA is 2026-11-19, no `preview` pin list).

### Task 24: Postgres 18 in the fork

**Files:**
- `ostrium-labs/postgres` `loams/REL_18_STABLE` (18.2 → merge `REL_18_6`, and later `REL_18_7`).
- In the `loams-postgres` repository: `.gitmodules` (`vendor/postgres-v18`, branch `loams/REL_18_STABLE`), `vendor/revisions.json`, `postgres.mk`, `Makefile`, `build-tools/Dockerfile` (if 18 needs new build dependencies), and the `Dockerfile`'s Postgres build for all majors.

**Interfaces:**
- `make postgres-v18` builds.
- The `loams-postgres-storage` image contains `/usr/local/v18`.
- 14 and 15 are removed from the build loops (their submodules stay until Task 41).

Tests:
- `make check` on `loams/REL_18_STABLE` in CI.
- The `loams-postgres-storage` image's smoke shows `/usr/local/v18/bin/postgres --version` = 18.6.

Commits:
- postgres: `Merge REL_18_6 into loams/REL_18_STABLE`;
- loams-postgres: `loams-pg18: vendor Postgres 18` (LP-0023).

### Task 25: The format diff, `postgres_versioninfo` and `postgres_ffi` at 18

**Files** (fork): `docs/loams/pg18-format-diff.md`, `libs/postgres_versioninfo/src/lib.rs`, `libs/postgres_ffi/{build.rs,src/lib.rs,src/pg_constants_v18.rs,src/xlog_utils.rs}`, and `scripts/pg2/pg-headers.sh` here (the loop already lists `v18`).

**Interfaces:**
- **The diff** (§51 §8.3) covers `XLOG_PAGE_MAGIC`, `rmgrlist.h`, every `*_xlog.h` the decoder or `walingest.rs` touches, `pg_control.h`, the SLRU layouts, and the smgr API. Each item is classified "decoder", "walingest", "redo", "compute only" or "none".
- Rulings to make in the diff:
  - **checksums at bootstrap:** pass `--no-data-checksums` to `initdb`, or prove that reconstructed pages carry valid checksums;
  - **AIO in the compute:** implement the smgr read-stream path, or set `io_method = sync` in compute settings;
  - **protocol 3.2:** what `local_proxy` and pgbouncer do.
- `PgMajorVersion::PG18`, with `ALL` updated.
- `postgres_ffi`: bindgen for v18, `pg_constants_v18.rs`, and `for_all_postgres_versions!` covers 18.
- An unknown `XLOG_PAGE_MAGIC` returns an error, never a guess.

Tests:
- `pg_constants_v18_match_headers`: static asserts generated from the headers.
- `xlog_page_magic_v18`
- `unknown_page_magic_is_error`
- `cargo check -p postgres_ffi -p wal_decoder` with headers v16–v18.

Commit `loams-pg18: format diff; versioninfo and ffi for 18` (LP-0024).

### Task 26: `wal_decoder` and the pageserver at 18

**Files** (fork): `libs/wal_decoder/src/{decoder.rs,models/*}`, `pageserver/src/{walingest.rs,basebackup.rs,import_datadir.rs,pgdatadir_mapping.rs,walredo.rs,walredo/*}`, `pgxn/neon_walredo/` (built for 18), and `libs/postgres_ffi/wal_craft` (fixtures at 18).

**Interfaces:**
- Every record the diff classified "decoder" or "walingest" is handled at 18.
- WAL redo runs the v18 `postgres --wal-redo`.
- Basebackup writes an 18 control file.
- `import_datadir` and `ImportPgdata` accept 18.
- A record type the decoder does not know is an error naming the rmgr and info byte.

Tests:
- `decode_all_rmgrs_pg18`: `wal_craft` generates every record type the decoder interprets, at 18, and decodes them.
- G2 at 18: `test_pg_regress`, branching at LSN and timestamp, restart from the pageserver, and `import_pgdata`.
- `pgbench_checksums_survive_restart_and_branch_pg18`: `pgbench -i -s 10`, then a 5-minute run; the table checksums are compared after a pageserver restart and on a branch.

Commit `loams-pg18: wal_decoder and pageserver for 18` (LP-0025).

### Task 27: `pgxn/neon`, the walproposer and `compute_ctl` at 18

**Files** (fork): `pgxn/neon/*.c,*.h` (smgr, `file_cache.c`, `walproposer*.c`, `neon_walreader.c`, `communicator*`, `neon_pgversioncompat.h`), `pgxn/neon_rmgr/`, `pgxn/neon_utils/`, `compute_tools/src/*` (spec, `pg_hba`, `extension_server`, config templates) and `compute/etc/`.

**Interfaces:**
- The `neon` extension builds and loads at 18.
- The smgr follows Task 25's AIO ruling.
- LFC resize works.
- The walproposer speaks protocol v3 to a safekeeper and to `loams-wal` with `pg_version = 180000+minor`.
- `neon_rmgr` handles 18's heap records.
- `compute_ctl` starts an 18 compute from a spec.

Tests:
- G2 at 18: the compute subset (`compute_ctl` spec and reconfigure, LFC, walproposer).
- `walproposer_pg18_to_loams_wal`: G3, a compute at 18 against `loams-wal` (both stores) with pgbench for 2 minutes; no lost commit (bank check).

Commit `loams-pg18: neon extension, walproposer and compute_ctl for 18` (LP-0026).

### Task 28: `loams-postgres-compute-v18` (preview)

**Files** (fork): `compute/compute-node.Dockerfile` (`PG_VERSION=v18` in every per-major `case`; versions of the `core` and `included` extensions that support 18; the others marked `majors` without 18), the manifest, and `loams-build-compute.yml`'s matrix (from the pin file's majors plus `preview = [18]`).

**Interfaces:**
- Images `loams-postgres-compute-v18:18.6-<sha12>`, with `edge` only. Not in Loams' pin file's `supported` list yet.
- The Debian base follows Q727 (default: `trixie` for 18 and the `loams-postgres-storage` image).
- Extensions that do not support 18 are listed in the release notes as "not yet at 18".

Tests:
- `core_extensions_survive_branch` at 18
- `included_extensions_create` at 18
- `compute_image_audit_clean` at 18

Commit `loams-pg18: loams-postgres-compute-v18 preview images` (LP-0027).

### Task 29: Loams at 18: decoder, safekeeper, `loams-postgres`

**Files** (here): `crates/loams-wal-decoder/{Cargo.toml,Cargo.lock,tests/golden.rs,tests/fixtures/fork-sender-18-*.bin,tests/fixtures/README.md}`, `crates/loams-safekeeper/src/{proto.rs,http.rs}` (only if version handling changes), `crates/loams-postgres/src/{majors.rs,pageserver.rs}` (generated `majors.rs`; `pg_version` validation), `scripts/pg2/pg-headers.sh` (`v18`, and `v19` for later), `.github/workflows/pg2-e2e.yml` (a matrix over the pin file's supported, preview and beta majors), and `deploy/loams-postgres-dev/compose.yaml` (a `compute-v18` service under profile `pg18`).

**Interfaces:**
- The decoder's four fork crates move to the `main` rev that has Task 26, tagged `loams-<date>-<n>`. `check-pinned-revs.sh` passes.
- Golden fixtures at 18 are recorded from the fork's own interpreted sender (the method of PG2 R31.x, documented in `tests/fixtures/README.md`).
- `loams-safekeeper` accepts `pg_version` 18xxxx (its `full_pg_version` and the greeting are version-agnostic today; confirm and add a test).
- `loams-postgres` refuses a `pg_version` outside the supported, preview, beta and maintenance majors (`unsupported_pg_version` reason, registered in `docs/api/reasons.md`).

Tests:
- `golden_pg18_matches_fork_sender`
- `it_pageserver_ingests_from_loams_wal_pg18` (`scripts/pg2/it-pageserver-loams-wal.sh` with `PG_VERSION=18`)
- `greeting_accepts_pg18`
- `create_timeline_rejects_unsupported_major`
- `cargo deny check` on the decoder's own policy

Commit `wal: the decoder and loams-wal at Postgres 18`.

### Task 30: The Postgres 18 gate and promotion (Q717)

**Files:** here, `deploy/pins/loams-postgres-images.toml` (by the release PR) and `deploy/loams-postgres-dev/README.md`; in the fork, `docs/loams/release.md`.

Steps:
1. Run §51 §8.4's matrix at 18, twice, on two different days, including:
   - Task 36's PgDog results at 18;
   - Task 37's 17 → 18 upgrade test (if Task 37 is not done yet, the promotion waits for it);
   - the desktop's local stack at 18.
2. When both runs are green, release with `supported = [17, 18]` (and, per Q717's default, `default = 18`).
3. Record the evidence links in the rulings. PG2's Task 0 ruling 7 is then superseded in practice.

Tests: the matrix; `pins.py check`; `nf1-pins.yml` `verify`.

Commit `deploy(pg): Postgres 18 supported`.

## NF1f — Postgres 19 (Tasks 31–35)

> **Superseded by [NF2](2026-10-18-nf2-postgres-18-19.md)** (D1107, 2026-10-18): this milestone's tasks moved to NF2, with the corrections of [§62](../design/62-postgres-major-versions.md) (the 18 base is `REL_18_6`, 19's GA is 2026-11-19, no `preview` pin list).

### Task 31: Track 19 from RC1

**Files:**
- `ostrium-labs/postgres` `loams/REL_19_STABLE` (from `postgres/postgres` `REL_19_STABLE`; vanilla to start).
- In the `loams-postgres` repository: `docs/loams/pg19-format-diff.md`, and `.github/workflows/loams-postgres-minors.yml` (19 added to the nightly).

**Interfaces:**
- From 2026-10-15 (RC1): a nightly vanilla build and `make check` of 19 in `build-tools`.
- **The 18 → 19 diff**, as Task 25, read at RC1 and updated at GA (2026-10-29). It specifically covers the multixact offset width (§51 §8.3) and every change to SLRU, the control file, smgr and WAL records.

Tests: the nightly build; the diff's checklist has no "unclassified" items.

Commit `loams-pg19: track Postgres 19; format diff` (LP-0028).

### Task 32: Port Neon's patch series to 19

**Files:** `ostrium-labs/postgres` `loams/REL_19_STABLE`, and the fork's `LOAMS_PATCHES.md` (one row per ported change).

**Interfaces:**
- Cherry-pick Neon's changes from `loams/REL_18_STABLE` (the commits over `REL_18_STABLE`) in `docs/core_changes.md`'s order. Each picked commit gets `Loams-Patch: LP-19xx` and a `(ported from <18 sha>)` line.
- Changes that 19 made unnecessary are dropped with a reason in the inventory.
- A change that needs a redesign at 19 (for example multixact handling) gets its own row and test.

Tests:
- `make check` and `installcheck-world` on `loams/REL_19_STABLE`.
- Neon's Postgres-side tests that run without the storage (the `neon_test_utils` paths).

Commit (postgres) one per ported change; (loams-postgres) `loams-pg19: patch inventory for 19` (LP-0029).

### Task 33: The Rust side and the compute at 19

**Files** (fork): as Tasks 25–27 for 19: `libs/postgres_versioninfo` (`PG19`), `libs/postgres_ffi` (`pg_constants_v19.rs`), `libs/wal_decoder`, the pageserver files, `pgxn/neon*`, `compute_tools`, and `vendor/postgres-v19` (submodule on `loams/REL_19_STABLE`).

**Interfaces:** as Tasks 25–27, with Task 31's diff as the checklist.

Tests:
- `decode_all_rmgrs_pg19`
- `xlog_page_magic_v19`
- `unknown_page_magic_is_error` (unchanged)
- G2 at 19
- `pgbench_checksums_survive_restart_and_branch_pg19`
- `walproposer_pg19_to_loams_wal`

Commit `loams-pg19: versioninfo, ffi, decoder, pageserver, neon extension and compute_ctl for 19` (LP-0030).

### Task 34: `loams-postgres-compute-v19` beta, and Loams at 19

**Files:**
- Fork: the compute Dockerfile and manifest (`core` at 19 only; `included` as each supports 19), and `loams-build-compute.yml` (tags with `-beta`; amd64 only per Q729's default).
- Here: the decoder's rev and `fork-sender-19-*.bin` fixtures; the pin file (`beta = [19]`, by the release PR); `deploy/loams-postgres-dev/compose.yaml` (a `compute-v19` service under profile `beta`); and `pg2-e2e.yml` (19 in the matrix with `continue-on-error: true` until Task 35).

**Interfaces:**
- `loams-postgres-compute-v19:19.<m>-beta-<sha12>` and `beta`.
- `loams-postgres` and `pg-control` accept 19 only for projects with `allow_beta_major` (PG2's `CreateProject` gains the field; a note for PG2 Task 1's owner, recorded here).

Tests:
- `golden_pg19_matches_fork_sender`
- `it_pageserver_ingests_from_loams_wal_pg19`
- `beta_major_requires_flag`
- `core_extensions_survive_branch` at 19

Commit (here) `wal: Postgres 19 beta in the decoder, loams-wal and the stacks`.

### Task 35: The Postgres 19 GA gate (Q718)

Steps:
1. Check Q718's conditions (default):
   - §51 §8.4's matrix at 19 green twice;
   - upstream 19.1 released (expected 2027-02-11, verify);
   - 30 days of the beta on Loams' test cluster with no data-loss bug;
   - Task 36's PgDog results at 19 without `error` rows in components Loams uses;
   - Task 37's 18 → 19 upgrade green.
2. When they hold, release with `supported` including 19, drop `-beta`, add arm64 per Q729, make `pg2-e2e.yml`'s 19 leg required, and remove the "beta" label from the docs.

Tests: the matrix; `pins.py check`; `verify-pins.sh`.

Commit `deploy(pg): Postgres 19 supported`.

## NF1g — Upgrades, Loams integration and ownership (Tasks 36–41)

> Tasks 36–38 and Task 41's removal of 14 and 15 moved to [NF2](2026-10-18-nf2-postgres-18-19.md) Tasks 12, 13, 25 and 16 (D1107, D1115). Tasks 39–41 stay here.

### Task 36: PgDog per major

**Files** (here): `conformance/router/pgdog-pg{17,18,19}.tsv`, `conformance/router/README.md` (the per-major procedure), and `scripts/nf1/pgdog-majors.sh`.

**Interfaces:** for the pinned PgDog image (PG2 Task 18) and each major's compute:
- the `conformance/router` statement inventory replayed through PgDog;
- driver connects with libpq 18 at `max_protocol_version=3.0` and `3.2`;
- a cancel request with a 3.2 cancel key;
- SCRAM;
- §51 §12.4's new-syntax statements.

Results use the inventory's classes. Gaps are documented with workarounds and, with the owner's approval, reported upstream (§46 §8.3).

Tests: `pgdog_major_has_no_error_rows_in_used_components` (a gate for Tasks 30 and 35).

Commit `conformance(pg): PgDog against Postgres 17, 18 and 19`.

### Task 37: Major upgrade of a branch through `fast_import` and `ImportPgdata`

**Files:**
- Fork: `compute_tools/src/bin/fast_import.rs` (only if the S3 endpoint for RustFS needs a setting; verify), and `docs/loams/upgrade.md`.
- Here: `crates/loams-postgres/src/pageserver.rs` (`create_timeline_import(t, tl, ImportPgdata { location: AwsS3 { region, bucket, key }, idempotency_key })` and `import_status`), `crates/loams-postgres/tests/fixtures/` (the import request and response), and `scripts/pg2/upgrade-branch.sh` (the local and desktop driver).

**Interfaces:**
- **Path 1 of §51 §9:**
  1. a read-only compute of the source branch;
  2. `fast_import pgdata` from the target major's compute image to the bucket prefix `pgupgrade/<tenant>/<new timeline>/`;
  3. the import into a new root timeline of the same tenant;
  4. a `loams-wal` timeline at the import's end LSN;
  5. a new branch `<name>-pg<N>`.
- The swap is PG2's (Task 51's `UpgradeProject`, amended). This task delivers the pieces and the script.
- **`compute_major_mismatch_refused`:** `loams-postgres`'s `ComputeSpecBuilder` refuses a compute image whose major differs from the timeline's `pg_version`.

Tests:
- `upgrade_17_to_18_preserves_checksums`: `deploy/loams-postgres-dev` with RustFS, a pgbench database at 17, upgraded, with per-table checksums equal.
- `upgrade_failure_keeps_source_serving`: the import is killed mid-way; the source branch is untouched and still writable.
- `upgrade_is_idempotent_by_key`: a retried import with the same key creates one timeline.
- `compute_major_mismatch_refused`

Then update PG2 Task 51 (a note in its plan) to use these pieces, with dump and restore as the fallback.

Commit `pg: upgrade a branch to a new major through fast_import and ImportPgdata`.

### Task 38: The `pg_upgrade --link` spike (Q725)

**Files:** in the fork, `docs/loams/pg-upgrade-spike.md`; here, the rulings.

Steps:
1. On a scratch tenant: materialise the source branch's data directory from a pageserver basebackup at a quiesced LSN.
2. Find what it takes to reach a clean shutdown that `pg_upgrade` accepts (Neon's basebackup markers, the `neon` settings).
3. Run `pg_upgrade --link` (and 18's `--swap`).
4. Upload, import as in Task 37, and compare checksums.
5. Measure both paths at 1 GB, 10 GB and 50 GB (estimate the 50 GB run if the runner's disk forbids it).
6. Rule on Q725 with the numbers: adopt it as path 2 (a new task in a follow-up plan) or drop it.

Tests: the checksums; the measurements recorded.

Commit `docs(nf1): pg_upgrade spike results`.

### Task 39: The desktop and Postgres majors

**Precondition:** PG2 Task 58 has merged. AP1e Tasks 21–23 are already built (PG2 Task 0 ruling 8): `apps/desktop-electron/src/main/sql/{neon.ts,pg.ts}` drive `deploy/loams-postgres-dev`, and `pg.ts`'s `PostgresBackend` is the seam.

**Files:** `deploy/loams-postgres-dev/compose.yaml` (`compute-v<N>` services generated from the pin file; profiles `pg18` and `beta`), `apps/desktop-electron/src/main/stacks/stacks.ts` (the version marker includes `fork_release`), `apps/desktop-electron/src/main/sql/backend/local-stack.ts`, `web/plugins/postgres/` (the upgrade prompt), and `apps/desktop-electron/test/pg-majors.test.ts`.

**Interfaces:**
- The local-stack backend reads each timeline's `pg_version` and starts the compute of that major. A new tenant uses `majors.default`.
- When a branch's major is below `default`, the page shows "Upgrade to Postgres <N>". On confirmation it runs Task 37's script locally and shows the new branch. The old one stays.
- A 16 timeline pulls `loams-postgres-compute-v16` (or, before our first 16 build, Neon's mirrored `neon-archive/compute-node-v16`) on demand for export or upgrade.
- A beta major is offered only behind the Settings toggle "Show beta Postgres versions".

Tests:
- `local_stack_starts_timeline_major_only`
- `upgrade_never_automatic`: no code path calls the upgrade without the confirmation event (Q728).
- `pin_change_refreshes_stack_files_not_volumes`
- `beta_major_hidden_by_default`
- An e2e on Linux: a 17 branch upgraded to 18, then a query on both.

Commit `feat(desktop): Postgres majors on the local stack, and branch upgrades`.

### Task 40: Runbooks, maintainers and the MVM switch

**Files:**
- Here: `docs/runbooks/loams-postgres-fork/{release.md,minor-release.md,security.md,new-major.md,mvm.md}`.
- Fork: `.github/workflows/loams-*.yml` (`if: vars.MAINTENANCE_MODE != 'mvm'` on the jobs MVM stops, §51 §13.3) and `scripts/loams/tests/test_mvm.py`.

**Interfaces:**
- Each runbook is a numbered procedure with the commands, the expected output and the escalation. Each is executed once by the second maintainer (Q722) and marked verified with the date.
- **`MAINTENANCE_MODE`:**
  - `full` (default) or `mvm`;
  - in `mvm`, extension updates run only on advisories, and new-major and rebrand jobs are skipped;
  - minors, advisories, the fuzz job, the Debian rebuild and the gate are never skipped.
- Record the first quarter's actual hours against §51 §13's estimates (D824) in the rulings.

Tests:
- `mvm_mode_keeps_security_jobs`: parses the workflows and asserts that the minor, advisory, fuzz and gate jobs have no `MAINTENANCE_MODE` condition.
- `mvm_mode_skips_listed_jobs`
- `actionlint`

Commit `docs(pg): Loams Postgres fork runbooks; ci: the MVM switch`.

### Task 41: Docs, status and cleanup

**Files:** `docs/design/51-loams-postgres-fork.md` (§14 as built, the evidence links), `docs/design/46-loams-postgres-production.md` (§12's fork-release line, §19), `docs/design/28-loams-postgres.md` (§10's header note), `docs/design/13-decision-log.md` (statuses), `docs/plans/README.md` (status), this plan's exit checklist, and the fork's `vendor/postgres-v14` and `-v15` submodules (removed, LP-0031, after 14's EOL on 2026-11-12).

Steps:
1. Tick each exit item with a link.
2. Update the statuses.
3. Remove 14 and 15 from the fork.

Commit `docs(nf1): exit, as built and status`.

---

## Exit criteria (with the owning tasks)

- [ ] **Hard fork and names:** `ostrium-labs/loams-postgres` with `main` as our trunk and `neon-fork-point` tagged; the fork network detached (owner, Q730); the crate `loams-postgres`, `deploy/loams-postgres-dev` and the decoder's new URL, with users' volumes untouched: Tasks 1b, 3.
- [ ] **Inventory and governance:** eight forks with branches, protection, CODEOWNERS (two maintainers), DCO, `SECURITY.md`, `NOTICE` with "Based on Neon by Databricks/Neon Inc."; the patch queue enforced: Tasks 3–5.
- [ ] **Mirror:** both pinned images (and v16) mirrored to `neon-archive/` with identical digests and audited; visibility per Q715: Task 1.
- [ ] **Own builds:** `loams-postgres-storage`, `loams-postgres-compute-v16`, `-v17` and `-v18` (and `-v19` beta) multi-arch from `main`; no CDDL, no private source, no `neondatabase` fetch: Tasks 7, 9–11.
- [ ] **Supply chain:** provenance, SBOM, cosign, GPL sources; Loams verifies every pin: Task 12.
- [ ] **Releases:** `lp-YYYY.MM.N` with pin PRs; the parity release; Loams on our images: Tasks 2, 13, 14.
- [ ] **Minors:** 17.11 and 16.15 shipped; 17.12, 18.7 and 16.16 within 7 days of 2026-11-12; nightly tracking from `postgres/postgres`: Tasks 17, 18.
- [ ] **Gate:** G0–G3 required on `main`: Task 16.
- [ ] **Security:** advisories daily; nightly fuzzing; targets in force: Task 6.
- [ ] **Extensions:** manifest, licence check, tiers, the `loams` extension, monthly updates: Tasks 20–23.
- [ ] **Postgres 18 supported** in Loams, with the matrix green twice: Tasks 24–30.
- [ ] **Postgres 19 beta** images and Loams support behind `allow_beta_major`; **19 supported** only after Q718's conditions: Tasks 31–35.
- [ ] **Upgrades:** 17 → 18 and 18 → 19 through `fast_import` and import, with checksums equal; PG2 Task 51 uses it; the `pg_upgrade` spike ruled: Tasks 37, 38.
- [ ] **PgDog** results per major: Task 36.
- [ ] **Desktop:** per-timeline majors, the upgrade prompt, beta hidden: Task 39.
- [ ] **`tokio-epoll-uring`:** licence texts and the upstream ask per Q716 (or the fallback build): Task 8.
- [ ] **Ownership:** runbooks verified by the second maintainer; the MVM switch; actual hours recorded against D824: Task 40.

## Self-review

- **Spec coverage.** All ten areas of the owner's brief map to tasks:

  | Area | Design | Tasks |
  |---|---|---|
  | 1. Inventory and governance | §51 §3 | 3–6 |
  | 1a. Hard fork and rebrand (amendment) | §51 §1a, §12.6 | 1b, 3–5 |
  | 2. Mirror | §51 §4 | 1, 2 |
  | 3. Own builds | §51 §5 | 7–14 |
  | 4. Postgres minors (the Neon sync is removed) | §51 §6 | 15–18 |
  | 5. Extensions | §51 §7 | 20–23 |
  | 6. Postgres 18 and 19 | §51 §8, §9 | 24–35, 37, 38 |
  | 7. Private dependencies | §51 §10 | 7 |
  | 8. `tokio-epoll-uring` | §51 §11 | 8 |
  | 9. Integration | §51 §12 | 2, 14, 29, 34, 36, 39 |
  | 10. Staffing (re-costed, D824) | §51 §13 | 40 |

- **Facts corrected against the brief:**
  - `tokio-epoll-uring` declares `MIT OR Apache-2.0` (and MIT for `uring-common`) in its manifests; only the licence files are missing (§51 §11, Task 0).
  - The mirrored binaries very likely contain `inferno` (CDDL) and the proxy the private `subzero-core`, which makes the mirror's visibility an owner question (Q715) rather than a given.
  - The compute Dockerfile has 98 stages, which is 38 third-party extensions, not "about 47".
  - The decoder is pinned by `rev` `1218fb7a`, not by the tag; the tag points at that commit.
- **Placeholder scan.** Digests that do not exist yet (our builds, v16's mirror) are produced by the tasks that name them. Every "(verify)" in §51 has a task that checks it (Tasks 0, 1, 11, 20, 25, 31, 37).
- **Risk to PG2.** NF1 does not block PG2's GA. PG2 keeps 17. Our images replace Neon's through the pin file without changing any API shape (Task 14 proves parity first).
- **Decisions the owner must make before the tasks that need them:**

  | Question | Before |
  |---|---|
  | Q715 (mirror visibility) | Task 1 |
  | Q716 (`tokio-epoll-uring`) | Task 8 |
  | Q719 (drop list) | Task 21 |
  | Q720 (Data API) | Task 7 |
  | Q721 (runner budget) | Task 11 |
  | Q722, Q734 (maintainers, staffing) | Task 5 |
  | Q730 (detach the fork network) | Task 3 |
  | Q731 (dev stack path) | Task 1b |
  | Q733 (bot; token scopes) | Task 1 |
  | Q724 (security targets) | Task 6 |
  | Q727 (Debian base) | Task 28 |
  | Q729 (arm64 compute) | Task 11 |
  | Q717 (default major) | Task 30 |
  | Q718 (19 GA) | Task 35 |
  | Q725 (`pg_upgrade`) | Task 38 |
  | Q728 (desktop prompt) | Task 39 |
  | Q723 (names) | Answered by D822 |
  | Q732 (compatibility names) | After NF1 |
  | Q726 (reproducibility) | Before or at Task 12 |

  The owner actions: the GitHub App (or approval for the controller to create it through the manifest flow), the teams, a token with `write:packages`, `read:packages` and `admin:org` (today it has `repo` and `workflow` only), package visibility, billing, detaching the fork network, and the clone of `ostrium-labs/postgres`.

## Rulings made during execution

### Task 0 rulings (2026-10-09, reconciled at `dev` `1cd3d5dc`)

Checked with `git` in this repository, the read-only Neon clone at `~/Documents/Ostriumlabs/neon` (HEAD `1218fb7a`), `gh api` (account `dina-kar`), anonymous `ghcr.io` manifest `HEAD` requests, and postgresql.org. Nothing was cloned or fetched.

1. **PG2 Task 2 has merged into `dev`.** `backend/pg2` (tip `6deaa2c8`, "task 2 fix round 1 rulings R2.11–R2.18") is an ancestor of `dev`; `git log dev..backend/pg2` is empty, and `1cd3d5dc` (this plan) sits directly on it. On `dev`:
   - `deploy/neon/compose.yaml` (lines 10–11) and `deploy/loams-pg-bench/compose.yaml` (lines 14–15) define `x-neon-image` = `ghcr.io/neondatabase/neon@sha256:7a4f1249…434c761f` and `x-compute-image` = `ghcr.io/neondatabase/compute-node-v17@sha256:13ab146d…5c70e26c3`, each overridable by `NEON_IMAGE` / `COMPUTE_IMAGE`;
   - `PG_HEADERS_IMAGE` is that `neon` digest in `.github/workflows/pg2.yml:64`, `pg2-e2e.yml:70` and `loams-pg-bench.yml:40`;
   - no `neondatabase/*:latest` remains under `deploy/` or `.github/`.

   So Tasks 2 and 14 branch from `dev`, not `backend/pg2`; their PRs do not need the "builds on `backend/pg2`" note. These are exactly the five consumers `pins.py check` must cover first. §51 §14's "Loams pins" row ("`dev`: `deploy/neon` still uses `latest`") is stale; Task 41 corrects it (this task edits rulings only).
2. **Both pinned source digests still resolve.** `HEAD https://ghcr.io/v2/neondatabase/{neon,compute-node-v17}/manifests/<digest>` returns 200 with `application/vnd.oci.image.index.v1+json` for both (2026-10-09). Task 1 can still copy them unchanged; it stays first in NF1a.
3. **§51 §14 is otherwise still true for `ostrium-labs/neon`.**
   - Default branch `main`, at `fa504217`; `compare main...neondatabase:neon:main` is `identical` (0/0).
   - **Upstream has not moved since 2026-08-31:** `neondatabase/neon` `main` is `fa504217` ("docs: fix typo proccess -> process (#12940)", 2026-08-31), `pushed_at` 2026-08-31, not archived. Task 19's weekly sync has nothing to merge today. If upstream stays still, it is the fork that carries every Postgres minor (Tasks 17 and 18), and the plan already assumes that.
   - 1,488 branches; the only `loams/*` branch is `loams/decoder-trim` at `1218fb7a` (one commit on `fa504217`, 2026-10-08).
   - 283 tags; the only `loams-*` tag is `loams-decoder-trim-1`, which is annotated (tag object `882f8b41`) and points at `1218fb7a`. No `nf-*` tags.
   - Actions: `actions/permissions` is `enabled: true`, `allowed_actions: all`; `actions/workflows` reports `total_count: 0` (no workflow registered). `ostrium-labs/postgres` is the same (`enabled: true`, `all`, 0 workflows); Task 3 disables its Actions. No task narrows `allowed_actions`; SHA pinning stays enforced by the Global Constraints and review.
4. **§51 §14 is still true for `ostrium-labs/postgres`, with versions added.**
   - 322 branches, none `loams/*`. Default branch `main` holds only `README.md` (`0061a6b2`, 2025-09-22), which is why GitHub reports no licence; `COPYRIGHT` is on the `REL_*` branches.
   - `_neon` branches: `REL_14_STABLE_neon` (`74c6ea95`), `REL_15_STABLE_neon` (`6056289b`), `REL_16_STABLE_neon` (`59027122`, 16.12), `REL_17_STABLE_neon` (`56692dfb`, 17.8), `REL_18_STABLE_neon` (`a616eefe`, 18.2), plus `REL_14_{6,7,8}_neon`, `REL_15_{1,2,3}_neon`, `anastasia/REL_17_STABLE_neon` and `rename_contrib_zenith_to_neon`. The versions are from each branch's `configure.ac` `AC_INIT`.
   - `REL_16/17/18_STABLE_neon` are `identical` to `neondatabase/postgres`'s branches of the same name.
   - Neon `fa504217`'s submodule revisions (`vendor/revisions.json`: 17.5 `1e01fcea`, 16.9 `a42351fc`, 15.13 `2aaab3bb`, 14.18 `2155cb16`) exist in the fork, and the 17 and 16 ones are ancestors of their `_neon` heads (475 and 387 commits behind). Task 3 can tag them in place; no branch has to be created to keep them reachable.
5. **The eight forks' licences match §51 §3.1.** From `gh api repos/ostrium-labs/<repo>` and each default branch's root:

   | Fork | Default branch | GitHub licence | Root licence files |
   |---|---|---|---|
   | `neon` | `main` | Apache-2.0 | `LICENSE`, `NOTICE` |
   | `postgres` | `main` | none | none on `main`; `COPYRIGHT` on `REL_*` |
   | `rust-postgres` | `neon` | NOASSERTION (two licences) | `LICENSE-APACHE`, `LICENSE-MIT` |
   | `azure-sdk-for-rust` | `main` (branch `neon` exists) | MIT | `LICENSE.txt`, `NOTICE.txt` |
   | `framed-websockets` | `main` | Apache-2.0 | `LICENSE` |
   | `tokio-epoll-uring` | `main` | none | none |
   | `pg_session_jwt` | `main` | Apache-2.0 | `LICENSE` |
   | `autoscaling` | `main` | Apache-2.0 | `LICENSE` |

   All eight are public, unarchived forks of their `neondatabase` parents. `rust-postgres` `f3cf448f` (the decoder's pin) is `neon`'s head. The organisation also has forks NF1 does not cover (`resonate`, `client-rust`, `sqlx`, `wesql`, `loams-desktop`).
6. **`tokio-epoll-uring`'s licence: §51 §11 is confirmed; the original brief was wrong.**
   - `tokio-epoll-uring/Cargo.toml`: `license = "MIT OR Apache-2.0"`; `uring-common/Cargo.toml`: `license = "MIT" # the same as tokio-uring at the time we forked it`. The same at `main` and at `781989bb`, in both `ostrium-labs` and `neondatabase`.
   - No licence, copying or notice file anywhere in the tree (`git/trees/main?recursive=1`: 0 paths match). GitHub reports no licence for either repository.
   - Fork `main` is `478ab1a3` (2026-09-21), `identical` to upstream. Neon's lock pins `781989bb` (2024-10-29) through `branch = "main"` in Neon's `Cargo.toml`. Task 7 (NF-0005) moves it to `ostrium-labs` by rev with a `loams-*` tag. Q716 is still open.
   - **Ruling for Task 8.** `781989bb..main` is 5 commits (`7adcb121`…`478ab1a3`, 2026-09-17 to 09-21, upstream PR #76) that fix a file-descriptor leak when an `open` future is cancelled; they touch only `tokio-epoll-uring/src/{ops/open_at.rs,system/slots.rs,system/submission/op_fut.rs,system/tests.rs}`. Task 8's "move NF-0005's rev to the commit with the texts" therefore also takes this fix, because `loams/main` starts from `main`. That is wanted (a real bug fix), but it is a code change, not only a licence change: Task 8's commit message names the 5 commits, and its PR in the `neon` fork runs G1 with both I/O engines.
7. **Upstream Postgres matches §51 §14.**
   - Latest minors: `REL_18_6` (`724edf9b`, tagged 2026-08-11), `REL_17_11` (`083ac033`, 2026-08-10), `REL_16_15` (`7d3e000c`, 2026-08-10).
   - `REL_19_STABLE` is at `1ecc48b9` (2026-10-09). Its latest tag is `REL_19_BETA4` (`b73d13c3`, tagged 2026-09-21, announced 2026-09-24). There is no `REL_19_RC1` yet.
   - postgresql.org: the wiki's "PostgreSQL 19 Open Items" gives "RC 1: October 15, 2026" and "GA: (Planned) October 29, 2026"; the roadmap says "planned for October 2026", and lists the next minors on 2026-11-12, 2027-02-11, 2027-05-13 and 2027-08-12. Task 31 starts on 2026-10-15 and checks the RC1 tag that day. Task 17's 17.12 date (2026-11-12) is confirmed.
8. **Cloning `ostrium-labs/postgres`: waiting on owner.** No approval has been given (the 2026-10-09 answers cover Q715, Q717, Q719 and the mode only). Until it is given, Tasks 15, 17, 24, 31 and 32 work through `gh api` and the fork's CI only. When approved, the clone is blobless at `~/Documents/Ostriumlabs/postgres` (Global Constraints).
9. **Owner actions outstanding (waiting on owner).** Checked with `gh api`:
   - **GitHub App `ostrium-labs-fork-bot`:** does not exist (`apps/ostrium-labs-fork-bot` returns 404). The organisation's installations are `coderabbitai`, `blacksmith-sh`, `depot-managed-runners` and `cloudflare-workers-and-pages`. Named as an owner action in Task 3. Its token is used by Task 9 (the `build-tools` digest PR), Task 13 (the pin PR on `ostrium-labs/loams`), Task 16 (dispatching `nf1-candidate.yml` here) and Task 18 (pushes to `ostrium-labs/postgres`). The fast-forward of `neon` `main` uses `GITHUB_TOKEN` and does not need it.
   - **Teams:** the organisation has `committers` and `maintainers` (members `dina-kar`, `Kesh3805`). `neon-maintainers`, `postgres-maintainers` and `security` do not exist. Needed by Task 5.
   - **A packages token:** `gh auth status` shows scopes `admin:public_key`, `gist`, `read:org`, `repo`, `workflow`. There is no `read:packages` (listing the organisation's packages returns 403) and no `write:packages`. The owner runs `gh auth refresh -h github.com -s write:packages,read:packages` before Task 1 can push. `admin:org` is also missing, so this session cannot read Actions billing or org-level Actions permissions.
   - **Package visibility (Q715): answered.** The mirrored packages stay **private** until Task 1's audit, whatever it finds. Changing a package to public is an owner action after the audit report.
   - **Runner budget (Q721): open.** The owner chose full maintenance mode (Q722's mode). Neither the $150-a-month cap for larger runners nor who holds billing has been answered. The organisation is on the `free` plan. Tasks 10 and 11 start on standard hosted runners (free for public repositories), which needs no budget. Any use of `RUNNER_HEAVY` (Blacksmith or Depot, both already installed) waits for Q721.
10. **Owner answers of 2026-10-09, recorded for the tasks that use them.**
    - **Q715:** mirrored images stay private until audited (Task 1; the plan's default).
    - **Q717:** Postgres 17 is the default at GA. 18 follows after its gate (Task 30); this is read as Q717's default, that 18 becomes the default for new projects once Task 30's gate passes, and Task 30 confirms it with the owner before flipping `default`. 19 stays beta until 2027 (consistent with NF1f's "GA no earlier than 2027-03" and Q718). The second half of Q717 (16 in maintenance until PG2 GA plus 6 months) was not addressed; Task 30 asks.
    - **Q719:** drop `pgrag`, `pg_mooncake` and `pg_duckdb` (Task 21, tier `dropped`). The other two parts (`plv8` and `rdkit` as `included`; GPL extensions with source artifacts) were not addressed; Task 21 uses the defaults and lists them as unconfirmed.
    - **Q722:** full maintenance mode. The named maintainers and CODEOWNERS teams are still open (Task 5).
11. **D800–D819 and Q715–Q729 belong to NF1 and nothing else.** `docs/design/13-decision-log.md` on `dev` has exactly one row for each ID. `git log --all -G` over the decision log finds them added only by `357d9ce2` (§51). No branch adds D820+ or Q730+. The next free IDs are **D820** and **Q730**.


### Hard-fork amendment rulings (2026-10-09, at `dev` `a3aeb560`)

Owner decision of 2026-10-09: "let the neon fork be hard fork, neon stopped open source after databricks acquisition and went silent over a repo, replace with our loams, let it be called loams-postgres not loams-neon." Checked read-only with `gh api` and the local clone; nothing was cloned or fetched.

12. **The repository is renamed.** `gh api repos/ostrium-labs/loams-postgres`: `full_name` `ostrium-labs/loams-postgres`, `fork: true`, `parent` `neondatabase/neon`, default branch `main`, public, not archived. `gh api repos/ostrium-labs/neon` answers with `ostrium-labs/loams-postgres` (GitHub's redirect). The tag `loams-decoder-trim-1` is still the annotated tag object `882f8b41`, pointing at `1218fb7a`. The local clone `~/Documents/Ostriumlabs/neon` is at `1218fb7a`, with `origin` `git@github.com:ostrium-labs/loams-postgres.git`. Ruling 3's facts otherwise stand under the new name.
13. **Still in Neon's fork network.** `fork: true` with parent `neondatabase/neon`. Detaching it is a GitHub support request by an organisation owner (Q730, owner action). Until then, PRs name `--repo ostrium-labs/loams-postgres --base main`.
14. **Token scopes unchanged.** `gh auth status`: `admin:public_key`, `gist`, `read:org`, `repo`, `workflow`. For NF1 that means `repo` and `workflow` only: no `write:packages` or `read:packages` (Task 1's mirror, every image push before the bot exists) and no `admin:org` (Task 5's teams). The GitHub App `loams-postgres-bot` (renamed from `ostrium-labs-fork-bot`, which was never created) is created by the owner later, or by the controller through GitHub's App manifest flow once the owner approves (Q733). Both stay "waiting on owner" (ruling 9).
15. **Task changes.** Task 1b is new (the rename, D823). Task 19 is removed and its number retired; NF1c is "Postgres minors and the gate" (Tasks 15–18). Task 3 creates no `loams/main`: it tags `neon-fork-point` on `fa504217` and fast-forwards `main` to `1218fb7a`. Task 4's base is `neon-fork-point`. Task 5's `NOTICE` adds "Based on Neon by Databricks/Neon Inc.". Task 6 adds the nightly fuzz job. Tasks 1, 2, 9–14, 29, 30, 34, 35, 40 and 41 use the new names (D822). Release tags are `lp-`, patch ids `LP-`; nothing had been released or recorded under `nf-` or `NF-`, so no tag or row is renamed.
16. **Name clashes for `loams-postgres` (D823).** None in the workspace: `crates/` has no `loams-postgres`; the planned `loams-pg-control` and `loams-pgdog-sync` and `deploy/loams-pg-bench` use `loams-pg-`; the desktop's `@loams/plugin-postgres` and stack id `postgres` are TypeScript; `loams.postgres.v1` is a proto package. Two constraints were found and are now in Task 1b: `deploy/neon/compose.yaml` sets `name: loams-neon`, which names users' Docker volumes, and `stacks.ts` uses one `dir: "neon"` for the dev source, the packaged resource and the per-user copy, so the rename splits it into `source` and `dir`. `deploy/loams-postgres` is not used because `deploy/helm/loams-postgres` and `deploy/loams-postgres-single` already name the product (Q731).
17. **Decision and question numbers.** The amendment uses D820–D824 and Q730–Q734, the next free ones (ruling 11). D802, D811 and D819 are marked superseded and D800, D801, D803, D804, D806–D810 and D818 amended in the decision log; Q723 is answered by D822.

### Task 1b rulings (2026-10-09, at `dev` `4d68cccc`)

1. **Clashes rechecked.** No crate, package or directory `loams-postgres` existed before the rename. `deploy/helm/loams-postgres` and `deploy/loams-postgres-single` do not exist yet; they are named only in §51 (§12.6, the planned deployments), which is still why the dev stack is `deploy/loams-postgres-dev`.
2. **The decoder's lockfile is rewritten, not re-resolved.** `cargo update -p pageserver_api -p postgres_ffi -p utils -p wal_decoder` against the new URL also moved unrelated picks (`itertools` 0.10.5 → 0.12.1 and `windows-sys` 0.59.0 → 0.61.2 in some dependents). The lockfile instead changes only the twelve `source = "git+https://github.com/ostrium-labs/…"` lines; `cargo metadata --locked`, `cargo tree` and `cargo test --locked` accept it, and the golden tests pass on byte-equal fixtures.
3. **`nf1-pins.yml` is created here** with one job, `names` (`scripts/nf1/tests/test_check_names.sh`, which runs `check-names.sh` on the repository and on generated fixtures). Task 2 adds `pins.py check` to it.
4. **`check-names.sh` is a little wider than the brief.** It also refuses `loams_neon::` and `deploy/neon` without a trailing slash, and skips `docs/` as a whole (the history keeps the old names, with the mapping note on top) and `scripts/nf1/` (its own patterns). `scripts/pg2/check-decoder-deny.py` keeps `ostrium-labs/neon` in its guard list and adds `ostrium-labs/loams-postgres`.
5. **The desktop splits `StackDef`.** `source` (`loams-postgres-dev`) is the shipped directory under `deploy/` or `resources/stacks`; `dir` (`neon`) is the per-user copy. `stackSource()` resolves the first; the test `copied_stack_dir_unchanged` covers both, and that compose runs from `<stacks>/neon`. The desktop's own compose project stays `loams-desktop-postgres`.
6. **Pre-existing:** `cargo deny check` in `crates/loams-wal-decoder` fails `advisories` (13 errors, `rustls 0.23.29`, `rustls-webpki 0.102.8` and others) on `dev` too, unchanged by this task; `bans`, `licenses` and `sources` pass. The root `cargo deny check` passes.


### Task 1 rulings (2026-10-11, at `dev` `1dc6e8a3`)

1. **The four images, all verified byte-identical and private.** Each was copied with `skopeo copy --all --preserve-digests`. `mirror-images.sh` then compared `skopeo inspect --raw` of source and mirror for the index and every child, including BuildKit's attestation manifests (SPDX SBOM and SLSA provenance). All four packages are `private` (`gh api orgs/ostrium-labs/packages?package_type=container`). GHCR accepts the nested `neon-archive/<name>` path, so no `neon-archive-<name>` fallback was needed.

   | Mirror (`ghcr.io/ostrium-labs/neon-archive/…:mirror-77e22e4b`) | Index digest (source = mirror) | Platforms (amd64, arm64) | Audit |
   |---|---|---|---|
   | `neon` | `sha256:7a4f124917bb929964b2d696d710f19584f80bb9bd51b2af4a6e2425434c761f` | `ead56a7b…`, `c1589ca5…` | CDDL: `inferno` 0.11.21 and 0.12.0 in 9 of the 10 `.dep-v0` binaries (all but `endpoint_storage`); private `subzero-core` 3.0.1 (git) in `proxy` and `pg_sni_router` |
   | `compute-node-v17` | `sha256:13ab146d3e7bbabb25a8532f315ac443e7512351d1ede0bab586def5c70e26c3` | `9b86e3ec…`, `b7821a6d…` | CDDL: `inferno` 0.11.21 and 0.12.0 in `local_proxy` |
   | `compute-node-v16` | `sha256:b3e151661bd2ee11eb2843c8926001966cb23969227e9673c5f42fc3fbe14249` | `203a5ae3…`, `44400466…` | CDDL: the same, in `local_proxy` |
   | `build-tools` | `sha256:bd1fe230b33aab4d7c2b336311aa8f96733a32a4eed4d61eecfb1c6b6d8cf6e3` | `94ec4434…`, `d5c55ee4…` | clean |

2. **`compute-node-v16` and `build-tools` were found through provenance.** For `compute-node-v16`, the tag is `8464`, Neon's `BUILD_TAG` for `77e22e4b`. Both platforms' SLSA provenance gives `GIT_VERSION` `77e22e4bf09d88b70b4a83a38c2f6de6301816b4` and `PG_VERSION` `v16`. The `compute-node-v17` index pinned by PG2 is the same tag `8464`. (`release-compute-8464` is a different build and was not mirrored.) For `build-tools`, the `neon` image's provenance (both platforms) names the material `ghcr.io/neondatabase/build-tools@d4a4001a…-bookworm` with digest `bd1fe230…`. That is the multi-arch index, and the tag `d4a4001a8381b2f2b33462941d4382ad5c71f4a39c12a4c66cec0397abd8d160-bookworm` still points at it.
3. **The audit reads more than `.dep-v0` (deviation).** Neon builds the compute image's Rust binaries with plain `cargo build` (only the storage `Dockerfile` uses `cargo auditable build`). An audit limited to `.dep-v0` therefore reported the compute images clean, although `local_proxy` links `inferno`. For an ELF file without `.dep-v0`, `audit_image.py` now lists the crates named by the Cargo source paths left in the binary (`registry/src/<index>/<crate>-<version>/`, `git/checkouts/<repo>-<hash>/<rev>/`). Those entries are marked `"evidence": "strings"` and are a lower bound. Tests: `audit_flags_cddl_without_dep_v0`, `audit_flags_subzero_checkout_without_dep_v0`. Tasks 10, 11 and 12 should still build every Rust binary with `cargo auditable`.
4. **`subzero`: in the `neon` image.** At `77e22e4b`, `proxy` depends on the path stub `libs/proxy/subzero_core` (Cargo.lock: `subzero-core` has no source). But Neon's `Dockerfile` (lines 66–71) runs `cargo add -p proxy subzero-core --git https://github.com/neondatabase/subzero --rev 396264617e78e8be428682f87469bb25429af88a` when the build secret `SUBZERO_ACCESS_TOKEN` is set, and Neon's CI set it. The `neon` image's `.dep-v0` records `subzero-core` 3.0.1 with source `git` in `proxy` and `pg_sni_router`, on both platforms. That is Neon's private code, so the `neon` mirror must never be made public, whatever happens to `inferno`. Our builds (Task 7, `check-no-private-sources.sh`) never pass that secret. The compute images have no `subzero`. `audit_image.py` flags a `subzero*` crate whose source is not `local`, and a `subzero` git checkout path.
5. **Licences other than CDDL.** No crate in any of the four audits has an AGPL, SSPL, BUSL, ELv2, Commons Clause or GPL licence on crates.io. Licences were looked up for every crates.io `name@version` and cached outside the repository. Non-Rust content (Postgres extensions in C, Debian packages) is out of Task 1's scope; it is covered by Tasks 10–12 (Syft and Trivy) and Task 20.
6. **Q715 applied.** `neon`, `compute-node-v17` and `compute-node-v16` have findings and stay private. `build-tools` is clean, but it is also private until the owner decides; it is a build image, not a runtime one. Changing any of them to public is an owner action. Nothing in Task 1 changes visibility.
7. **Deviations from the interface.**
   - Audit files are named `deploy/pins/audit/sha256-<hex>.json`, without a colon in the filename.
   - `audit-image.sh` stages the whole index once as an OCI layout and reads every platform from it.
   - `mirror-images.sh` takes `MIRROR_FROM=oci:<layout>:<ref>` to push from that staged layout. The verification still compares against the source registry. This halved the downloads over a slow link (about 0.6 MB/s): the compute images and `build-tools` were audited first, then pushed from the audited bytes. `neon` was copied registry to registry, then audited from the mirror.
   - `SKOPEO=<binary>` runs a local skopeo (1.24.1 here) in place of the pinned `quay.io/skopeo/stable@sha256:a585e4a3…` (v1.21.0) through podman. That keeps tool images off the full root disk; the default stays the pinned image.
   - `--verify-only` re-checks an existing mirror.
   - The crates.io licence lookup uses the crates.io API (`/api/v1/crates/<name>/<version>`), since the sparse index has no licence field.
8. **`pins.py check` refusing a `mirror:` source without its audit file belongs to Task 2,** which creates `pins.py`. `audit_runs_before_visibility` covers the README's runbook order now. `test_every_mirrored_index_has_an_audit_file` checks that each digest in the README's table has its file. Task 2 adds the pin-file half.
9. **CI.** `nf1-pins.yml` gains the job `mirror`. It runs `scripts/nf1/tests/test_mirror.py` with a `registry:3.0.0` service (pinned by digest), skopeo from apt and `rust-audit-info` 0.5.4. CI workflows are disabled for now (owner, 2026-10-10), so the suite was run locally against the same registry image. The workflow's header comment is reworded, because `check-names.sh` refused its literal old names (a failure already on `dev`).

### Task 2 rulings (2026-10-11, at `dev` `ac0c88d9`)

1. **The first content is Neon's reference for every image.** Task 1 made no package public (Task 1 ruling 6), so every pin keeps Neon's repository, with a `mirror:` source naming it: `fork_release = "mirror-77e22e4b"`, `fork_commit = "77e22e4bf09d88b70b4a83a38c2f6de6301816b4"`. `[majors]` is `supported = [17]`, `beta = []`, `maintenance = [16]`, `default = 17` (Q717). The images are `storage` (`ghcr.io/neondatabase/neon@sha256:7a4f1249…`, `pg_minors` 16.9 and 17.5, from `vendor/revisions.json` at `77e22e4b`), `compute-v17` (`…/compute-node-v17@sha256:13ab146d…`, 17.5) and `compute-v16` (`…/compute-node-v16@sha256:b3e15166…`, 16.9). `compute-v16` has no consumer yet; it is pinned because 16 is a maintenance major and `pins.py check` requires a compute image for each listed major.
2. **The neon-archive packages are never deployed (controller ruling after Task 1).** They stay private for good: the `neon` mirror holds Neon's private `subzero-core` (in `proxy` and `pg_sni_router`), and all three runtime mirrors hold CDDL `inferno`. `pins.py check` therefore:
   - refuses a pin whose `repository` names `ostrium-labs/neon-archive`, and `set` refuses such a release and writes nothing;
   - refuses any tracked file that names `ostrium-labs/neon-archive` outside the reference places: `docs/`, `scripts/nf1/`, `deploy/pins/audit/`, a pin's `mirror:` source (a reference to where the bytes are archived), and, in `deploy/loams-postgres-dev/README.md`, the Mirror section and the generated table that shows the sources.

   The dev and bench stacks still run Neon's original images from `ghcr.io/neondatabase` (the same bytes as the mirror), as on `dev` before this task. Neither runs `proxy` or `pg_sni_router`, and neither compute spec configures `local_proxy`. Nothing is published. Task 14 replaces these pins with our own builds.
3. **The release document (for Task 13).** `pins.py set --from-release <url|path>` reads a TOML document in the pin file's own schema, from an `https` URL or a local path. Plain `http` and other schemes exit 2. Task 13's release must attach that document as an asset, and the bot's PR passes the asset's URL. Without `--from-release`, `set` re-renders the pin file and its consumers from the current pin file.
4. **`check` is stricter than the contract's three rules.** It also requires:
   - the pin file to be byte-identical to `pins.py`'s own rendering (the file says "Do not edit by hand"; a comment or reordering fails);
   - `majors.default` to be one of `supported`, and no major to appear in two lists;
   - for every listed major, a `compute-v<N>` image whose `pg_minor` equals `storage.pg_minors["<N>"]`;
   - a fork source (`ostrium-labs/loams-postgres@<40 hex>`) to go with a `ghcr.io/ostrium-labs/loams-postgres-*` repository;
   - a `mirror:` source to carry the pinned digest (a mirror keeps digests), with `deploy/pins/audit/sha256-<hex>.json` present. That last rule is the pin-file half of `audit_runs_before_visibility` (Task 1 ruling 8).
5. **The README's image table is a consumer.** It is generated between the `pins.py` markers in `deploy/loams-postgres-dev/README.md`. The old table's amd64 manifest column is now one sentence of prose about the `77e22e4b` build, because the pin file holds index digests only.
6. **`majors.rs`.** `pins.py rust` writes `crates/loams-postgres/src/majors.rs`; once that file exists, `check` compares it and `set` rewrites it. This task does not create it (Task 29 owns it).
7. **CI and the e2e check.** `nf1-pins.yml` gains the job `pins` (`pins.py check`, then `test_pins.py`). It runs on every PR, so on every PR that touches the pin file or a consumer. CI workflows are disabled (owner, 2026-10-10), so the suite ran locally. `pg2-e2e.yml` was not run: that would mean pulling the images, and this machine's root disk is full. The generated lines are byte-identical to `dev`, since the digests did not change; only comments changed. `override_envs_still_work` runs `docker compose config --images` against both compose files, with and without `NEON_IMAGE` and `COMPUTE_IMAGE`.

### Task 3 rulings (2026-10-11, at `dev` `4a458a9f`)

Applied with `gh api` (account `dina-kar`, org admin; the token now has `admin:org`) and `git push` over ssh.

1. **`loams-postgres` refs.** `main` was fast-forwarded `fa504217` → `1218fb7a` (a plain push, before any ruleset existed). Annotated tags `neon-fork-point` (`fa504217`, "Neon's last public commit; Loams Postgres is a hard fork from here") and `loams-20261012-1` (`1218fb7a`) were pushed; the name keeps the plan's date although it was created on 2026-10-11. `main` stays the default branch. Pushing `main` registered Neon's 43 workflow files as `active`, but no run has started (0 runs after the push and after PR #1): the fork does not run them. LP-0002 (Task 7) deletes them; until then a PR into `main` should be watched for queued runs.
2. **The decoder's current pin is not on `main`.** Since the decoder-trim-2 round, `crates/loams-wal-decoder` pins `80356b29` (`loams/decoder-trim-2`, one commit on `1218fb7a`, annotated tag `loams-decoder-trim-2`). Task 3 fast-forwards `main` to `1218fb7a` only, as written; `80356b29` stays reachable through its `loams-*` tag, which `check-pinned-revs.sh` accepts. `main` is now PR-only, so moving `80356b29` onto `main` would be a merge PR (Task 4 or later; it also needs an `LP-` row, as `1218fb7a` does).
3. **Protection is rulesets, not classic branch protection** (rulesets take `loams/**` patterns through REST and are the organisation's convention, as on `ostrium-labs/loams`). On `loams-postgres`:
   - `loams: no force-push (main, loams/*)`: `non_fast_forward` on `refs/heads/main` and `refs/heads/loams/**`, **no bypass actors** (an admin cannot rewrite history either);
   - `loams: no deletion (main, loams/*)`: `deletion`, same refs except `refs/heads/loams/probe`, no bypass actors. The probe branch is left out so `force_push_rejected` can clean up after itself while being under the same force-push rule;
   - `main: PRs only, one code-owner approval`: `pull_request` (1 approval, code-owner review, stale reviews dismissed) on `main`, with `OrganizationAdmin` as a bypass actor in mode `pull_request` only: an admin can merge a PR without the approval, but cannot push to `main` directly;
   - `loams: pinned tags are immutable`: `deletion`, `non_fast_forward` and `update` on tags `neon-fork-point`, `loams-*` and `lp-*`, no bypass actors (the Global Constraints' "no deletion of a `loams-*`, `lp-*` or `neon-fork-point` tag", enforced). The same tag ruleset is on every fork below.
4. **Deviations from "as in the shared contract".**
   - **No required status checks yet.** `loams-gate / g0`, `g1`, `g2`, `dco` and `patches` do not exist; requiring them now would block every PR. Each is added to the `main` ruleset by the task that creates its workflow (Task 4 `patches`, Task 5 `dco`, Task 16 `g0`/`g1`). `g2` runs only when triggered, so it cannot be a plain required check; Task 16 decides how it gates.
   - **The approval rule is bypassable by org admins in PRs.** The organisation has two members and no `loams-postgres-maintainers` team or CODEOWNERS file yet (Task 5, Q722, Q734), so a strict one-approval rule would stop every fork PR on one person. Removing the bypass once the team has two maintainers is an owner decision.
   - **`loams/*` work branches get no-force-push and no-deletion only, not the PR rule.** A PR rule on work branches would make them unpushable; §3.5's "a PR, one CODEOWNER approval" applies to `main`.
5. **The other forks.** `loams/<the branch Neon used>` was created at that branch's head, with a ruleset `loams/*: no force-push, no deletion` (`non_fast_forward` and `deletion` on `refs/heads/loams/**`, no bypass) and the tag ruleset:

   | Fork | Branch | At |
   |---|---|---|
   | `rust-postgres` | `loams/neon` | `f3cf448f` (the decoder's pin) |
   | `azure-sdk-for-rust` | `loams/neon` | `39ab8c6f` (Neon's `Cargo.toml` used branch `neon`) |
   | `framed-websockets` | `loams/main` | `34eff3d6` |
   | `tokio-epoll-uring` | `loams/main` | `478ab1a3` |
   | `pg_session_jwt` | `loams/main` | `937a79a6` (Neon built tag `v0.3.1`; the branch is `main`) |
   | `autoscaling` | `loams/main` | `a4bce09e` |

   `autoscaling` and `framed-websockets` were then archived. `azure-sdk-for-rust` stays unarchived. Default branches are unchanged.
6. **`postgres`.** Actions disabled (`PUT actions/permissions {enabled:false}`; now `{"enabled":false}`). Rulesets: `REL_*_STABLE_neon: frozen history` (`update`, `non_fast_forward`, `deletion` on `refs/heads/REL_*_STABLE_neon`, no bypass; `REL_14…18_STABLE_neon` match, the point-release `_neon` branches such as `REL_17_5_neon` do not), `loams/*: no force-push, no deletion`, and the tag ruleset. Neon's submodule revisions (17.5 `1e01fcea`, 16.9 `a42351fc`) are ancestors of the now-frozen `_neon` heads (Task 0 ruling 4), so no tag was added for them.
7. **`check-pinned-revs.sh`.**
   - **The compare direction is reversed from the plan.** GitHub's `compare/<base>...<head>` reports `behind` when *head* is behind *base*. `compare/<rev>...main` is therefore `ahead` (not `behind`) when `rev` is on `main`. The script asks `compare/<branch>...<rev>` and accepts `behind` or `identical`, which is the plan's intent (checked live: `compare/main...1218fb7a` was `ahead` before the fast-forward and `identical` after).
   - **Target branches** are fixed in the script: `main` for `loams-postgres` (and the old name `neon`), `loams/neon` for `rust-postgres` and `azure-sdk-for-rust`, `loams/main` for the other small forks.
   - **Other `ostrium-labs` repositories are listed as `skip`, not checked.** `Cargo.toml` here also pins `ostrium-labs/resonate` (`e3606698`, only on the unprotected branch `loam/0.10.1`) and `ostrium-labs/client-rust` (`1f8962b0`, only on the unprotected branch `loam`). They are not NF1 forks and have no `loams/` branch; checking them would fail today. Protecting them (a `loams-*` tag or a ruleset) belongs to their own tracks.
   - It reads `Cargo.toml` only (not `Cargo.lock`), both inline tables and `[dependencies.x]` tables; a `loams-*` tag counts when it points (through annotated tag objects) at the rev; any `gh` error fails the rev.
   - The job `revs` in `nf1-pins.yml` runs it and `test_pinned_revs.py` with `GH_TOKEN: github.token`.
8. **Test results.** `check-pinned-revs.sh .` on `dev`: `ok loams-postgres 80356b29 (tag loams-decoder-trim-2)`, `ok rust-postgres f3cf448f (identical loams/neon)`, `skip` for `resonate` and `client-rust`. `force_push_rejected` (opt-in, `NF1_PROBE=1`): the non-fast-forward push to `loams/probe` was refused with `GH013 … Cannot force-push to this branch`, and the probe was deleted.
9. **Owner actions still open:** the teams and `loams-postgres-bot` (Q733), detaching the fork network (Q730), and the admin bypass on `main` (ruling 4).

### Task 4 rulings (2026-10-12, at `dev` `ce9e6811`)

Fork PR ostrium-labs/loams-postgres#2 (branch `loams/nf1-patch-queue` from `main` `1c29d403`: `0daa3c0e` LP-0000, `de3f8317` LP-0032), merged into `main` as the merge commit `119e34a4` with the organisation admin's PR bypass (Task 3 ruling 4; no second maintainer exists). CodeRabbit: no actionable comments. `loams/nf1-governance` was already merged by Task 3's PR #1, so Task 4 used a new work branch.

1. **`LOAMS_PATCHES.md`.** The plan's seven columns (id, title, area, owner, since, origin, drop when) under `## Patches`. **Origin** is one of `loams-only`, `upstreamable` (a general fix another Neon fork could take as is; nothing is sent upstream, D820) or `cherry-pick` (needs the `(cherry picked from neondatabase/neon <40 hex>)` line). That column is the patch's "upstreamable" flag; no eighth column was added. These values supersede §51 §3.4's `loams` / `cherry-pick neondatabase/neon <sha>` (the sha moves into the commit message), and §3.4 now says so. A dropped patch keeps its row (its "drop when" then names the revert): its commits stay in history, so removing the row would fail the check. Rows: LP-0000 (governance: charter, patch queue; `ci`, `loams-only`), LP-0001 (decoder trim; `build`, `upstreamable`), LP-0032 (ruling 4). The file's header says ids LP-0002 to LP-0031 stay reserved for the tasks this plan gave them (LP-0018 retired), so new, unplanned patches take the next id after LP-0031.
2. **`1218fb7a` has no trailer and cannot be rewritten** (Task 3 ruling 1). `LOAMS_PATCHES.md` has a second table, `## Commits without a trailer` (commit as 40 hex, id, why), that maps it to LP-0001. This amends the Task 4 interface ("fails when a commit has no `Loams-Patch` trailer"): a commit listed there passes. An entry outside `neon-fork-point..HEAD` is a finding, so the table cannot go stale silently.
3. **`check-patches.py`** (stdlib only): `--base neon-fork-point --head HEAD`, plus `--repo`, `--patches`, `--parse-only`, and `--allow-sync-merges` for `ostrium-labs/postgres` (a merge carrying `Loams-Sync:` exempts the commits it brings in, i.e. everything reachable from its other parents; without the flag, or without the trailer, those commits are checked like any other). Merge commits are never patches; the commits they bring in are checked. Stricter than the brief: one id per commit (two different `Loams-Patch` values fail), a malformed id fails, a commit with the cherry-pick line must belong to a `cherry-pick` row, and the file's format is checked (columns, duplicate ids, `@handle` owners, ISO dates, the origin vocabulary; exit 2). Exit 1 lists every finding. `test_check_patches.py`: 25 tests on scratch repositories, including the six named ones. On the fork's `main` after the merge (`119e34a4`): `ok: 4 commits, 3 patches in neon-fork-point..origin/main`.
4. **The first real patch, LP-0032: least privilege for spec roles (PG2 R11.9's fork half).** `compute_ctl` created every spec role it did not find as `CREATE ROLE … INHERIT CREATEROLE CREATEDB BYPASSRLS REPLICATION IN ROLE neon_superuser` (`compute_tools/src/spec_apply.rs`, `CreateAndAlterRoles`). Now:
   - `compute_api::spec::Role` gains `privileged: bool` (`#[serde(default, skip_serializing_if = "Not::not")]`): absent means false, specs written before it still parse, and an unprivileged role serializes exactly as before. Neon's `compute_ctl` ignores unknown fields, so a spec carrying it is safe on the `77e22e4b` images too.
   - A role is created as `CREATE ROLE <name> <spec options> LOGIN PASSWORD …`: Postgres's defaults (`NOSUPERUSER NOCREATEDB NOCREATEROLE INHERIT NOREPLICATION NOBYPASSRLS`, no membership) plus exactly what the spec's `options` ask for. `privileged: true` keeps Neon's statement byte for byte. A role named in `local_proxy_config.jwks` is plain whatever the flag says, as before.
   - The flag applies at creation only. The `ALTER ROLE` path for an existing role is unchanged (password and `LOGIN`), so a role created privileged stays so, and making an existing role privileged needs a new role. No Loams role exists outside tests (PG2 R6.2), so there is nothing to demote.
   - `neon_local`'s `test` user and the two `test_compute_catalog.py` specs that want "grants equivalent to our real environment" set `privileged: true`, so the fork's Python suite keeps its expectations (it runs in the fork's CI, Task 16; not run locally). The compute's OpenAPI schema documents the field.
   - Tests (`cargo test -p compute_tools --lib spec_apply::tests`, `cargo test -p compute_api`): `spec_role_gets_least_privilege_by_default`, `privileged_spec_role_keeps_neon_admin_attributes`, `spec_role_gets_the_options_it_asks_for`, `jwks_role_is_never_privileged`, `existing_role_is_only_altered` (through `get_operations(CreateAndAlterRoles)`), and `role_privileged_is_opt_in` (serde).
   - Origin `loams-only`: Neon's control plane relies on privileged spec roles, so the flipped default is ours. Drop when: never (Loams' role model).
5. **For PG2 Task 24.** Loams' spec builder (`crates/loams-pg-control/src/upcall/spec.rs`, with `loams_postgres::spec::Role` of `name` and `encrypted_password` only) needs no change for least privilege: it sends no `privileged`, so every API role, and `loams_ro`, gets `LOGIN` and nothing else once Loams runs our compute image. Task 24 adds `loams_ro`'s grants (Task 0 ruling 8) and decides whether any API role is `privileged` (a project owner role, if the API offers one); `CreateRole` has no such field today. **It takes effect only on our own compute images (Task 11, switched in Task 14):** the pins still name Neon's `77e22e4b` compute, which creates privileged roles. So no pin moves in this task: the pin file pins image digests, and no image of ours exists yet.
6. **No required check yet.** Actions are disabled on `loams-postgres` (`{"enabled":false}`, controller, Task 3), so `loams-patches.yml` does not run and `patches` is not added to the `main` ruleset (24863579): a required check that never reports would block every PR. Whoever enables Actions adds `patches` as a required check in the same step. The workflow and the check were run locally.
7. **`80356b29` (the decoder's pin, `loams/decoder-trim-2`) is still not on `main`.** Merging it needs an LP id and, because it has no trailer, a row in `## Commits without a trailer`. Left for the task that next moves the decoder's rev.

### Task 5 rulings (2026-10-12, at `backend/nf1-t4` `0e21d7f2`)

Fork PRs, all merged with merge commits by the org admin's PR bypass: ostrium-labs/loams-postgres#3 (branch `loams/nf1-security` from `main` `119e34a4`, commits `5a4800ca` and `2ba913b0`, merged as `be151ad1`), ostrium-labs/postgres#1 (into `loams/meta`, `efecf23b`), ostrium-labs/tokio-epoll-uring#1 (`2d0adc85`) and ostrium-labs/pg_session_jwt#1 (`26ec87f9`) (into `loams/main`). Every commit is signed off and carries `Loams-Patch: LP-0000`. The three small-fork commits were made with the git data API (blobs, tree, commit, ref), because those repositories have no local clone and none was made.

1. **No teams were created; CODEOWNERS uses the organisation's admins.** The plan lists the three teams as an owner action (Q722, Q734), and `codeowners_valid` needs `codeowners/errors` to be `[]`, which an owner that does not exist fails ("Unknown owner"). Every file keeps §51 §3.5's sections: storage, compute and CI; Postgres (`/vendor/`, `/pgxn/`, `/compute/`); security (`/deny.toml`, `/.github/`, `/LOAMS_PATCHES.md`, `/NOTICE`). Each section names the team that will own it in a comment, and each is owned by the three org admins (`@dina-kar`, `@Kesh3805`, `@ostrium-tech`) until then. Swapping in a team is a one-line edit per section. The security section also owns `/CODEOWNERS` and `/SECURITY.md`. In the small forks, `*` names the future `loams-postgres-maintainers` (`postgres-maintainers` in `postgres`), and `/.github/` (and `/deny.toml` in `tokio-epoll-uring`) names `security`. `codeowners/errors` is `[]` on all four branches; Neon's file on `loams-postgres` `main` had 22 errors (its `@neondatabase/*` teams). With CODEOWNERS present, `main`'s ruleset (code-owner review) now needs an approval from one of the admins other than the author, or the admin PR bypass (Task 3 ruling 4).
2. **`check-dco.sh` exempts the trailer exception table.** `1218fb7a` has no `Signed-off-by` either, and cannot be rewritten, so a check from `neon-fork-point` failed on it. The script reads the 40-hex shas of `LOAMS_PATCHES.md`'s `## Commits without a trailer` section and skips them (one list of pre-rule commits, not two); a sha elsewhere in the file exempts nothing. The table is read from a trusted revision, `EXEMPT_FROM` (default `HEAD`; the workflow passes the pull request's base commit), so a pull request cannot exempt its own unsigned commit by adding a row (CodeRabbit on fork PR #3, fixed in `2ba913b0`). The sign-off is matched by the author's email, as in Loams' original; CodeRabbit's request to also match the name was declined, since the copy should not diverge from Loams' rule alone. `BASE` defaults to `neon-fork-point` and `HEAD` to `HEAD`, and the workflow runs it over `neon-fork-point..HEAD` (on a PR, GitHub's merge commit; merges are skipped). One fix to the copy: the range is computed in an assignment, so a missing ref fails instead of checking an empty range (the Loams original's `for sha in $(git rev-list …)` passes on a bad ref). The test is `scripts/loams/tests/test-dco.sh` (Loams' six cases plus: Neon history exempt, an unsigned commit fails, the table exempts it, the table in the wrong section does not, a bad ref fails). `test_governance.py` runs it as `dco_rejects_unsigned`, beside `notice_retains_upstream` and `codeowners_valid`.
3. **No required check.** `loams-dco.yml` has the jobs `dco` and `governance`. Actions are disabled on `loams-postgres` (Task 4 ruling 6), so neither is added to the `main` ruleset; whoever enables Actions adds `dco` (and `patches`) in the same step.
4. **Security contact and private reporting.** `SECURITY.md` names `security@ostriumlabs.org` and 3 business days, as §51 §3.7 does (Loams' `SECURITY.md` uses the same address); no other address was invented. Each `SECURITY.md` links Loams' policy (`ostrium-labs/loams/security/policy`) and its own repository's private reporting. Private vulnerability reporting was off on every fork and is now enabled (`PUT …/private-vulnerability-reporting`) on `loams-postgres`, `postgres`, `tokio-epoll-uring`, `pg_session_jwt`, `rust-postgres` and `azure-sdk-for-rust` (§3.7: "every fork"); `autoscaling` and `framed-websockets` are archived and were left alone. Response targets (Q724) are not stated: they are Task 6's. `postgres`'s file sends vulnerabilities in PostgreSQL itself to the PostgreSQL Security Team and ones in the fork's changes to us.
5. **`postgres`: `loams/meta` was created at `REL_17_STABLE_neon`'s head (`56692dfb`)**, as the plan says, and the files go there by PR. **This amends Task 15:** `loams/REL_17_STABLE` should start from `loams/meta`'s tip (one commit on `56692dfb`) rather than "unchanged", and Task 15 adds the same two files to `loams/REL_16_STABLE` and `loams/REL_18_STABLE`.
6. **`NOTICE`** keeps Neon's five lines unchanged and appends the plan's text one sentence per line ("Loams Postgres." / "Based on Neon by Databricks/Neon Inc. (…), licensed under the Apache License 2.0." / "Modifications Copyright 2026 Ostrium Labs and the Loams Authors." / "Changes from Neon are listed in LOAMS_PATCHES.md."). Shipping it in images under `/usr/share/doc/loams-postgres/` stays with Tasks 10 and 11.
7. **`LOAMS_PATCHES.md` and `LOAMS.md`.** LP-0000's title now covers this task ("…; CODEOWNERS, DCO check, SECURITY.md, NOTICE"), and the exception table says the DCO check uses it too. `LOAMS.md` names the DCO script, CODEOWNERS and SECURITY.md in one sentence.
8. **Owner actions still open:** create `loams-postgres-maintainers`, `postgres-maintainers` and `security` with at least two maintainers each, then swap them into the four CODEOWNERS files (Q722, Q734); the admin bypass on `main` (Task 3 ruling 4).

### Task 6 rulings (2026-10-12, at `backend/nf1-t5` `ec42b180`)

Fork PR ostrium-labs/loams-postgres#4 (branch `loams/nf1-advisories` from `main` `be151ad1`, commits `dc0d2f47` and `83f236ea` (review fixes), `Loams-Patch: LP-0008`), merged into `main` as the merge commit `bea29e18` with the organisation admin's PR bypass. CodeRabbit: 2 Minor, both fixed.

1. **Targets (Q724 still open; its defaults applied).** `sev:critical` 72 hours, `sev:high` 7 days, `sev:pg-minor` 7 days after the upstream release, `sev:other` 30 days (D805's figure; §3.7's "next monthly roll-up" is read as 30 days). The clock starts at the Postgres release date, at a GitHub advisory's publication, and otherwise when the daily run first sees the finding. Fork `SECURITY.md` now states the targets (Task 5 ruling 4 left them to this task).
2. **Severity.** `sla.py` computes CVSS 3.x base scores itself: 9.0 and above critical, 7.0–8.9 high, below other. A RUSTSEC vulnerability with no CVSS 3 score (none, or CVSS 4.0 only) is `high` until someone triages it; informational RUSTSEC entries (unmaintained, unsound, notice) are `other`. Trivy findings use the highest `V3Score` of any vendor, or Trivy's own severity when there is no score. GitHub advisories use `cvss_v3`, or their severity. A fuzzer memory error (an AddressSanitizer or MemorySanitizer report) is `critical`; any other crash, timeout or OOM is `high`. Changing the `sev:` label re-dates the issue from the same start (`sla.py sweep`), and an issue filed by hand with a `sev:` label gets a due date from when it was created.
3. **Every finding goes to a private tracker, not the fork's issues (deviation).** Issues are disabled on `loams-postgres` (`has_issues: false`). A fuzzer crash is an undisclosed vulnerability ("a private security issue"), and a public list of our images' unpatched CVEs helps nobody, so `sla.py` files every finding into one private repository: `vars.LOAMS_SLA_REPO`, default `ostrium-labs/loams-postgres-security`. `sla.py` refuses (exit 2) to file a private finding into a public repository. The fuzzer's output goes to a file, never to the public run log or an artifact; the reproducer (base64, up to 16 KiB) and the output go into the private issue. **Waiting on owner:** create the private repository, set the secret `LOAMS_SLA_TOKEN` (issues and labels write on it; `GITHUB_TOKEN` cannot write to another repository) or use the bot App (Q733), and turn Actions on in the fork's settings (`actions/permissions`).
4. **Deduplication and escalation.** Each issue body carries a marker with the finding's key (`rustsec:<id>`, `cve:<id>` (one issue per CVE across images and packages), `ghsa:<id>`, `pg-release:<date>`, `fuzz:<target>:<signature>`). A key that is already known, open or closed, is never filed again, and a closed issue is not reopened. A crash's signature is where it panicked (without the machine's paths) plus the first frame in our own code when the panic is in std or a dependency (`RUST_BACKTRACE=1`), or else the sanitizer's summary. Escalation adds `overdue` and posts one comment naming `@ostrium-labs/security` (not created yet, Task 5 ruling 8) and the owners of `/SECURITY.md` in CODEOWNERS, who stand in until the team exists. `--max-new 50` per run, most severe first: the first runs against Neon's `77e22e4b` images would otherwise open hundreds of issues at once.
5. **Sources as built.** cargo-deny 0.20.2 and Trivy 0.74.0, both checked by sha256 (Trivy 0.75.0 was only 11 days old). The pin file is read from `ostrium-labs/loams`'s default branch (`dev`). Until Task 14 that file pins Neon's images, so the first runs will file their known CVEs, most of them overdue at once: that is the backlog Task 14 clears. `pg-security-feed.py` reads `versions.rss` (the latest minor of each major, with its date), because `news.rss` keeps only the last ten news items; a release counts as a security minor when its release notes name a CVE. Beta majors are not tracked. There is one finding per release day. Run today, it finds `pg-release:2026-08-13` (17.11 and 16.15), due 2026-08-20 and already overdue: that is Task 17's catch-up. GitHub advisories: `scripts/loams/advisory-watch.txt` (pgdog; `rust-postgres/rust-postgres`, `Azure/azure-sdk-for-rust`, `neondatabase/tokio-epoll-uring` and `neondatabase/pg_session_jwt`, the upstreams of the unarchived small forks; `neondatabase/neon`, labelled `upstream-signal`), plus each extension's GitHub source from `compute/loams-extensions.toml` once Task 20 writes it, published in the last 30 days.
6. **Fuzzing as built.** `fuzz/` is a member of the fork's workspace (crate `loams-fuzz`), so it shares `Cargo.lock` and the root `[patch]`. A separate workspace would need its own copy of the `neondatabase/rust-postgres` patch for Task 7 to repoint. The proxy's parsers are crate-private, so the proxy gains a `fuzzing` feature, enabled only by the fuzz crate, that adds `proxy::fuzzing`. New dependencies: `libfuzzer-sys` 0.4.13 (2026-06-04) and `arbitrary` 1.4.2 (2025-08-14; 1.5.0 is 6 days old). `deny.toml` allows NCSA (LLVM's libFuzzer) for `libfuzzer-sys` only. The nightly is `nightly-2025-05-10` (1.89.0-nightly, the fork's 1.88.0 era): `nightly-2026-09-15` cannot compile the proxy ("overflow evaluating the requirement … Send" in `serverless/mod.rs`). Bump it together with `rust-toolchain.toml`. The proxy targets run **without** AddressSanitizer. The workspace's `tikv-jemallocator` features make jemalloc replace `malloc`, and under ASan the proxy targets crash with SIGSEGV before libFuzzer starts. Panics and hangs are still found, and the parsers are safe Rust. `cargo fuzz` keeps overflow checks on, so an overflow panic is filed as `high` and can be re-triaged.
7. **Verification (Actions are disabled in the fork's settings: `actions/permissions` is `enabled: false`, so no workflow runs).** 29 tests in `test_sla.py`, including the six named ones; `fuzz_targets_build` was run locally with `cargo +nightly-2025-05-10 fuzz build`. Dry runs on live data: the feed, real `cargo deny` output, the watch list, `sweep`, and a fuzz finding against the public fork, which was refused. Each target was run locally for 60 seconds. Two targets found crashes within that time. They are not described here: they will be filed privately once the tracker exists. Until then the reproducers are kept outside any repository. `actionlint` is not installed; both workflows were parsed as YAML and their shell steps checked with `bash -n`.
8. **No required check.** As in Task 4 ruling 6: whoever enables Actions decides whether `loams-advisories / tests` and the fuzz build gate pull requests.

### Task 7 rulings (2026-10-13, at `dev` `21cd2b5f`)

Fork PR ostrium-labs/loams-postgres#5 (branch `loams/nf1-hygiene` from `main` `bea29e18`), merged into `main` as the merge commit `864cc63e` with the organisation admin's PR bypass. One commit (or more) per patch id: `33a2ba84` and `074ee639` LP-0002, `3e395b31` LP-0003, `abe092ca` LP-0004, `cd1bc4b1` and `e8ffc45c` LP-0005, `5f15acc3` and `ce5c7350` LP-0006, `e2ff0f39` and `941c2c32` LP-0008. CodeRabbit (one round, 5 comments): 3 fixed (case-insensitive matching, the hygiene test also runs `cargo deny check sources`, `sla.py` filters per finding), 2 declined with reasons (ruling 6's compose stack; the benches README path is right for `cargo bench`, checked in criterion 0.5.1 and pprof 0.14).

1. **LP-0002.** `scripts/loams/strip-upstream-ci.sh` deleted Neon's 43 workflows. The scripts only they used (`.github/scripts/`, `.github/file-filters.yaml`) went with them, and `.github/actionlint.yml` now lists our workflows' variables (`LOAMS_SLA_REPO`) instead of Neon's 36 variables and runner labels, so `actionlint` passes on every `loams-*.yml`. `.github/actions/` stays for Task 16 to reuse or delete, except `prepare-for-subzero` (LP-0004). `.github/ISSUE_TEMPLATE` and the PR template are Neon's and untouched (issues are off on the fork).
2. **LP-0003.**
   - pprof is built with `["criterion", "frame-pointer", "prost-codec"]` and jemalloc_pprof with `["symbolize"]`. Cargo.lock drops `inferno` 0.11.21 and 0.12.0 and the crates only they used (arrayvec, num-format, quick-xml 0.26 and 0.37, rgb, str_stack).
   - `CDDL-1.0` is out of `deny.toml`, with a comment saying why.
   - The profiling routes answer `format=svg` with 400 and exactly the plan's body, `{"error":"svg flamegraphs are not built; use format=pprof"}`, before any profile is taken. That body is not Neon's usual `ApiError` shape (`{"msg": …}`).
   - The five Criterion benches write `Output::Protobuf`, and `pagebench --profile` writes `profile.pb`.
3. **LP-0004.** Besides the `SUBZERO_ACCESS_TOKEN` blocks, the build stage no longer copies the plan stage's `proxy/Cargo.toml` and `Cargo.lock`, which only carried the `cargo add`. `pgxn/neon/communicator`'s `rest_broker = []` feature stays: it is a feature definition, not a CI feature list. `proxy/README.md` keeps the auth broker instructions (without `--is-rest-broker`) and gains "The REST broker (Data API) is not supported" (Q720's default).
4. **LP-0005.**
   - All four `rust-postgres` crates and the `[patch.crates-io]` entry use one rev, `f3cf448f`, the head of `loams/neon`. The lock had `postgres` alone at `1f21e795`, the branch's head when that crate was last updated. `f3cf448f` is 4 commits ahead and changes nothing under `postgres/`, so the code built is the same.
   - `azure-sdk-for-rust` is pinned at `f64bd572`, `tokio-epoll-uring` at `781989bb` (moved by Task 8), and `framed-websockets` at `34eff3d6`.
   - Annotated tags `loams-20261013-1` were created on `rust-postgres`, `azure-sdk-for-rust`, `tokio-epoll-uring` and `pg_session_jwt` (`6b08baaf`, the `v0.3.1` commit LP-0006 downloads). `framed-websockets` is archived, so it is read-only and cannot take a tag; its rev is the head of its protected `loams/main`.
   - Cargo.lock changes only its source lines.
   - `deny.toml`'s `allow-org` is `ostrium-labs`, and `unknown-git = "deny"`. A probe that pointed one dependency back at `neondatabase` failed `cargo deny check sources`.
5. **LP-0006.**
   - `pg_session_jwt` now downloads from the fork's `v0.3.1` tag. The tarball's sha256 is unchanged (`62fec9e4…`, downloaded and compared).
   - `pgrag` is dropped (Q719), with its stages, onnxruntime, `compute/patches/onnxruntime.patch` and `docker-compose/ext-src/pgrag-src`. `compute/manifest.yaml`'s `neon.unstable_extensions` still names `rag`; Task 21 regenerates it.
   - `lcov` is **removed**, not taken from Debian. It served Neon's coverage reports, which went with LP-0002, and the CPAN modules it needed are gone too. `scripts/coverage` needs a local lcov.
   - `ARG REPOSITORY=ghcr.io/ostrium-labs` in the `Dockerfile` and `docker-compose/compute_wrapper/Dockerfile`.
6. **`check-no-private-sources.sh` is wider than the plan.**
   - It also checks every `Cargo.toml` and `.gitmodules`.
   - It matches `neondatabase` (not only `neondatabase/`), ignoring case, so `ghcr.io/neondatabase` without a slash and `neondatabase-labs` fail too.
   - Full-line comments are allowed. The allowlist (`scripts/loams/private-sources-allowlist.txt`) is empty. `vendor/` is not checked.
   - Not covered, because it is neither a Dockerfile nor a workflow: `docker-compose/docker-compose.yml`, Neon's extension-test stack. Its default `REPOSITORY` is still `ghcr.io/neondatabase` and its image names are Neon's. It moves to our images with Task 16.
7. **Tests.** `scripts/loams/tests/test_hygiene.sh` has the named tests:
   - `upstream_ci_absent`;
   - `deny_rejects_cddl` (`cargo deny check licenses sources` passes, and a CDDL-1.0 fixture crate fails);
   - `no_inferno` (`cargo tree --locked --target all -e features -i inferno` finds nothing);
   - `no_private_sources` (passes on the branch, and fails on `neon-fork-point` in Cargo.lock, the Dockerfiles and the workflows; plus fixtures).

   A new `.github/workflows/loams-hygiene.yml` runs it (a deviation: the plan leaves that to G0; Task 16 folds it in). Also run locally:
   - `cargo test -p http-utils --lib` (5 passed, including `profile_cpu_svg_is_refused` and `profile_heap_svg_is_refused`);
   - `cargo check --locked -p pageserver -p proxy -p compute_tools -p storage_controller` on the new sources;
   - `cargo check --benches` for postgres_ffi, utils, wal_decoder and pageserver;
   - `test_sla.py` (31), `test_check_patches.py`, `test_governance.py`, `check-patches.py`, `check-dco.sh` and `actionlint` 1.7.7.

   `safekeeper`'s benches need `walproposer`, which needs a full Postgres build, so they were not checked locally.
8. **Pre-existing, for Task 16's G0.** `cargo fmt --check` and `cargo clippy -D warnings` over the whole workspace already fail at `neon-fork-point`, in Neon's `libs/remote_storage/src/gcs_bucket.rs`: unformatted imports and about 40 unused-import warnings. G0 must fix that file or scope those checks. Every file this task changed passes `rustfmt --check`.
9. **LP-0008 follow-up (preparing for Actions).** The run logs and run states of `loams-postgres` are public. The changes:
   - `sla.py` prints nothing for a private finding. Before, `file --kind fuzz` printed the finding's key, which names the crash site. Tests: `test_fuzz_cli_keeps_the_finding_out_of_the_log` and `test_public_findings_are_still_printed_beside_private_ones`.
   - `loams-fuzz.yml` no longer echoes the fuzzer's exit status.
   - The filing step runs on every scheduled run, because a skipped step said "no crash".
   - The job fails only when filing fails.
   - The fuzz step waits out its time budget after an early stop.
   - A `tracker` job skips scheduled fuzzing while `LOAMS_SLA_TOKEN` is unset.

   **Owner, before enabling Actions:** a pull request's workflow can restore the default branch's Actions caches, including the fuzz corpus. Keep "Require approval for all outside collaborators" for fork pull request workflows.
10. **Not built here.** The Dockerfile changes were reviewed but not built: images build in CI (Tasks 9–11), and Actions are disabled on the fork. Until Task 9, the storage `Dockerfile`'s `$REPOSITORY/$IMAGE:$TAG` (`ghcr.io/ostrium-labs/build-tools:pinned`) does not exist.

### Task 8 rulings (2026-10-13, at `dev` `ffe1e091`)

`ostrium-labs/tokio-epoll-uring` PR #2 (branch `loams/nf1-licence`, one commit `1d8152ab`, `Loams-Patch: LP-0007`, made with the git data API: there is no local clone, and none was made) merged into `loams/main` as `5ab61ac8`, tagged `loams-20261013-2`; CodeRabbit: no actionable comments. Fork PR ostrium-labs/loams-postgres#6 (branch `loams/nf1-uring-licence`, `7b26e124`) merged into `main` as `1b928b56`; CodeRabbit was rate-limited (one review an hour on the plan), so it was self-reviewed.

1. **The texts, as declared (Q716's default; no licence chosen or changed).** `LICENSE-MIT` ("Copyright (c) 2023-2026 the tokio-epoll-uring authors (Neon Inc.)") and `LICENSE-APACHE` (the canonical apache.org text, sha256 `cfc7749b…`) at the root. `uring-common/LICENSE-tokio-uring` is `tokio-uring`'s `LICENSE` at `d5e90539bd6d1c518e848298564a098c300866bc`, unchanged: MIT, "Copyright (c) 2021 Carl Lerche". `LICENSING.md` maps the files: the vendored ones are `uring-common/src/buf.rs`, `src/buf/*.rs` and `src/open_options.rs`, each starting with `tokio-uring`'s notice (upstream PRs #24 `91d67e20` and #25 `53c832d3`); `lib.rs`, `io_fd.rs` and `open_options_io_uring_ext.rs` are Neon's (MIT, under `LICENSE-MIT`).
2. **The pin moved from `781989bb` to `5ab61ac8`** (LP-0005's `tokio-epoll-uring` rev; only the two Cargo.lock source lines change, the crates' manifests are the same). That brings Task 0 ruling 6's five upstream commits (`7adcb121` … `478ab1a3`, the cancelled-`open` file-descriptor leak) plus Task 5's `CODEOWNERS`/`SECURITY.md`. Checked locally in place of G1: tokio-epoll-uring's own workspace tests at `5ab61ac8` (20 + 2 passed, including the cancelled-open tests), and `cargo test -p pageserver --lib virtual_file` with `NEON_PAGESERVER_UNIT_TEST_VIRTUAL_FILE_IOENGINE=tokio-epoll-uring` and `=std-fs` (19 passed each).
3. **`docs/loams/LICENSING-tokio-epoll-uring.md`** in the fork records the finding, the resolution and what is open. `LOAMS_PATCHES.md` row LP-0007 (drop when upstream publishes licence files that match its manifests).
4. **Q716 is still open: nothing was posted upstream.** Filing the one issue on `neondatabase/tokio-epoll-uring` is an exception to D231 the owner has not made. The fallback (`io-uring` feature, `loams/nf1-no-uring`) was not started: no counsel finding says the declaration is insufficient.
5. **`third_party_licences_include_tokio_epoll_uring` lands with Task 10**, whose `THIRD_PARTY_LICENSES.html` it checks (cargo-about needs `about.toml` to point at the repository-root texts, because they are not in the crate's directory).

### Task 9 rulings (2026-10-13, at `dev` `f8fc9f1b`)

Fork PR ostrium-labs/loams-postgres#7 (branch `loams/nf1-build-tools`, `bd5dcda5`, LP-0009) merged into `main` as `517503ee`; CodeRabbit was rate-limited after the retry, so it was self-reviewed.

1. **`loams-build-tools.yml`.** `build-tools/Dockerfile` is built for **bookworm only** (the storage image's Debian; Neon also built a bullseye variant, which nothing here uses), for amd64 and arm64, on `ubuntu-24.04` and `ubuntu-24.04-arm`.
   - On `main`, each architecture is pushed by digest with BuildKit's provenance (`mode=max`) and SBOM, and the two are joined into `ghcr.io/ostrium-labs/loams-postgres-build-tools:<sha12>`.
   - The cache is `loams-postgres-buildcache:build-tools-<arch>`, written from `main` only.
   - A pull request builds both architectures and pushes nothing.
   - Triggers: `build-tools/` changes, monthly (the 2nd, 03:23 UTC) and manual dispatch.
   - Task 12 then moved the signing and attestation to `loams-sign.yml`, as the plan said it would.
2. **The pin PR.**
   - `scripts/loams/pin-build-tools.sh` rewrites `ARG BUILD_TOOLS=` and nothing else. It refuses a tag, another registry or a short digest.
   - The job `pin` commits as `github-actions[bot]`, with a matching `Signed-off-by` and `Loams-Patch: LP-0009`, so the DCO and patch checks pass. It pushes to `bot/build-tools-<digest12>`, outside the `loams/*` no-deletion ruleset, so the branch can be deleted after merging.
   - The PR is opened with `LOAMS_BOT_TOKEN` (the App, Q733) when that secret exists, and with `GITHUB_TOKEN` otherwise.
   - The `GITHUB_TOKEN` path needs the owner's "Allow GitHub Actions to create and approve pull requests". The checks of a PR opened with `GITHUB_TOKEN` wait for a maintainer to approve them; the App token runs them at once.
3. **`Dockerfile`: `FROM $BUILD_TOOLS`** in the three build stages, replacing `$REPOSITORY/$IMAGE:$TAG` (`pinned`).
   - **Bootstrap:** until our first build, the default is the neon-archive mirror of Neon's last public build-tools (`ghcr.io/ostrium-labs/neon-archive/build-tools@sha256:bd1fe230…`). That image is byte-identical to Neon's `d4a4001a…-bookworm` and its audit is clean (Task 1).
   - It is a builder stage only: nothing from it ships in an image, so the "never deployed" ruling on the neon-archive packages (Task 2 ruling 2) is kept. The first run's pin PR replaces it.
   - This needs the owner to give `loams-postgres` read access to that private package, or to run `loams-build-tools.yml` before the first storage build.
4. **No `ARG BUILD_TOOLS` in the compute Dockerfile** (deviation). It has no build-tools base: it builds its own `build-deps` from Debian and installs its own toolchain.
5. **Tests.** `test_hygiene.sh build_tools_pinned_by_digest`:
   - the ARG is an `ostrium-labs` image by digest;
   - every `FROM` is `$BUILD_TOOLS` or the pinned Debian base;
   - `pin-build-tools.sh` changes exactly that line and refuses bad references.

   This is the plan's G0 grep check; it runs in `loams-hygiene.yml` until Task 16. "The workflow builds on PR" waits for Actions (owner). `actionlint` is clean.

### Task 10 rulings (2026-10-13, at `dev` `f8fc9f1b`)

Fork PR ostrium-labs/loams-postgres#8 (branch `loams/nf1-storage`, `468ad273`, LP-0010) merged into `main` as `bde73437`; CodeRabbit was rate-limited after the retry (one review an hour on the plan), so it was self-reviewed.

1. **`loams-build-storage.yml`** builds the `Dockerfile` for amd64 and arm64.
   - **Runners:** `vars.RUNNER_HEAVY` and `vars.RUNNER_HEAVY_ARM`, defaulting to the standard `ubuntu-24.04` and `ubuntu-24.04-arm` (Q721 is open).
   - **BuildKit's root on `/mnt`** when the runner has more than 40 GB free there. Docker's `data-root` moves; `daemon.json` is merged with `jq`.
   - **Build args:** `GIT_VERSION=<sha>` and `BUILD_TAG` (`<sha12>`, or the release passed by `workflow_call`).
   - **Cache:** `loams-postgres-buildcache:storage-<arch>`, written from `main` only.
   - **Push by digest.** `index` creates `:<sha12>` and `:edge` and attests the provenance.
   - **`smoke`:**
     - `labels_present`: the revision label equals the commit, on both platforms;
     - the licence files are present;
     - `smoke_select_1`.
   - **`audit`:** `storage_image_audit_clean`, `audit-image.sh` exits 0.
   - A pull request builds both architectures and pushes nothing.
2. **The smoke stack is `scripts/loams/smoke/` in the fork** (deviation; the plan only says "a `deploy/loams-postgres-dev`-shaped stack"). It is Loams' `deploy/loams-postgres-dev` without published ports: RustFS by index digest `8cc98017…`, broker, pageserver, one safekeeper and a compute. `run.sh` drives it:
   - a random compose project `loams-smoke-<hex>`;
   - a tenant and timeline created through the pageserver API (`curl` inside the pageserver container);
   - `SELECT 1`, and a check of the server's major version;
   - optional SQL files;
   - `pageserver --version` compared against `EXPECT_COMMIT`;
   - `down -v` of its own project only.

   The smoke spec preloads `pgaudit`, which Task 11's core smoke needs.
   **Run locally** with podman on Neon's 77e22e4b images: `SELECT 1` on 170005, `EXPECT_COMMIT` checked both ways, nothing left behind.
3. **The smoke's compute** is `vars.SMOKE_COMPUTE_IMAGE`. It defaults to the neon-archive mirror of Neon's compute-node-v17 (test only, never shipped; it needs package read access) until our compute-v17 exists.
4. **`audit-image.sh` and `audit_image.py` are vendored** from `scripts/nf1` (same contract). The skopeo image is pinned inline. The tests are `test_audit_image.py` (9 tests).
5. **`THIRD_PARTY_LICENSES.html`** comes from cargo-about 0.9.2 (sha256-checked per architecture), using `about.toml` and `about.hbs`.
   - **Accepted licences:** deny.toml's allow list plus its exceptions. MIT is first, so a dual-licensed crate is listed with its own copyright notice; Apache-2.0 first would have dropped `tokio-epoll-uring`'s MIT notice.
   - **`tokio-epoll-uring`:** a `clarify` entry points it at the repository-root texts, with checksums.
   - The image build fails if the notice is missing.
   - `third_party_licences_include_tokio_epoll_uring` (Task 8's test) is in `test_hygiene.sh`, and `loams-hygiene.yml` installs cargo-about.
6. **`.dockerignore` fixed a broken build.** `fuzz/` has been a workspace member since LP-0008 (Task 6), but `.dockerignore` left it out of the build context. Cargo could not load the workspace in the image build, so the storage and compute images could not be built from `main`. `test_docker_context.py` now runs `cargo metadata --locked` on exactly what `.dockerignore` lets through.
7. **Labels:** `org.opencontainers.image.{title,description,source,revision,version,vendor,licenses}`. **Files:** `/usr/share/doc/loams-postgres/{LICENSE,NOTICE,LOAMS_PATCHES.md,THIRD_PARTY_LICENSES.html}`.
8. **Not measured:** the build times for §51 §5.2 (Q721) need Actions. The workflow writes each architecture's job time to the run summary.
9. **Pre-existing, not changed:** the final stage downloads the AWS CLI from `awscli.amazonaws.com` without a checksum. That breaks "every downloaded source by sha256". Left for Task 20 or a follow-up.

### Task 11 rulings (2026-10-13, at `dev` `f8fc9f1b`)

Fork PR ostrium-labs/loams-postgres#9 (branch `loams/nf1-compute`, LP-0011) merged into `main` as `3178b3b4`; CodeRabbit: 1 comment (the smoke's checks printed false instead of failing), fixed in b827700d.

1. **The matrix comes from Loams' pin file.** `scripts/loams/compute-matrix.py` reads `supported ∪ maintenance ∪ beta` (today 16 and 17) and takes each major's minor from `vendor/revisions.json`.
   - Debian follows Neon: bullseye for 14–16, bookworm for 17 and newer, until Q727 (Task 28).
   - Tags follow the shared contract: `<pgminor>-<sha12>` and `edge`. A beta major gets `<pgminor>-beta-<sha12>` and `beta`.
   - Both amd64 and arm64 (Q729's default).
   - Tests: `test_compute_matrix.py` (7, in `loams-hygiene.yml`).
2. **Heavy stages** (`scripts/loams/compute-heavy-stages.txt`):
   - The list is `plv8-build`, `rdkit-build`, `postgis-build`, `pgrouting-build`, plus `pg_mooncake-build` beside `pg_duckdb-build`. pg_mooncake also vendors DuckDB; both leave the list when Task 21 drops them (Q719).
   - On `main` each heavy stage builds in its own job (`--target`, `outputs: type=cacheonly`).
   - Each writes its **own** cache ref, `compute-v<N>-<arch>-<stage>`. One shared ref would be overwritten by every job.
   - The image build reads all of them plus its own `compute-v<N>-<arch>`.
   - BuildKit runs one stage at a time (`max-parallelism = 1`, as Neon's CI did), because each stage runs `make -j$(nproc)`.
   - A pull request skips the heavy jobs and builds from `main`'s caches.
3. **`cargo auditable` in the compute image** (Task 1 ruling 3). `compute_ctl`, `fast_import` and `local_proxy` are built with `cargo auditable build` (cargo-auditable 0.7.0, as build-tools), so the audit reads their `.dep-v0` instead of the strings fallback.
   - `GIT_VERSION` reaches that stage.
   - The Rust crates' `THIRD_PARTY_LICENSES.html` is generated in the same step, because the crate sources live only in its cache mounts.
   - The image has OCI labels and `/usr/share/doc/loams-postgres-compute/{LICENSE,NOTICE,LOAMS_PATCHES.md,THIRD_PARTY_LICENSES.html}`.
4. **`core_extensions_create`.** `compute/tests/loams-smoke/core-extensions.sql` uses §51 §7.1's `core` tier until Task 20's manifest exists:
   - contrib;
   - `neon` and `neon_utils`;
   - `vector`, `postgis`, `pg_cron`, `pg_partman` (in schema `partman`), `pgaudit`, `pg_hint_plan`, `hypopg`, `pg_repack`, `pg_ivm` and `pg_session_jwt`;
   - `wal2json`, which is an output plugin, decoding one change through a logical slot.

   Every `CREATE EXTENSION` is `IF NOT EXISTS`, because `compute_ctl` creates `pg_stat_statements` and `neon` itself. A `DO` block then fails on any missing extension. The `pg_stat_statements`, pgvector and PostGIS queries follow.
   **Run locally** through `scripts/loams/smoke/run.sh` on Neon's 77e22e4b `compute-node-v17` and `-v16`: ok on 170005 and 160009. `pgaudit` needs `shared_preload_libraries`, so the smoke spec preloads it.
5. **`compute_image_audit_clean`:** `audit-image.sh` exits 0 on the index, and the job checks that `compute_ctl`, `local_proxy` and `fast_import` are among the audited binaries.
6. **Timings (`heavy_stage_jobs_under_limit`)** go to each job's summary. The cold and warm durations per major and architecture, and the Q721 budget check, need the first runs (Actions: owner).
7. **The smoke's storage image** is `vars.SMOKE_STORAGE_IMAGE`, defaulting to `loams-postgres-storage:edge` from Task 10.
8. **LP-0032 takes effect with these images.** Our `compute_ctl` creates spec roles with least privilege (Task 4 ruling 4). Loams switches to them in Task 14.

### Task 12 rulings (2026-10-13, at `dev` `4e9e715f`)

Fork PR ostrium-labs/loams-postgres#10 (branch `loams/nf1-sign`, LP-0012) merged into `main` as `233f02d1`; CodeRabbit was rate-limited after the retry, so it was self-reviewed. Here: `scripts/nf1/verify-pins.sh`, `verify_pins.py`, `tests/test_verify_pins.sh` and `nf1-pins.yml`.

1. **`loams-sign.yml`** (reusable; inputs `image`, `digest`, `sources`, `pg-version`, `debian`):
   - `cosign sign --yes` (keyless);
   - `actions/attest-build-provenance`, pushed to the registry;
   - an SPDX SBOM **per platform** (Syft through `anchore/sbom-action` with a `platform:` config; both are attested against the index digest with `actions/attest-sbom`);
   - with `sources: true`, the compute sources: `oras attach --artifact-type application/vnd.loams.sources.v1 … sources.tar:application/x-tar`, and the attached artifact is signed too.

   Callers: `loams-release.yml` (Task 13) and `loams-build-tools.yml`, whose inline attestation (Task 9) moved here. Edge builds keep their inline provenance attestation and stay unsigned.
2. **The sources before Task 20 (deviation).** The plan lists `sources.tar` "from the manifest", but the manifest (Task 20) does not exist yet. `scripts/loams/compute-sources.py` appends two generated stages to the compute Dockerfile instead.
   - **`loams-sources`** takes, from every `-src` stage (38 today), the downloaded tarballs and the patches applied (`<stage>--<file>`), plus Postgres' tree as `postgres-v<N>.tar.gz`. It adds `SHA256SUMS`, and `GIT-SOURCES` listing each `git clone` with its URL and tag (plv8's stage already packs its clone as `plv8.tar.gz`).
   - **`loams-sources-out`** outputs those files alone.

   Every GPL component (PostGIS, pgRouting, postgresql-unit; SFCGAL is LGPL) arrives as a tarball. A new `-src` stage is covered without an edit. Tests: `test_compute_sources.py`. Task 20 can switch the list to the manifest.
3. **`verify-pins.sh`** (a wrapper for `verify_pins.py`, stdlib).
   - For each pin with a fork source:
     - **`cosign verify`** with the shared identity: issuer `https://token.actions.githubusercontent.com`, and `^https://github.com/ostrium-labs/loams-postgres/\.github/workflows/loams-(release|sign)\.yml@refs/heads/main$`.
     - **`gh attestation verify --repo ostrium-labs/loams-postgres --source-digest <the pin's commit> --source-ref refs/heads/main`**: the provenance must come from the fork, for that commit (PR #537 review; the plan said `--owner ostrium-labs`, which any repository of the organisation satisfies).
     - **The SBOM attestations** (`--predicate-type https://spdx.dev/Document/v2.3`, the same scope), at least one distinct document per platform (two):
       - SPDX expressions are evaluated: `OR` needs one allowed side, `AND` needs all;
       - CDDL fails anywhere, Debian packages included;
       - outside `pkg:deb/` packages, only the plan's allowlist plus deny.toml's `0BSD`, `CC0-1.0` and `Unicode-3.0` pass;
       - Debian packages' licences are listed separately;
       - `NOASSERTION` is counted, not refused.
     - **For `compute-v<N>`:** `oras discover` must find the sources artifact, and `cosign verify` with the same identity must pass on it: attached is not enough.
   - Every tool call has a timeout (`TOOL_TIMEOUT`, default 300 s); a timeout fails the image.
   - `mirror:` pins print a `::notice::` and call no tool. Today's pin file passes this way.
4. **Tests:** `test_verify_pins.sh` uses stub `cosign`, `gh` and `oras` that answer from fixtures. The stub cosign applies the script's `--certificate-identity-regexp` itself, so the identity is what gets tested. Cases:
   - `unsigned_digest_fails_verify`;
   - `wrong_identity_fails_verify`: a `dev` branch, the old name `ostrium-labs/neon`, and `loams-build-storage.yml` all fail;
   - `sbom_has_no_forbidden_licence`: CDDL, SSPL, CDDL in a Debian package, and a missing SBOM all fail; `MIT OR SSPL-1.0` passes;
   - `provenance_bound_to_the_fork_commit`, `one_sbom_per_platform`;
   - `gpl_sources_attached` (missing, or attached but unsigned);
   - `mirror_pin_skipped_with_notice` (fixtures only; the job runs `verify-pins.sh` on the repository's pin file separately);
   - `tool_timeout_is_a_failure`;
   - `signed_pins_pass`.

   Two mutations (accepting any branch; ignoring CDDL) each made the suite fail.
5. **`nf1-pins.yml`.** The `checks` job installs cosign v3.0.6 (`sigstore/cosign-installer` v4.1.2) and oras (`setup-oras` v2.0.1), then runs `verify-pins.sh` and `test_verify_pins.sh`. These are steps in the existing job, not a separate `verify` job, to keep to one runner slot, as its comment asks.
6. **Not run against real images:** no fork image is signed until Actions are enabled (owner). The Fulcio `job_workflow_ref` claim the plan asks to confirm (that a reusable workflow's certificate names `loams-sign.yml`) is checked on the first release.

### Task 13 rulings (2026-10-13, at `dev` `88a3770a`)

Fork PR ostrium-labs/loams-postgres#11 (branch `loams/nf1-release`, LP-0013) merged into `main` as `de63e7e4`. CodeRabbit left 3 comments, all fixed: release documents are validated before any registry write, the month must be 01–12, and compute digests for unpinned majors are refused.

1. **`loams-release.yml`.** Triggers: `workflow_dispatch` (inputs `kind` = `full|storage|compute`, `version`) and `workflow_call` (for Task 18). It runs on `main` only. The steps:
   - `check-tag` refuses a malformed or existing version;
   - **the gate:** `loams-hygiene`, `loams-patches` and `loams-dco` as reusable workflows, until Task 16's `loams-gate.yml` adds G1–G3;
   - the builds: `loams-build-storage.yml` and `loams-build-compute.yml` with `build-tag` = the version;
   - `tag-images` resolves the digests without writing anything, refuses an image release tag that already exists, and writes the pin document and the notes. A refused document stops the release before any registry write. Only then does it tag the images `lp-YYYY.MM.N` (storage) and `<pgminor>-lp-YYYY.MM.N` (compute; a beta major gets `<pgminor>-beta-lp-…`);
   - `loams-sign.yml` per image, as a matrix; compute images also get their sources;
   - the annotated tag and the GitHub Release, with the notes and the pin document `loams-postgres-images.toml` as an asset, taken from `tag-images`' artifact;
   - the pin PR.

   The three check workflows gain `workflow_call` and put the caller's name in their concurrency group, so a push to `main` cannot cancel a release's gate.
2. **`kind`.** `storage` releases the storage image, `compute` every pinned major's compute image, and `full` both. A compute minor release is `full`, because the storage image carries the WAL-redo Postgres (§51 §5.6). A partial release keeps Loams' other pins, and `pin-doc` refuses one that would leave a major's storage and compute minors apart (`pins.py check` would fail on it).
3. **`release-notes.py`** (stdlib):
   - `check-tag` (month 01–12) and `previous` (the newest earlier `lp-` tag).
   - `pin-doc`: Loams' current pin file with the released images replaced, `source = "ostrium-labs/loams-postgres@<commit>"`, and minors from `vendor/revisions.json`; compute digests for a major Loams does not pin are refused. Documents generated for a `full` and a `storage` release passed Loams' `pins.py` `validate()` unchanged.
   - `notes`: the digests (built here, or "unchanged" and their source); the Postgres majors, minors and status; the extension versions, from `compute/loams-extensions.toml` once Task 20 writes it (until then the notes say so); the `LP-` rows added and dropped since the previous release, from `LOAMS_PATCHES.md` at both tags.

   Tests: `test_release.py`, 10 tests, including `release_tag_immutable`, `notes_list_digests_and_patches` and `pin_pr_matches_release`.
4. **The pin PR** uses `LOAMS_BOT_TOKEN` (the `loams-postgres-bot` App, Q733).
   - Steps: a shallow clone of Loams `dev`, `pins.py set --from-release <the release asset's https URL>`, `pins.py check`, then a commit as the bot with a matching `Signed-off-by`, branch `bot/pins-<version>`, and a PR into `dev`.
   - The body repeats the pin document, so the PR and the release can be compared digest by digest.
   - Without the token the job prints the `pins.py set` command to run by hand.
5. **Cadence.** Weekly on Tuesdays: in a month's first week, if no `lp-YYYY.MM.*` tag exists yet, the job opens one issue on `ostrium-labs/loams` (issues are off on the fork). It needs the bot token.
6. **`docs/loams/release.md`**: the cadence table, cutting a release, what it needs, and verifying with `verify-pins.sh`.
7. **Not run:** a real release needs Actions, package publishing and the bot token (owner). Task 14 cuts the first one.

### Task 14 rulings (2026-10-16, at `dev` `c8bcf4d8`)

Loams PR #679 (branch `nf1-b2-t14`). There is no fork change: the release was cut with Task 13's workflow.

1. **The parity release is `lp-2026.10.1`** (`78b24759`, the merge of fork PR #17).
   - `lp-2026.10.0`'s run failed in the compute sources step and created no git tag or Release. Fork PR #17 ("the sources check reads the whole tar listing") fixed that step, and `.1` is the plan's re-release (step 4).
   - The Postgres revisions equal the mirror's: `vendor/revisions.json` and both submodules are identical at `78b24759` and `77e22e4b` (17.5 `1e01fcea`, 16.9 `a42351fc`).
   - The three packages are public.
2. **The pin PR was opened by hand.** The release's `pin-pr` job has no `LOAMS_BOT_TOKEN` (Q733), so it printed the command. This PR runs `pins.py set --from-release https://github.com/ostrium-labs/loams-postgres/releases/download/lp-2026.10.1/loams-postgres-images.toml`, and the diff is the release asset's document.
3. **New consumer: the release stack's `endpoint-storage`** (`deploy/release/compose.yaml`, `image: ${NEON_IMAGE:-…}`).
   - It still defaulted to Neon's image and nothing checked it, so `pins.py` now generates and checks it (7 consumers).
   - The release stack's computes need no line: `loams-pg-control` compiles the pin file in.
   - `release/upgrade-path.toml`'s preview.3 `loams_postgres` component follows, as `check-upgrade-path.sh` requires. The PR carries `ci:upgrade`, so the upgrade gate runs a preview.2 stack upgraded onto these images.
4. **`test_pins.py` no longer reads the live pin file as its starting state.** Each scratch tree is rendered from `fixtures/pins/pins-mirror-77e22e4b.toml` (the Task 2 file), so the golden diffs stay put when the pins move. `test_the_repository_pins_our_own_release` checks:
   - an `lp-` release;
   - every repository under `ghcr.io/ostrium-labs/loams-postgres-`;
   - every source equal to `ostrium-labs/loams-postgres@<fork_commit>`;
   - no `ghcr.io/neondatabase/` in any consumer outside the README's Mirror history.

   That is "no `mirror:` source left".
5. **Step 3, as run:**
   - **`verify-pins.sh` on the real pins** (cosign v3.0.6, oras 1.3.1, locally and in a dispatched `nf1-pins.yml`): all three images verified. This confirms the Fulcio identity Task 12 left open: `loams-sign.yml`'s certificate passes the shared regex.
   - **`pg2-e2e.yml` dispatched on the branch**: every job green (Deploy, Pageserver ingest, pg-control deploy, Compose runtime with `it_endpoint`, Kubernetes runtime).
   - **The decoder's golden tests** (`pg2.yml` WAL decoder, headers from the new storage digest): green. The headers extracted from our image are identical to those from Neon's `neon` image, per file, for v16 and v17 (R31.6).
   - **The fixtures:** re-captured against our images and byte-equal except two timestamps (`crates/loams-postgres/tests/fixtures/README.md`).
   - **The desktop's stack copy:** the desktop ships `deploy/loams-postgres-dev` unchanged (`electron-builder.config.cjs`). It was run under a scratch compose project, not the Electron app: start, a tenant and timeline, `SELECT 1` (17.5), a branch, and `SELECT 1` on the branch's compute. Torn down with `down -v` of that project only.
6. **A dev breakage fixed on the way:** `crates/loams-wal-decoder/deny.toml` lacked the root's `zip@7.2.0` ignore (61cc37cb), so `check-decoder-deny.py` failed `pg2.yml`'s WAL decoder job. The entry is copied, with its removal date.
7. **Left:** `loams-pg-control`'s tests still name Neon's compute digest as an arbitrary pinned image (fakes, not consumers). Q715's default path is closed: the defaults are `ghcr.io/ostrium-labs`, and the neon-archive packages stay private. Until the bot token exists, every release's pin PR is opened by hand.

### Task 15 rulings (2026-10-16, at `dev` `c30e36c0`)

Fork PR ostrium-labs/loams-postgres#18 (branch `loams/nf1-postgres-branches`, LP-0014) merged into `main` as `b9b9becd`. CodeRabbit does not review the fork repositories, so it was self-reviewed.

1. **The branches were created through the REST API** (`POST repos/ostrium-labs/postgres/git/refs`), with no clone (Task 0 ruling).
   - `loams/REL_17_STABLE` = `efecf23b`, the tip of `loams/meta`: `REL_17_STABLE_neon` `56692dfb` plus Task 5's governance merge (`SECURITY.md`, `CODEOWNERS`). Task 5 ruling 5 already amended "unchanged" for 17.
   - `loams/REL_16_STABLE` = `2e2cf61f`: `REL_16_STABLE_neon` `59027122` plus one commit adding the same two blobs (`Loams-Patch: LP-0000`, signed off).
   - `loams/REL_18_STABLE` = `05e445f1`: `REL_18_STABLE_neon` `a616eefe` plus the same commit.

   So "unchanged" (the plan's interface) becomes "unchanged except the governance files". Every branch we maintain carries `SECURITY.md` and `CODEOWNERS`.
2. **Protection is Task 3's rulesets, unchanged.** `loams/**` has no force-push and no deletion, `REL_*_STABLE_neon` is frozen (update, non-fast-forward and deletion refused), and the tags are immutable. `gh api repos/ostrium-labs/postgres/rules/branches/loams/REL_17_STABLE` lists `non_fast_forward` and `deletion`.
3. **The tests are one script in the fork,** `scripts/loams/check-postgres-branches.py` (stdlib). It runs in `loams-hygiene.yml` on every PR and push to `main`, with 12 unit tests on a fake API in `test_postgres_branches.py`.
   - `branches_exist_and_protected`: each `loams/REL_<N>_STABLE` exists, descends from its `_neon` branch, carries both governance files, and refuses force-push and deletion. Each `_neon` branch refuses updates. It reads the public rules endpoint, so the hygiene job needs no extra token.
   - `submodule_url_relative`: every `.gitmodules` URL in the fork is `../postgres.git`.

   The live check failed before the branches existed (`missing` for all three) and passes now: `check-postgres-branches: ok (.gitmodules and ostrium-labs/postgres loams/REL_{16,17,18}_STABLE)`.
4. **The submodules still point at Neon's revisions** (17.5 `1e01fcea`, 16.9 `a42351fc`), with `branch = REL_<N>_STABLE_neon` in `.gitmodules`. Task 17 moves them to `loams/REL_<N>_STABLE` with the 17.11 and 16.15 merges. Postgres 18 has no submodule until Task 24.
5. **Q733 is answered (owner, 2026-10-16).** The owner set `LOAMS_BOT_TOKEN` as an organisation secret visible to every repository. From the next release on, `loams-release.yml`'s `pin-pr` job opens the pin PR itself, and the cadence check can open its issue. Task 18's pushes to `ostrium-labs/postgres` use the same secret. The rest of Q733 is now moot: the bot is a token, so nobody needs to refresh the `gh` token's scopes for it. The token's scopes were not inspected, so the first real use is its check.
6. **Clone approval (Task 0).** The owner approved cloning `ostrium-labs/postgres` for Task 17 (2026-10-16). The clone must be blobless or treeless, outside `/tmp` and `/mnt/Projects`, and deleted afterwards. A CI job that does the upstream merge is preferred, because it can be repeated for every later minor.

### Task 16 rulings (2026-10-16, at `dev` `f329a139`)

Fork PRs ostrium-labs/loams-postgres#19 (`6212f59e`, the gate), #21 (a mode bit), #22 (concurrency, protobuf) and #23 (clippy `--no-deps`), plus the shellcheck fix carried in #20; all are LP-0015. Loams PR #698 (`nf1-candidate.yml`). CodeRabbit skips the fork repositories and was rate-limited on #698, so all of these were self-reviewed.

1. **When the gate runs (owner, 2026-10-16; this answers Q721):**
   - on GitHub-hosted standard runners;
   - **nightly** at 21:07 UTC, away from Loams' nightly at 02:17;
   - **on every release:** `loams-release.yml` calls G0–G2 before it builds, and G3 before it tags;
   - **by hand** (`workflow_dispatch`, any set of tiers).

   It does **not** run on pull requests.
   - **Concurrency.** The organisation's concurrent-job limit is shared with Loams' CI. So there is one gate per caller and ref: a newer nightly or dispatch cancels the older one, and a release's calls queue and are never cancelled. At most two heavy jobs run at once: G1 beside G2's build, then the G2 majors with `max-parallel: 2`.
   - **No image builds from these PRs.** A fork PR that touches Rust code also starts the storage and compute image builds. Those were cancelled on these PRs, because the only Rust change was rustfmt and G1/G2 compile it.
2. **Required checks (deviation).** The plan's `required_checks_configured` lists `g0`, `g1`, `dco` and `patches`. A tier that does not run on pull requests cannot be a required check, so `main`'s new ruleset `main: required checks (hygiene, patches, dco)` requires the pull-request tier instead. It has the same admin bypass as the PR rule. `gate.py required-checks` verifies the ruleset live in `loams-hygiene`. G0–G2 gate releases instead.
3. **G0** runs rustfmt, clippy `-D warnings` on G1's crates, `cargo deny check bans licenses sources`, actionlint (with shellcheck), and the `hygiene`, `patches` and `dco` workflows.
   - **Advisories are not in G0 (deviation).** `cargo deny check advisories` fails at today's `main`. `loams-advisories.yml` already gives each advisory a tracked issue with a due date (D805), and putting advisories in G0 would stop every release at once.
   - **Fixes the first runs needed:**
     - `libs/remote_storage/src/gcs_bucket.rs` was the workspace's one unformatted file; it is now formatted (formatting only).
     - A shellcheck finding in `loams-build-compute.yml` (SC2153) was fixed.
     - `libprotobuf-dev` is installed, because `storage_broker`'s build script needs `google/protobuf/empty.proto` for clippy.
     - Clippy runs with `--no-deps`: it lints G1's crates themselves, not every workspace crate they depend on. `remote_storage`'s GCS backend has 41 findings at `main`; cleaning it up is a follow-up.
4. **G1 runs once, not once per major (deviation).** The Rust tests do not take the major from the environment: `postgres_ffi` is generic over every major it has bindings for, and the pageserver's WAL-redo tests use `DEFAULT_PG_VERSION` (17). So G1 builds Postgres and Neon's extensions for every major (`make neon-pg-ext`), then runs:
   - `cargo nextest` on `postgres_ffi`, `wal_decoder`, `pageserver_api`, `compute_api` and `compute_tools`;
   - the pageserver's tests twice, with `NEON_PAGESERVER_UNIT_TEST_VIRTUAL_FILE_IOENGINE=std-fs` and with `tokio-epoll-uring`.

   The per-major tier is G2.
   - It runs in the pinned `loams-postgres-build-tools` image (`scripts/loams/gate-docker.sh`) as uid 1000, because Postgres refuses root, with seccomp unconfined so io_uring is available.
   - **The first run passed:** 105, 283 and 283 tests, in 20 minutes.
5. **G2** is one release build (`make all`, feature `testing`, 38 minutes), shared as an artifact, then `test_runner` per pinned major (16, 17) on `scripts/loams/gate-subset.txt`. The subset has 9 functions, 18 tests with their parameters:
   - `test_pg_regress`;
   - `test_branch_behind` and `test_branching_with_pgbench`;
   - `test_compute_reconfigure`;
   - `test_pgdata_import_smoke`;
   - `test_lfc_resize` and `test_local_file_cache_unlink`;
   - `test_wal_acceptor.py::test_restarts` and `::test_restart_endpoint`.

   `gate.py subset-check` (in `loams-hygiene`) checks that each name exists. A flake is skipped only by a `deselect <node> <issue URL>` line.
   - **The first run passed:** 18 tests on pg16 and 18 on pg17, about 15 minutes each.
6. **`gate_fails_on_red_tier`:**
   - **Unit tests:** `gate.py verdict` fails the job `gate` when a job of a requested tier failed, was cancelled or was skipped.
   - **Live:** the first run (https://github.com/ostrium-labs/loams-postgres/actions/runs/38935843561) had G1 and G2 green and G0's lint red, and the job `gate` failed.
7. **G3 (deviation: a pin document, not per-image digests).** The fork sends the candidate's whole pin document (`release-notes.py pin-doc`), because a minor release changes `pg_minor` too, not only digests.
   - **In Loams:** `scripts/nf1/candidate.py` accepts an image only when it is built from the candidate's commit (in a `ghcr.io/ostrium-labs/loams-postgres-*` repository), or when it is exactly what Loams pins today. `pg2-e2e.yml` and `pg2.yml`'s WAL decoder job became callable (`candidate-pins`). `candidate-smoke.sh` checks the version, an extension smoke and a branch; it passed locally on 17.5. The status goes back as `loams-gate / g3`.
   - **In the release:** `tag-images` is split. `candidate` resolves the digests and writes the documents; G3 then runs on that document; only then does `tag-images` tag the images.
8. **`g3_reports_status` is blocked on the owner.** The dry run with today's pins (https://github.com/ostrium-labs/loams-postgres/actions/runs/38941121083) failed at the dispatch: `HTTP 403: Resource not accessible by personal access token`. `LOAMS_BOT_TOKEN` is a fine-grained token, and it lacks **Actions: read and write on `ostrium-labs/loams`**. The report also needs **Commit statuses: read and write on `ostrium-labs/loams-postgres`**.

   Until the token has both, G3 fails, so **no release can be tagged**. That is deliberate: a release without Loams' integration is not a release.

### Task 17 rulings (2026-10-16 and 2026-10-17)

Fork PR ostrium-labs/loams-postgres#20 (LP-0016) merged.

1. **The merge runs in CI, not on a workstation** (owner, 2026-10-16: prefer a repeatable CI job). `loams-postgres-merge.yml` runs `scripts/loams/postgres-merge.sh`:
   - a blobless clone of `ostrium-labs/postgres` inside the job;
   - the upstream tag from `postgres/postgres`;
   - `git merge --no-ff` with `Loams-Sync: <tag>`;
   - `push` writes `loams/minor-<N>-<tag>`; `promote` fast-forwards `loams/REL_<N>_STABLE`;
   - a conflict uploads the files and the diff.

   `test_postgres_merge.sh` covers it with 5 cases. Nothing was cloned locally.
2. **Starting points.** `loams/REL_17_STABLE` is at 17.8 and `loams/REL_16_STABLE` at 16.12, from Neon's branches.
3. **Dry runs (no push):**
   - **`REL_16_15`** (https://github.com/ostrium-labs/loams-postgres/actions/runs/38937088537): 5 conflicted files.
     - `hash_xlog.c`: Neon's `am_wal_redo_postgres` guard against upstream's new `XLogFlushBufferForRedoIfInit` helper.
     - `walsender.c`.
     - `pgstat.c`: includes.
     - `encoding.out` and `encoding.sql`: tests added on both sides.
   - **`REL_17_11`** (https://github.com/ostrium-labs/loams-postgres/actions/runs/38937081682): 10 conflicted files. They are the same kinds as in 16, plus `read_stream.c`, `postgres.c`, `injection_point.c`, the injection-points test module, and **`heapam.c`**.
4. **A maintainer must decide 17.11's `heapam.c` before the 17 merge.** Upstream 17.11 includes "Fix VM clear WAL logging by registering VM blocks": heap insert, delete, update and lock records now register the visibility-map page as an extra block reference. Neon writes those records as its own `RM_NEON_ID` records (`xl_neon_heap_*`). The pageserver ingests them (`wal_decoder`, `walingest.rs`), and `neon_rmgr` replays them, and the pageserver clears visibility-map bits from the record's flags itself. There are two ways forward:
   - **(a)** Keep Neon's records without the new block reference, and confirm that the pageserver's own VM handling already covers the case the fix is for.
   - **(b)** Add the VM block reference to the `RM_NEON_ID` records, and change `neon_rmgr`, `wal_decoder`, the pageserver's ingest and Loams' `loams-wal-decoder` fixtures to match.

   Either way it is a WAL-format question (Review Focus 7), so it is not settled inside a merge commit. 16.15 does not have this change, so its five conflicts are mechanical.
5. **Still to do for `lp-2026.11.0` (deadline 2026-11-19):**
   - resolve both merges on `loams/minor-<N>-<tag>`, recording each resolution in its merge commit;
   - move `vendor/postgres-v16` and `vendor/postgres-v17`, `.gitmodules` (`branch = loams/REL_<N>_STABLE`) and `vendor/revisions.json`;
   - run G1 and G2 on that branch, and G3 once the token is fixed;
   - release.

   17.12 and 16.16 follow on 2026-11-12, through the same workflow.
6. **17.11's `heapam.c`: option (a) (owner, 2026-10-17).** The fork keeps its heap WAL record format and does not take upstream `c0d9864f`'s VM block references.
   - **The premise holds.** The pageserver derives every VM clear from the records' `*_ALL_VISIBLE_CLEARED` and `XLH_LOCK_ALL_FROZEN_CLEARED` flags and stores it on the VM page itself. Computes WAL-log every VM page they evict, replicas clear the bits from the same flags, and nothing in Neon's storage model reads a WAL summary or a missing VM full-page image. The citations are in the fork's `docs/loams/pg17-vm-clear.md` (LP-0016).
   - **The resolution.** `heapam.c` is a three-way merge of ours with REL_17_11's `heapam.c` minus `c0d9864f`. That also drops the two additions that would otherwise have merged into the fork's records without a conflict. Upstream's tests of the registration (`pg_walsummary` `t/002_blocks.pl`, `pg_combinebackup` `t/012_vm_consistency.pl`) are reverted with it.
   - **The test.** `test_loams_vm_clear.py` runs the bug's scenario through the pageserver, the primary and a replica. It also guards the format: no heap record registers a VM block. It passes at 16.15 and at 17.11.
7. **The recut (owner, 2026-10-17, option 1).**
   - **Why.** Task 15's `loams/REL_<N>_STABLE` started at Neon's `REL_<N>_STABLE_neon` heads, 387 (16) and 475 (17) commits past the revisions `main` pins. That range holds Neon's own Postgres-side changes ("Remove unnecessary set_lwlsn_block_* hooks", "Move LastWrittenLsnLock to neon extension", `smgr_read_slru_segment` gone). Those pair with `pgxn/neon` changes Neon never published, so our `pgxn/neon` did not build at 16.15 (gate run https://github.com/ostrium-labs/loams-postgres/actions/runs/38991579525).
   - **The new branches.** `loams/pg16` starts at `a42351fc` (16.9) and `loams/pg17` at `1e01fcea` (17.5), the revisions `main` pinned. Both were created through the API, each has the governance commit, and the existing `loams/**` rulesets protect them.
   - **Retired, never deleted.** `loams/REL_16_STABLE`, `loams/REL_17_STABLE` and `loams/REL_18_STABLE` (`REL_18_STABLE_neon` has the same changes) are kept protected and marked superseded in the fork's `docs/loams/postgres-branches.md`. Task 24 chooses the base for 18.
   - **The check.** `check-postgres-branches.py` now checks the new layout: each branch descends from its pinned base, the retired branches still exist, and the submodules track `loams/pg<N>`.
   - **The merge job** uses `loams/pg<N>` and `loams/minor-pg<N>-<tag>`. On `promote` it reuses an existing minor branch (a hand resolution), so it never re-merges.
8. **The merges (resolved by hand in an approved blobless clone, now deleted).**
   - **16.15:** `loams/minor-pg16-REL_16_15` `23f0d68a`. Conflicts in `hash_xlog.c` (upstream's `XLogFlushBufferForRedoIfInit()`, with Neon's `!am_wal_redo_postgres` moved into the helper), `walsender.c`, `pgstat.c`, `pg_restore.c` and `002_pg_dump.pl`.
   - **17.11:** `loams/minor-pg17-REL_17_11` `aef0fc52`. Conflicts in `heapam.c` (as in ruling 6), `hash_xlog.c`, `walsender.c`, `read_stream.c`, `postgres.c` and `pgstat.c`.
   - **One fix with no textual conflict, in both merges:** upstream's new `_bt_get_endpoint()` call needs Neon's extra `parent` argument.
   - Each resolution is recorded in its merge commit. Both minor branches were promoted: `loams/pg16` = `23f0d68a` and `loams/pg17` = `aef0fc52`, through `loams-postgres-merge.yml` with `promote`.
9. **A storage fix the minors require.** Upstream `8ba61bc0` ("Set next multixid's offset when creating a new multixid", in 16.15 and 17.11) makes readers rely on the next multixid's offset being set. The pageserver did not set it: `test_pg_regress`'s basebackup comparison differed in `pg_multixact/offsets/0000` at both majors. `ingest_multixact_create` now follows upstream (fork `88b997ad`, LP-0016):
   - a new `NeonWalRecord::MultixactOffsetCreatePair` when the next multixid is on the same page. It is added before the testing-only variant, so no serialized index moves.
   - an offset record on the next page when that page exists;
   - otherwise (WAL from an older minor) the next page is initialized with the offset, and its later zero-page record is skipped, as upstream's `pre_initialized_offsets_page`.

   `test_multixact` joined G2.
10. **Gated at both majors:** https://github.com/ostrium-labs/loams-postgres/actions/runs/38999301976. Every tier passed: G0, G1, and G2 at 21 tests per major (pg16, pg17). Fork PR ostrium-labs/loams-postgres#24 merged as `e69172aa`.
11. **G3 and Q733.** With the owner's fix to `LOAMS_BOT_TOKEN` (it lacked access to `loams-postgres`), G3 runs end to end and posts `loams-gate / g3`. The first full run failed only in `decoder / WAL decoder`, and that was not the candidate: `dev`'s `crates/loams-wal-decoder/deny.toml` lacked the root's graspologic `allow-git` entry (`check-decoder-deny.py`), and Loams #707 fixed it. **Q733 is fully resolved.**
12. **Next: `lp-2026.11.0`** (16.15 and 17.11; due 2026-11-19). The release builds the images and runs G0–G3 before tagging; 17.12 and 16.16 follow on 2026-11-12 through the same merge job.
