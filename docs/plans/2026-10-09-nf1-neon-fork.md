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
- The decoder: `crates/loams-wal-decoder/Cargo.toml` (`git = "https://github.com/ostrium-labs/loams-postgres"` for `pageserver_api`, `postgres_ffi`, `utils` and `wal_decoder`, **the same `rev` `1218fb7a6a37e1b4c268bad5c2952c238d81d368`**; the section `[patch."https://github.com/ostrium-labs/loams-postgres"]`), its `Cargo.lock` (regenerated: Cargo treats a new URL as a new source; `cargo update -p pageserver_api -p postgres_ffi -p utils -p wal_decoder` in CI, or locally once the owner allows the fetch), and `deny.toml`'s `allow-git`.
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
