# NF1 — Ostrium Labs Owns the Neon Fork Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact names, paths, branches, tags or defaults, use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Planned** (2026-10-09). Track NF1, design [§51](../design/51-neon-fork.md) (D800–D819, Q715–Q729), an addendum to [§28](../design/28-loams-postgres.md) §10 and [§46](../design/46-loams-postgres-production.md). **Owner decision, 2026-10-09: "Mirror and fully fork neon postgres all repos we will maintain it add extension new pg 19 support etc."** NF1 supersedes D241's cadence and answers Q649. It amends [PG2](2026-10-08-pg2-postgres-production.md) Task 0 ruling 7, Task 2 ruling R2.1, Task 51 and Task 55 (§51 §12.5). **Postgres 19 is beta in every Loams artifact and document until Task 35's gate passes**; upstream's GA is 2026-10-29.

**Goal:** Ostrium Labs runs the Neon fork as a maintained product:
- every fork inventoried, licensed, protected and patch-tracked;
- Neon's last public images mirrored with unchanged digests;
- our own signed, attested, multi-arch images built from `ostrium-labs/neon` and pinned by Loams by digest;
- Postgres minors within 7 days, behind a test gate;
- a licence-checked extension catalogue with a Loams extension;
- Postgres 18 supported, and Postgres 19 in beta then supported;
- a major-upgrade path for Neon branches;
- private dependencies and CDDL removed, and `tokio-epoll-uring`'s licence resolved;
- a costed staffing model with a minimum viable maintenance mode.

The exit is the checklist at the end of this plan.

**Architecture** (§51 §3, §5, §12):
- **Three kinds of repositories:**
  - `ostrium-labs/neon` holds the code we build and **all CI** (`.github/workflows/loams-*.yml` on `loams/main`);
  - `ostrium-labs/postgres` holds branches only (`loams/REL_<N>_STABLE`);
  - `ostrium-labs/loams` (this repository) holds the pin file, the consumers, the verification and the integration tests.
- **Flow:** upstream → `main` (fast-forward mirror) → merge PR into `loams/main` behind the gate → release `nf-YYYY.MM.N` → images by digest, signed and attested → a bot PR here updates `deploy/pins/neon-images.toml` → every consumer is regenerated from it and `cosign verify` runs.

**Tech stack:**
- GitHub Actions; Docker Buildx with BuildKit (registry cache, `provenance: mode=max`, `sbom: true`); `skopeo` (run through podman, pinned by digest); `cosign` (keyless, Sigstore); `oras`; `actions/attest-build-provenance` and `actions/attest-sbom`; Syft; Trivy; `rust-audit-info` (cargo-auditable); `cargo deny`; `cargo about`; `askalono`; `actionlint`; Python 3 (stdlib only) for scripts; `gh`.
- The fork's toolchain is its own (`rust-toolchain.toml`, 1.88.0 at `fa504217`); Loams' crates stay on 1.97.
- Postgres 16, 17, 18 and 19 trees from `ostrium-labs/postgres`.

**Spec:**
- [§51](../design/51-neon-fork.md) (all), and D800–D819, Q715–Q729 in the [decision log](../design/13-decision-log.md).
- [§28](../design/28-loams-postgres.md) §10 (the fork's history and estimates).
- [§46](../design/46-loams-postgres-production.md) §8.3 (the PgDog licence boundary), §9 (`loams-wal`), §12 (upgrades), §16.1 (extensions), §18 (the desktop contract).
- [PG2](2026-10-08-pg2-postgres-production.md): Task 2 and its rulings R2.1–R2.10 (branch `backend/pg2`), Task 31 rulings (R31.3, R31.6, R31.12), Tasks 51, 55 and 58.

## Global Constraints

- **Repositories, worktrees and branches.**
  - Loams: work in `~/Documents/Ostriumlabs/loams-wt/nf1-<milestone>`, one branch per milestone (`feat/nf1a-governance`, `feat/nf1b-builds`, `feat/nf1c-sync`, `feat/nf1d-extensions`, `feat/nf1e-pg18`, `feat/nf1f-pg19`, `feat/nf1g-integration`), each from `dev`, with stacked PRs into `dev`.
  - The `neon` fork: worktrees of `~/Documents/Ostriumlabs/neon` under `~/Documents/Ostriumlabs/neon-wt/<topic>`, on `loams/<topic>` branches, with PRs into `loams/main`.
  - The `postgres` fork: **ask the owner before cloning** it (Task 0). Once approved, clone blobless (`--filter=blob:none`) to `~/Documents/Ostriumlabs/postgres`, with worktrees under `~/Documents/Ostriumlabs/postgres-wt/<topic>`.
  - Never use `git stash`.
- **Commits.** `git commit -s` (DCO) everywhere. Every commit in a fork that is not upstream's carries a `Loams-Patch: NF-<nnnn>` trailer (§51 §3.4). Commit areas: here `neon`, `deploy`, `ci`, `wal`, `pg`, `desktop`, `docs`; in the fork `loams-build`, `loams-ci`, `loams-licence`, `loams-pg18`, `loams-pg19`, `loams-ext`, and the upstream areas (`pageserver`, `compute`, …) for code.
- **Never rewrite fork history.** No force-push or rebase on `loams/*` or `main`, and no deletion of a `loams-*` or `nf-*` tag. Upstream comes in by merge (D802).
- **Rust builds.** Loams crates use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR` and never build in `/tmp`. Run one cargo build at a time. Build touched crates only (`cargo test -p loams-wal-decoder`). **The Neon workspace is not built locally in full:** local work is `cargo check -p <crate>` on the crates touched, with headers from `scripts/pg2/pg-headers.sh`. Images, `neon_local` and `test_runner` run in the fork's CI.
- **No container builds on the build machine** while a cargo build runs. Image builds happen in CI. The mirror (Task 1) is a registry-to-registry copy, with no local build.
- **Pins.**
  - Every image is referenced by digest; every git dependency by `rev` with a `loams-*` tag on it; every downloaded source by sha256.
  - A new dependency, action or extension version must be at least 14 days old, except security fixes. Record each pin in the commit message.
  - Actions are pinned by commit SHA.
- **Licences.** No CDDL in any binary or image we publish (owner ruling, 2026-10-08). No AGPL, SSPL, BUSL, ELv2, TSL, Commons Clause or unlicensed code in an image. GPL components only with an attached source artifact (§51 §3.6). PgDog stays unmodified and outside every image (§46 §8.3).
- **No `neondatabase/*` fetch at build time** after Task 7: no git dependency, Dockerfile download, action or registry from that organisation. `scripts/loams/check-no-private-sources.sh` enforces it.
- **Owner actions are named, not assumed.** Creating teams, the GitHub App, package visibility changes, billing and counsel are listed in the tasks that need them. A task that needs one records "waiting on owner" in its rulings and continues with what does not depend on it.
- **Postgres 19 is labelled beta** in every image tag (`-beta`), pin entry (`beta = [19]`), compose profile (`beta`) and document until Task 35.

## Review Focus

1. **A mirrored digest differs from its source**, or a mirror is published before its audit. Expected: never. Tests: Task 1 `mirror_digests_match`, `audit_runs_before_visibility`.
2. **A published image contains CDDL code or code from a private repository.** Expected: never. Tests: Task 7 `deny_rejects_cddl`, `no_private_sources`; Task 10 `neon_image_audit_clean`; Task 11 `compute_image_audit_clean`; Task 12 `sbom_has_no_forbidden_licence`.
3. **Loams pins an image that is unsigned, unattested, or signed by another identity.** Expected: CI fails. Tests: Task 12 `unsigned_digest_fails_verify`, `wrong_identity_fails_verify`; Task 2 `pins_match_consumers`.
4. **A pinned fork revision becomes unreachable.** Expected: CI fails before merge. Tests: Task 3 `pinned_revs_reachable`, `force_push_rejected` (branch protection probe).
5. **A patch enters `loams/main` untracked.** Expected: the gate fails. Tests: Task 4 `unlisted_patch_fails`, `trailer_without_row_fails`.
6. **A Postgres security minor misses the 7-day target unnoticed.** Expected: an issue with a due date opens on release day and escalates. Tests: Task 6 `pg_security_release_opens_issue`, `overdue_issue_escalates`; Task 18 `new_minor_tag_starts_release`.
7. **A WAL record of a new major is decoded wrongly or skipped silently.** Expected: an unknown record or magic is an error; every record type has a fixture. Tests: Task 25 `unknown_page_magic_is_error`; Task 26 `decode_all_rmgrs_pg18`; Task 29 `golden_pg18_matches_fork_sender`; Task 33 `decode_all_rmgrs_pg19`.
8. **A compute of one major attaches to a timeline of another.** Expected: refused. Tests: Task 37 `compute_major_mismatch_refused`; Task 39 `local_stack_starts_timeline_major_only`.
9. **A major upgrade loses or corrupts data, or runs without the user asking.** Expected: never. Tests: Task 37 `upgrade_17_to_18_preserves_checksums`, `upgrade_failure_keeps_source_serving`; Task 39 `upgrade_never_automatic`.
10. **An extension with a forbidden licence, or a source with the wrong hash, enters the image.** Expected: the build fails. Tests: Task 20 `forbidden_licence_rejected`, `manifest_matches_dockerfile`, `sha_mismatch_fails`.
11. **The sync pushes something other than a fast-forward to `main`, or brings upstream's workflows back.** Expected: refused or re-stripped. Tests: Task 19 `sync_refuses_non_ff`, `sync_restrips_upstream_ci`.

---

## File structure

```
ostrium-labs/neon (branch loams/main)
  LOAMS.md                                   Task 3   charter, independence statement, release map
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
  docs/loams/{sync.md,release.md,extensions.md,pg18-format-diff.md,pg19-format-diff.md,LICENSING-tokio-epoll-uring.md}
  scripts/loams/{check-patches.py,check-no-private-sources.sh,strip-upstream-ci.sh,check-extensions.py,
                 sync-upstream.sh,postgres-minors.py,release-notes.py,audit-image.sh,sla.py}
  .github/workflows/loams-{patches,gate,build-tools,build-neon,build-compute,release,sync-upstream,
                           postgres-minors,extensions-update,advisories,rebuild-diff}.yml
  .github/workflows/<upstream>.yml           Task 7   deleted (NF-0002)
ostrium-labs/postgres                        branches loams/REL_16_STABLE … loams/REL_19_STABLE (Tasks 15, 17, 24, 31, 32)
ostrium-labs/tokio-epoll-uring (loams/main)  LICENSE-MIT, LICENSE-APACHE, uring-common/LICENSE-tokio-uring, LICENSING.md   Task 8
ostrium-labs/pg_session_jwt, rust-postgres, azure-sdk-for-rust, framed-websockets, autoscaling   Task 3 (branches, protection, archive)

ostrium-labs/loams (this repository)
  deploy/pins/neon-images.toml               Tasks 1, 2, 14 (and every release after)
  deploy/pins/audit/<digest>.json            Tasks 1, 10, 11 (cargo-auditable package lists)
  scripts/nf1/{mirror-images.sh,pins.py,check-pinned-revs.sh,verify-pins.sh}
  deploy/neon/{compose.yaml,README.md}       Tasks 2, 14, 39
  deploy/loams-pg-bench/compose.yaml         Tasks 2, 14
  .github/workflows/{pg2.yml,pg2-e2e.yml,loams-pg-bench.yml}   Task 2 (PG_HEADERS_IMAGE from the pin file)
  .github/workflows/nf1-pins.yml             Tasks 2, 12
  .github/workflows/nf1-candidate.yml        Task 16 (the gate's G3, dispatched by the fork)
  crates/loams-wal-decoder/{Cargo.toml,Cargo.lock,tests/fixtures/}   Tasks 3, 29, 34
  crates/loams-safekeeper/                   Task 29 (version handling, if any change is needed)
  crates/loams-neon/src/{majors.rs,pageserver.rs}   Tasks 29, 37
  scripts/pg2/pg-headers.sh                  Task 29 (v18, v19)
  conformance/router/pgdog-pg{17,18,19}.tsv  Task 36
  apps/desktop-electron/src/main/stacks/stacks.ts, …/sql/backend/local-stack.ts   Task 39 (after PG2 Task 58)
  docs/runbooks/neon-fork/{release.md,minor-release.md,security.md,sync.md,new-major.md,mvm.md}   Task 40
  docs/design/51-neon-fork.md, 13-decision-log.md, README.md; docs/plans/README.md   Tasks 0, 41
```

## Shared contracts (all tasks use these names)

### The pin file: `deploy/pins/neon-images.toml`

```toml
# Written by scripts/nf1/pins.py (set); checked by pins.py (check). Do not edit by hand.
schema = 1
fork_release = "nf-2026.10.0"            # or "mirror-77e22e4b" while only mirrored images are pinned
fork_commit = "<40-hex>"                 # ostrium-labs/neon loams/main commit (or the upstream commit for a mirror)

[majors]
supported = [17]                          # Postgres majors new projects may use
beta = []                                 # need allow_beta_major; 19 goes here first
maintenance = [16]                        # minors and exports only, no new projects
default = 17                              # for new projects and new desktop tenants

[images.neon]
repository = "ghcr.io/ostrium-labs/neon"
digest = "sha256:<64-hex>"                # multi-arch index
source = "ostrium-labs/neon@<40-hex>"     # or "mirror:ghcr.io/neondatabase/neon@sha256:<64-hex>"
pg_minors = { "16" = "16.9", "17" = "17.5" }

[images.compute-node-v17]
repository = "ghcr.io/ostrium-labs/compute-node-v17"
digest = "sha256:<64-hex>"
source = "ostrium-labs/neon@<40-hex>"
pg_minor = "17.5"
```

`pins.py check` fails unless:
- every consumer's default equals the file (the `x-neon-image` and `x-compute-image` anchors of both compose files, `PG_HEADERS_IMAGE` in three workflows, and later the single-node compose and the Helm values);
- every `digest` is 64 hex characters;
- a `source` is either a fork commit or a `mirror:` reference.

`pins.py set --from-release <url>` rewrites the file and all consumers. `pins.py rust` writes `crates/loams-neon/src/majors.rs` (`SUPPORTED`, `BETA`, `MAINTENANCE`, `DEFAULT`).

### Fork refs

| Ref | Rule |
|---|---|
| `main` (neon) | Fast-forward to `neondatabase/neon` `main` only, by `GITHUB_TOKEN` |
| `loams/main` (neon) | Default branch; PRs only; required checks `loams-gate / g0`, `g1`, `g2` (when triggered), `dco`, `patches` |
| `loams/<topic>` | Work branches |
| `loams/REL_<N>_STABLE` (postgres) | Ours, per major; `REL_<N>_STABLE_neon` mirrors Neon's branch |
| Tags `loams-<YYYYMMDD>-<n>` | Any revision pinned outside a release |
| Tags `nf-YYYY.MM.N` | Fork releases; immutable |

### Patch trailer and inventory

- The trailer is `Loams-Patch: NF-0007`. Upstream merges carry `Loams-Sync: <upstream sha>`.
- `LOAMS_PATCHES.md` rows: `| NF-0007 | Licence texts for tokio-epoll-uring | licence | @owner | 2026-10-20 | loams-only | Neon adds LICENSE files upstream |` (columns: id, title, area, owner, since, upstream, drop when).

### Image tags

- `neon`: `<sha12>`, `nf-YYYY.MM.N`, `edge`.
- `compute-node-v<N>`: `<pgminor>-<sha12>`, `<pgminor>-nf-YYYY.MM.N`, `edge`; a beta major adds `-beta` to each, plus `beta`.
- `build-tools`: `<sha12>`.
- Mirror: `mirror-77e22e4b`.
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
--certificate-identity-regexp '^https://github.com/ostrium-labs/neon/\.github/workflows/loams-(release|sign)\.yml@refs/heads/loams/main$'
```

Signing runs in the reusable workflow `loams-sign.yml`, called by `loams-release.yml`. Fulcio puts the called workflow's `job_workflow_ref` in the certificate (verify in Task 12), which is why the regex accepts both names. Only `loams-release.yml` and `loams-build-tools.yml` call it; `edge` builds are unsigned and never pinned.

---

## Execution order

1. Task 0.
2. **NF1a** (Tasks 1–6). Task 1 first: the mirror protects against Neon deleting its packages.
3. **NF1b** (Tasks 7–14), after Task 4. Task 14 (the parity release and the switch to our images) is the milestone's exit.
4. **NF1c** (Tasks 15–19) after Task 13. **Task 17 (the catch-up to 17.11) is urgent**: it must ship by 2026-11-19 at the latest, with 17.12 (released 2026-11-12) merged.
5. **NF1d** (Tasks 20–23) after Task 11. Task 21 answers Q651 for PG2 Task 55.
6. **NF1e** (Tasks 24–30) after Tasks 16 and 17.
7. **NF1f**: Task 31 starts on 2026-10-15 (Postgres 19 RC1), beside everything else. Tasks 32–35 follow Task 27.
8. **NF1g**: Task 36 with Task 28. Task 37 after Task 28 and PG2 Task 2. Task 38 after Task 37. Task 39 after PG2 Task 58. Tasks 40 and 41 last.

Target dates (estimate, 1.5 engineers, §51 §8.5):

| Milestone | Window |
|---|---|
| NF1a | 2026-10-12 → 10-16 |
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
   - Has PG2 Task 2 (branch `backend/pg2`: pinned digests in `deploy/neon`, `deploy/loams-pg-bench` and three workflows) merged into `dev`? If not, Tasks 2 and 14 build on `backend/pg2` and say so in their PRs.
   - Is §51 §14's as-built table still true? Check `ostrium-labs/neon`'s default branch, branches, tags and active workflows; `ostrium-labs/postgres`'s `REL_*_neon` branches; the eight forks' licences.
   - **`tokio-epoll-uring`'s licence.** The brief said "no licence file and no Cargo licence field". §51 §11 found `license = "MIT OR Apache-2.0"` in `tokio-epoll-uring/Cargo.toml` and `"MIT"` in `uring-common/Cargo.toml`, at `main` and at `781989bb`, and no licence file. Re-check and record.
   - Upstream Postgres: the latest minors (expected 18.6, 17.11, 16.15), `REL_19_STABLE`'s latest tag, and the RC1 and GA dates from postgresql.org.
   - Owner approval to clone `ostrium-labs/postgres` (memory rule: never clone without asking). Until it is given, Tasks 15, 17, 24, 31 and 32 work through `gh api` and the fork's CI only.
   - Owner actions outstanding: the `ostrium-labs-fork-bot` GitHub App; teams `neon-maintainers`, `postgres-maintainers` and `security`; a token with `write:packages` for the mirror; package visibility (Q715); the runner budget (Q721).
   - Are D800–D819 and Q715–Q729 still free in `docs/design/13-decision-log.md`?
2. Commit `docs(nf1): task 0 rulings`.

## NF1a — Governance and the mirror (Tasks 1–6)

### Task 1: Mirror Neon's last public images, and audit them

**Files:** create `scripts/nf1/mirror-images.sh`, `scripts/nf1/audit-image.sh`, `deploy/pins/audit/<digest>.json` (one per mirrored index) and `scripts/nf1/tests/test_mirror.py`. Modify `deploy/neon/README.md` (a "Mirror" section).

**Interfaces:**
- `mirror-images.sh [--dry-run] <src-ref@digest> <dst-repo> <tag>` runs `podman run --rm quay.io/skopeo/stable@sha256:<pinned> copy --all --preserve-digests --retry-times 3 docker://<src> docker://<dst>:<tag>`, with credentials from `REGISTRY_AUTH_FILE`. It then verifies: `skopeo inspect --raw` of source and destination are byte-identical for the index and for every child manifest digest it lists.
- `audit-image.sh <ref@digest>` extracts each binary that has a `.dep-v0` section, for every platform, with `rust-audit-info`. It writes `{binary: [{name, version, source}]}` and exits 3 when it finds a forbidden licence package (`inferno`, or any crate whose licence in the embedded data or the crates.io index is CDDL) or a `neondatabase/subzero` source.

Images:
- `ghcr.io/neondatabase/neon@sha256:7a4f1249…` → `ghcr.io/ostrium-labs/neon:mirror-77e22e4b`;
- `ghcr.io/neondatabase/compute-node-v17@sha256:13ab146d…` → `ghcr.io/ostrium-labs/compute-node-v17:mirror-77e22e4b`;
- `compute-node-v16` of the same build: find its tag by listing tags whose `neon` label or `compute_ctl --version` names `77e22e4b`, and record the digest;
- `build-tools`, at the digest the `77e22e4b` build used (from the image's provenance attestation).

Tests:
- `mirror_digests_match`: runs the verification against a local `registry:2` with a two-platform fixture index (CI).
- `mirror_refuses_unpinned_source`: a tag instead of a digest exits 2.
- `audit_flags_cddl` and `audit_flags_subzero`: fixture `.dep-v0` payloads.
- `audit_runs_before_visibility`: the README's runbook orders "audit" before "make public", and `pins.py check` refuses a `mirror:` source whose audit file is missing.

Steps:
1. Write the tests (FAIL).
2. Write the scripts (PASS).
3. Run the mirror for real (owner token) and the audits.
4. Record the findings and both digests per image in the README table and in this plan's rulings.
5. Apply Q715. If the owner has not answered, keep the default: a package with a finding stays private.
6. Commit `deploy(neon): mirror Neon's 77e22e4b images to ghcr.io/ostrium-labs, with audits`.

### Task 2: The pin file, and every consumer generated from it

**Files:** create `deploy/pins/neon-images.toml`, `scripts/nf1/pins.py`, `scripts/nf1/tests/test_pins.py` and `.github/workflows/nf1-pins.yml`. Modify `deploy/neon/compose.yaml`, `deploy/loams-pg-bench/compose.yaml`, `.github/workflows/{pg2.yml,pg2-e2e.yml,loams-pg-bench.yml}` and `deploy/neon/README.md`.

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

Commit `deploy(neon): one pin file for the Neon images; consumers generated from it`.

### Task 3: Fork settings, branch layout and reachable pins

**Files:**
- In `ostrium-labs/neon` (branch `loams/nf1-governance`): `LOAMS.md`, and a `README.md` banner (independence statement, §51 §3.6).
- Here: `scripts/nf1/check-pinned-revs.sh`, `scripts/nf1/tests/test_pinned_revs.py`, and a job in `nf1-pins.yml`.

**Interfaces:**
- **`neon`:**
  - create `loams/main` from `main`, then merge `loams/decoder-trim` into it (the merge keeps `1218fb7a` reachable);
  - set `loams/main` as the default branch;
  - protect `loams/*` and `main` as in the shared contract;
  - tag `loams-20261012-1` on the merge.
- **The other forks:** create `loams/<upstream branch>` and protect it. `autoscaling` and `framed-websockets` are archived once their `loams/` branch exists (`azure-sdk-for-rust` stays unarchived, frozen).
- **`postgres`:** disable Actions (`PUT /repos/ostrium-labs/postgres/actions/permissions` `{enabled:false}`). Protect `REL_*_STABLE_neon` (mirror) and `loams/*`.
- `check-pinned-revs.sh` finds every `github.com/ostrium-labs/<repo>` rev in any `Cargo.toml` here. For each, it asks `gh api repos/ostrium-labs/<repo>/compare/<rev>...loams/main` (or the repository's `loams/` branch). The status must be `behind` or `identical`, or the rev must be the target of a `loams-*` tag.

Tests:
- `pinned_revs_reachable`: today's decoder rev passes after the merge.
- `unreachable_rev_fails`: a fixture rev.
- `force_push_rejected`: a probe push with `--force` to a throwaway `loams/probe` branch under the same protection is refused; the branch is then deleted. The result is recorded.

Owner actions: the teams and the GitHub App (Task 0).

Commits:
- fork: `loams-ci: loams/main as the default branch; fork charter` (`Loams-Patch: NF-0000`, the charter's row);
- here: `ci(neon): check that pinned fork revisions stay reachable`.

### Task 4: The patch queue

**Files** (fork, `loams/nf1-governance`): `LOAMS_PATCHES.md`, `scripts/loams/check-patches.py`, `scripts/loams/tests/test_check_patches.py` and `.github/workflows/loams-patches.yml`.

**Interfaces:**
- `check-patches.py --base main --head HEAD` lists the non-merge commits in `main..HEAD`. It fails when a commit has no `Loams-Patch` trailer, a trailer's id has no row, or a row has no commit. Merge commits with `Loams-Sync` are exempt.
- The first rows: NF-0000 (charter), NF-0001 (decoder trim, `1218fb7a`).

Tests:
- `unlisted_patch_fails`
- `trailer_without_row_fails`
- `row_without_commit_fails`
- `upstream_commits_exempt`
- `sync_merge_exempt`

All run on a temporary git repository built in the test.

Commit `loams-ci: the patch queue and its check` (NF-0000).

### Task 5: CODEOWNERS, DCO, SECURITY and NOTICE

**Files:**
- Fork `neon`: `CODEOWNERS`, `SECURITY.md`, `NOTICE`, `.github/workflows/loams-dco.yml`, `scripts/loams/check-dco.sh` (copied from Loams' `scripts/ci/check-dco.sh`, with the base at the merge base with `main`).
- `SECURITY.md` and `CODEOWNERS` also go into `postgres` (on `loams/REL_17_STABLE`, created in Task 15; until then on a `loams/meta` branch), `tokio-epoll-uring` and `pg_session_jwt`.

**Interfaces:**
- CODEOWNERS as §51 §3.5.
- `SECURITY.md` points at Loams' policy and private reporting.
- `NOTICE` keeps Neon's lines and appends: "Modifications Copyright 2026 Ostrium Labs and the Loams Authors. Changes from Neon are listed in LOAMS_PATCHES.md."

Tests:
- `codeowners_valid`: `gh api repos/ostrium-labs/neon/codeowners/errors` returns `[]`.
- `dco_rejects_unsigned`: the copied script's own test.
- `notice_retains_upstream`: the first three lines equal upstream's.

Owner action: name the two maintainers (Q722).

Commit `loams-licence: CODEOWNERS, DCO, SECURITY, NOTICE` (NF-0000).

### Task 6: Advisories and response targets

**Files** (fork): `.github/workflows/loams-advisories.yml`, `scripts/loams/sla.py`, `scripts/loams/pg-security-feed.py` and `scripts/loams/tests/test_sla.py`.

**Interfaces:**
- **Daily:**
  - `cargo deny check advisories` on `Cargo.lock`;
  - `trivy image --severity HIGH,CRITICAL --format json` on every digest in Loams' pin file (fetched from `ostrium-labs/loams`);
  - `pg-security-feed.py` reads postgresql.org's release feed and opens an issue when a release names security fixes for a supported or maintenance major;
  - GitHub advisories on `neondatabase/neon`, `pgdogdev/pgdog` and each manifest extension's repository.
- `sla.py` labels issues `sev:critical|high|pg-minor|other` and sets the due date from §51 §3.7's table (Q724). After the due date it escalates: an `@ostrium-labs/security` mention and the `overdue` label.

Tests:
- `pg_security_release_opens_issue`: a fixture feed.
- `due_dates_follow_targets`
- `overdue_issue_escalates`
- `duplicate_finding_not_reopened`

Commit `loams-ci: advisories and response targets` (NF-0008).

## NF1b — Our own builds (Tasks 7–14)

### Task 7: Build hygiene patches

**Files** (fork, `loams/nf1-hygiene`):
- delete `.github/workflows/*.yml` except `loams-*.yml`, and delete `.github/actions/prepare-for-subzero/`;
- modify `Cargo.toml` and `Cargo.lock`, `deny.toml`, `Dockerfile`, `compute/compute-node.Dockerfile`, `build-tools/Dockerfile`, `proxy/README.md` and `.config/hakari.toml` (only if hakari needs it);
- add `scripts/loams/strip-upstream-ci.sh`, `scripts/loams/check-no-private-sources.sh` and `scripts/loams/tests/test_hygiene.sh`.

**Interfaces** (one `Loams-Patch` id each):
- **NF-0002, upstream CI removed.** `strip-upstream-ci.sh` deletes every workflow without the `loams-` prefix. It is idempotent, and the sync (Task 19) reruns it.
- **NF-0003, no CDDL.** In the workspace `Cargo.toml`: `pprof` features `["criterion", "frame-pointer", "prost-codec"]` and `jemalloc_pprof` features `["symbolize"]`, both without `flamegraph`. Remove `CDDL-1.0` from `deny.toml`'s allow list. `http-utils`' profiling routes answer 400 `{"error":"svg flamegraphs are not built; use format=pprof"}` for `format=svg`.
- **NF-0004, no private Data API.** Delete the `SUBZERO_ACCESS_TOKEN` blocks from `Dockerfile` (keep `cargo chef prepare` and `cook` unconditional). Keep `libs/proxy/subzero_core` (the stub) so the workspace builds. Remove `rest_broker` from every CI feature list. Replace `proxy/README.md`'s instructions with a note that the REST broker is not supported in this fork (Q720).
- **NF-0005, git dependencies to `ostrium-labs` by rev.** `framed-websockets`, `tokio-epoll-uring`, `rust-postgres` (four crates and the `[patch.crates-io]` entry) and the `azure_*` crates use `git = "https://github.com/ostrium-labs/<repo>", rev = "<sha>"`, at the revisions `Cargo.lock` has today, each tagged `loams-<date>-1` in its fork.
- **NF-0006, no `neondatabase` downloads.**
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

Commit one per patch id, e.g. `loams-licence: build pprof without flamegraph; CDDL out of deny.toml` (NF-0003).

### Task 8: `tokio-epoll-uring`'s licence

**Files:**
- In `ostrium-labs/tokio-epoll-uring` (`loams/main`): `LICENSE-MIT`, `LICENSE-APACHE`, `uring-common/LICENSE-tokio-uring` and `LICENSING.md`.
- In the `neon` fork: `docs/loams/LICENSING-tokio-epoll-uring.md`, and the `about.toml` used by `cargo about` (Task 10).

**Interfaces:**
- **The licence texts as declared** (§51 §11): MIT and Apache-2.0 for `tokio-epoll-uring`, attributed to "the tokio-epoll-uring authors (Neon Inc.)". MIT plus `tokio-uring`'s copyright notice for `uring-common`'s vendored buffer code (taken from the `tokio-uring` release the vendoring commit names).
- `LICENSING.md` cites the manifest lines and upstream commits (`781989bb`, PR #24).
- Move NF-0005's `tokio-epoll-uring` rev to the commit with the texts.
- **If Q716 allows it:** file one issue on `neondatabase/tokio-epoll-uring` asking for licence files that match the manifests. Record the link.
- **Fallback, only if Q716 or counsel says the declaration is insufficient:** a pageserver feature `io-uring`, on by default. Off, the `TokioEpollUring` engine is compiled out and `uring-common`'s traits are vendored under `pageserver/src/virtual_file/owned_buffers_io/buf/` with the MIT notice. G1 then runs both builds. Estimate: 1–2 engineer-weeks. It goes on a separate branch, `loams/nf1-no-uring`, and is not merged unless needed.

Tests:
- `third_party_licences_include_tokio_epoll_uring`: Task 10's generated licence file contains both texts.
- With the fallback only: `cargo check -p pageserver --no-default-features --features <rest>` and the G1 unit tests on `StdFs`.

Commit `loams-licence: licence texts for tokio-epoll-uring as its manifests declare` (NF-0007).

### Task 9: Our `build-tools` image

**Files** (fork): `.github/workflows/loams-build-tools.yml`, `build-tools/Dockerfile` (NF-0006's edits), and the `Dockerfile` and compute Dockerfile `ARG TAG`, which becomes a digest (`ARG BUILD_TOOLS=ghcr.io/ostrium-labs/build-tools@sha256:…`).

**Interfaces:**
- Multi-arch and cached as in Task 10. Signed and attested through Task 12's `loams-sign.yml` (Task 9 lands first with the attestation steps inline, and switches to the reusable workflow when Task 12 merges).
- Triggered by changes under `build-tools/`, monthly, and manually.
- The digest is written into both Dockerfiles by a bot PR.

Tests:
- The workflow builds on PR (no push).
- `build_tools_pinned_by_digest`: a grep check in G0.

Commit `loams-build: build-tools image from the fork` (NF-0009).

### Task 10: The `neon` image workflow

**Files** (fork): `.github/workflows/loams-build-neon.yml`, `scripts/loams/audit-image.sh` (the same contract as Task 1's, vendored), `about.toml`, and `Dockerfile` (labels; `/usr/share/doc/loams-neon/{LICENSE,NOTICE,LOAMS_PATCHES.md,THIRD_PARTY_LICENSES.html}`).

**Interfaces:**
- Jobs `build (amd64)` and `build (arm64)` on `vars.RUNNER_HEAVY` and `vars.RUNNER_HEAVY_ARM` (defaults `ubuntu-24.04` and `ubuntu-24.04-arm`):
  - BuildKit's root on `/mnt`;
  - `cache-from` and `cache-to` `type=registry,ref=ghcr.io/ostrium-labs/buildcache:neon-<arch>,mode=max` (cache-to only from `loams/main`);
  - `cargo auditable build` (already in the Dockerfile);
  - push by digest only;
  - build args `GIT_VERSION=<sha>` and `BUILD_TAG=nf-…|<sha12>`.
- Job `index`: `docker buildx imagetools create` with tags `<sha12>` and `edge` (release tags come from Task 13).
- Job `smoke`:
  - `pageserver --version` names the commit;
  - start a minimal `deploy/neon`-shaped stack (pageserver, broker, one stock safekeeper as the reference, compute from the pinned compute image), create a tenant and timeline, and run `SELECT 1` through the compute;
  - `audit-image.sh` exits 0.
- `THIRD_PARTY_LICENSES.html` comes from `cargo about generate` with `about.toml`'s accepted licences (no CDDL).

Tests:
- `neon_image_audit_clean`
- `smoke_select_1`
- `labels_present` (`org.opencontainers.image.revision` equals the commit)
- The measured build times go into the rulings and §51 §5.2's table (Q721).

Commit `loams-build: neon image workflow` (NF-0010).

### Task 11: The compute image workflow

**Files** (fork): `.github/workflows/loams-build-compute.yml`, `compute/compute-node.Dockerfile` (labels; `/usr/share/doc/loams-compute/`; `ARG BUILD_TOOLS`), and `scripts/loams/compute-heavy-stages.txt`.

**Interfaces:**
- Matrix `pg ∈ pins.supported ∪ maintenance ∪ beta` (today 16 and 17) × `arch ∈ {amd64, arm64}` (Q729).
- Heavy stages (`plv8-build`, `rdkit-build`, `postgis-build`, `pgrouting-build`, and while catalogued `pg_duckdb-build`) build in separate jobs with `--target <stage>` that push only the cache (`buildcache:compute-v<N>-<arch>`). The final job then builds the image from a warm cache, within 6 h.
- Index tags as in the shared contract.
- Job `smoke`: start the compute against the Task 10 stack. For every `core` extension in the manifest (before Task 20: the list in §51 §7.1), run `CREATE EXTENSION`; then run `pg_stat_statements` and `vector` smoke queries. `audit-image.sh` on `compute_ctl`, `local_proxy` and `fast_import`.

Tests:
- `compute_image_audit_clean`
- `core_extensions_create`
- `heavy_stage_jobs_under_limit` (the run's timings recorded)
- Cold and warm durations per major and arch recorded in the rulings; Q721's budget checked against them.

Commit `loams-build: compute image workflow` (NF-0011).

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
- `wrong_identity_fails_verify`: a fixture signed from a branch other than `loams/main`.
- `sbom_has_no_forbidden_licence`
- `gpl_sources_attached`: `oras discover` lists the artifact on a compute digest.
- `mirror_pin_skipped_with_notice`

Commits:
- fork: `loams-ci: sign, attest and attach sources` (NF-0012);
- here: `ci(neon): verify the pinned Neon images' signatures and attestations`.

### Task 13: Releases, tags and cadence

**Files** (fork): `.github/workflows/loams-release.yml`, `scripts/loams/release-notes.py`, `docs/loams/release.md` and `scripts/loams/tests/test_release.py`.

**Interfaces:**
- `workflow_dispatch(inputs: kind = neon|compute|full, version = nf-YYYY.MM.N)`, also called by Task 18.
- **Steps:**
  1. Refuse if the `nf-` tag exists.
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

Commit `loams-ci: releases nf-YYYY.MM.N with pin PRs to Loams` (NF-0013).

### Task 14: The parity release, and Loams on our images

**Files:**
- Fork: none beyond running Task 13.
- Here: `deploy/pins/neon-images.toml` (by the bot PR), the consumers, `deploy/neon/README.md`, and `crates/loams-neon/tests/fixtures/README.md` (a note that the fixtures were re-checked).

Steps:
1. Cut `nf-2026.10.0` from `loams/main` with **the same Postgres revisions as the mirror** (16.9, 17.5; §51 §5.5).
2. Merge the bot PR here.
3. Run:
   - `loams-neon`'s fixture tests (byte-equal);
   - `pg2-e2e.yml` (`it_deploy_neon_tenant_timeline_branch`, `it-pageserver-loams-wal.sh`);
   - `loams-wal-decoder`'s golden tests with `PG_HEADERS_IMAGE` on the new `neon` digest (headers identical, R31.6);
   - the desktop's stack copy on a scratch profile (start, create a branch, `SELECT 1`).
4. Any difference is a pipeline defect: fix it and re-release `nf-2026.10.1`.
5. Once green, compose defaults point at `ghcr.io/ostrium-labs`, which closes Q715's default path.

Tests: the above, all green. `pins.py check` shows no `mirror:` source left except as history in the README.

Commit `deploy(neon): pin ostrium-labs builds nf-2026.10.0`.

## NF1c — Upstream sync and Postgres minors (Tasks 15–19)

### Task 15: The Postgres fork's branches

**Files:** `ostrium-labs/postgres` branches. In the `neon` fork: `docs/loams/postgres-branches.md`.

**Interfaces:**
- For each major 16, 17 and 18: `loams/REL_<N>_STABLE` created from `REL_<N>_STABLE_neon`'s head (`a616eefe` for 18), unchanged.
- `REL_<N>_STABLE_neon` is kept as Neon's mirror.
- Protection as Task 3.
- Without a local clone (Task 0), branches are created through `gh api repos/ostrium-labs/postgres/git/refs`.

Tests:
- `branches_exist_and_protected` (`gh api` probe)
- `submodule_url_relative`: `.gitmodules` in `loams/main` still uses `../postgres.git`, so CI resolves to `ostrium-labs/postgres`.

Commit (fork) `loams-ci: postgres branch layout` (NF-0014).

### Task 16: The test gate (G0–G3)

**Files:**
- Fork: `.github/workflows/loams-gate.yml` and `scripts/loams/gate-subset.txt` (G2's `test_runner` selection, with each name checked to exist).
- Here: `.github/workflows/nf1-candidate.yml`.

**Interfaces:**
- **G0 to G2** as §51 §6.4.
  - G1 runs `cargo nextest run --locked` on the listed crates, for each supported major, with `NEON_PAGESERVER_UNIT_TEST_VIRTUAL_FILE_IOENGINE` set to `std-fs` and to `tokio-epoll-uring`.
  - G2 runs `./scripts/pytest` (Neon's runner) on `gate-subset.txt` with `neon_local`, one job per major. The subset: `test_pg_regress`, branching, `compute_ctl` spec and reconfigure, `import_pgdata`, LFC, and the walproposer against a safekeeper. Names are confirmed in this task.
- **G3:**
  - the fork calls `gh workflow run nf1-candidate.yml --repo ostrium-labs/loams -f neon=<digest> -f compute17=<digest> …` with the bot's token;
  - here, `nf1-candidate.yml` overrides the pin file in the job and runs `pg2-e2e.yml`'s jobs, the decoder's golden tests and the extension smoke;
  - it reports a commit status back to the fork's commit (`loams-gate / g3`).

Tests:
- `gate_fails_on_red_tier`: a fixture PR with a failing G1 test is blocked.
- `required_checks_configured`: `gh api …/branches/loams%2Fmain/protection` lists `g0`, `g1`, `dco`, `patches`.
- `g3_reports_status`: a dry-run candidate with the current digests reports success.

Commits:
- fork: `loams-ci: the four-tier gate` (NF-0015);
- here: `ci(neon): candidate run for fork releases`.

### Task 17: The catch-up: 17.11 and 16.15 (then 17.12 and 16.16)

**Files:**
- `ostrium-labs/postgres` `loams/REL_17_STABLE` and `loams/REL_16_STABLE`.
- In the `neon` fork: `vendor/postgres-v17`, `vendor/postgres-v16`, `vendor/revisions.json`, `pgxn/neon` and `compute/` (fixes as needed).

**Interfaces:**
- Merge the upstream tags (`REL_17_11`, `REL_16_15`) into `loams/REL_<N>_STABLE`. Each conflict's resolution is recorded in the merge commit's message.
- Move the submodules to the merge commits and update `revisions.json` (`"v17": ["17.11", "<sha>"]`).
- Fix what G1 and G2 find in `pgxn/neon`, the extensions and `compute_ctl`.
- Release `nf-2026.11.0` (compute 16 and 17, and `neon` for its WAL-redo binaries).
- When 17.12 and 16.16 ship on 2026-11-12, Task 18's job merges them, and the release `nf-2026.11.1` ships by 2026-11-19.

Tests:
- G1 and G2 at 16 and 17.
- G3: Loams' integration, plus `loams-neon` fixtures re-recorded only if a response shape changed, with the diff reviewed.
- `select version()` on the compute answers 17.11 (then 17.12).
- `deploy/neon`'s README smoke.

Commits:
- postgres: `Merge REL_17_11 into loams/REL_17_STABLE` (`Loams-Sync: REL_17_11`);
- neon: `loams-pg: Postgres 17.11 and 16.15` (NF-0016).

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

Commit `loams-ci: nightly Postgres minor tracking` (NF-0017).

### Task 19: The weekly upstream sync

**Files** (fork): `.github/workflows/loams-sync-upstream.yml`, `scripts/loams/sync-upstream.sh`, `docs/loams/sync.md` and `scripts/loams/tests/test_sync.sh`.

**Interfaces:**
- **Monday:**
  1. `git fetch https://github.com/neondatabase/neon main`;
  2. if `main` is an ancestor, push the fast-forward with `GITHUB_TOKEN`, otherwise fail and open an issue (never force);
  3. create `loams/sync-<date>`, `git merge --no-ff main -m "Merge upstream" --trailer "Loams-Sync: <sha>"`;
  4. run `strip-upstream-ci.sh`, and commit any re-deletion under NF-0002;
  5. open a PR. The gate runs.
- **Monthly**, the same for the small forks (only the `main` or `neon` mirror branch; a PR only if Neon's lock changes a pin).
- `sync.md` is the conflict playbook. It covers:
  - keep upstream's code, then re-apply a `Loams-Patch` by hand;
  - never drop a `LOAMS_PATCHES.md` row in a sync;
  - escalate after two failed weeks.

Tests:
- `sync_refuses_non_ff`
- `sync_restrips_upstream_ci`
- `sync_noop_when_no_upstream_change`
- `sync_pr_has_trailer`

Commit `loams-ci: weekly upstream sync` (NF-0018).

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

Commit `loams-ext: extension manifest and licence check` (NF-0019).

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

Commit `loams-ext: the catalogue's tiers; dropped extensions removed` (NF-0020).

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

Commit `loams-ext: the loams extension` (NF-0021).

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

Commit `loams-ci: monthly extension updates` (NF-0022).

## NF1e — Postgres 18 (Tasks 24–30)

### Task 24: Postgres 18 in the fork

**Files:**
- `ostrium-labs/postgres` `loams/REL_18_STABLE` (18.2 → merge `REL_18_6`, and later `REL_18_7`).
- In the `neon` fork: `.gitmodules` (`vendor/postgres-v18`, branch `loams/REL_18_STABLE`), `vendor/revisions.json`, `postgres.mk`, `Makefile`, `build-tools/Dockerfile` (if 18 needs new build dependencies), and the `Dockerfile`'s Postgres build for all majors.

**Interfaces:**
- `make postgres-v18` builds.
- The `neon` image contains `/usr/local/v18`.
- 14 and 15 are removed from the build loops (their submodules stay until Task 41).

Tests:
- `make check` on `loams/REL_18_STABLE` in CI.
- The `neon` image's smoke shows `/usr/local/v18/bin/postgres --version` = 18.6.

Commits:
- postgres: `Merge REL_18_6 into loams/REL_18_STABLE`;
- neon: `loams-pg18: vendor Postgres 18` (NF-0023).

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

Commit `loams-pg18: format diff; versioninfo and ffi for 18` (NF-0024).

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

Commit `loams-pg18: wal_decoder and pageserver for 18` (NF-0025).

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

Commit `loams-pg18: neon extension, walproposer and compute_ctl for 18` (NF-0026).

### Task 28: `compute-node-v18` (preview)

**Files** (fork): `compute/compute-node.Dockerfile` (`PG_VERSION=v18` in every per-major `case`; versions of the `core` and `included` extensions that support 18; the others marked `majors` without 18), the manifest, and `loams-build-compute.yml`'s matrix (from the pin file's majors plus `preview = [18]`).

**Interfaces:**
- Images `compute-node-v18:18.6-<sha12>`, with `edge` only. Not in Loams' pin file's `supported` list yet.
- The Debian base follows Q727 (default: `trixie` for 18 and the `neon` image).
- Extensions that do not support 18 are listed in the release notes as "not yet at 18".

Tests:
- `core_extensions_survive_branch` at 18
- `included_extensions_create` at 18
- `compute_image_audit_clean` at 18

Commit `loams-pg18: compute-node-v18 preview images` (NF-0027).

### Task 29: Loams at 18: decoder, safekeeper, `loams-neon`

**Files** (here): `crates/loams-wal-decoder/{Cargo.toml,Cargo.lock,tests/golden.rs,tests/fixtures/fork-sender-18-*.bin,tests/fixtures/README.md}`, `crates/loams-safekeeper/src/{proto.rs,http.rs}` (only if version handling changes), `crates/loams-neon/src/{majors.rs,pageserver.rs}` (generated `majors.rs`; `pg_version` validation), `scripts/pg2/pg-headers.sh` (`v18`, and `v19` for later), `.github/workflows/pg2-e2e.yml` (a matrix over the pin file's supported, preview and beta majors), and `deploy/neon/compose.yaml` (a `compute-v18` service under profile `pg18`).

**Interfaces:**
- The decoder's four fork crates move to the `loams/main` rev that has Task 26, tagged `loams-<date>-<n>`. `check-pinned-revs.sh` passes.
- Golden fixtures at 18 are recorded from the fork's own interpreted sender (the method of PG2 R31.x, documented in `tests/fixtures/README.md`).
- `loams-safekeeper` accepts `pg_version` 18xxxx (its `full_pg_version` and the greeting are version-agnostic today; confirm and add a test).
- `loams-neon` refuses a `pg_version` outside the supported, preview, beta and maintenance majors (`unsupported_pg_version` reason, registered in `docs/api/reasons.md`).

Tests:
- `golden_pg18_matches_fork_sender`
- `it_pageserver_ingests_from_loams_wal_pg18` (`scripts/pg2/it-pageserver-loams-wal.sh` with `PG_VERSION=18`)
- `greeting_accepts_pg18`
- `create_timeline_rejects_unsupported_major`
- `cargo deny check` on the decoder's own policy

Commit `wal: the decoder and loams-wal at Postgres 18`.

### Task 30: The Postgres 18 gate and promotion (Q717)

**Files:** here, `deploy/pins/neon-images.toml` (by the release PR) and `deploy/neon/README.md`; in the fork, `docs/loams/release.md`.

Steps:
1. Run §51 §8.4's matrix at 18, twice, on two different days, including:
   - Task 36's PgDog results at 18;
   - Task 37's 17 → 18 upgrade test (if Task 37 is not done yet, the promotion waits for it);
   - the desktop's local stack at 18.
2. When both runs are green, release with `supported = [17, 18]` (and, per Q717's default, `default = 18`).
3. Record the evidence links in the rulings. PG2's Task 0 ruling 7 is then superseded in practice.

Tests: the matrix; `pins.py check`; `nf1-pins.yml` `verify`.

Commit `deploy(neon): Postgres 18 supported`.

## NF1f — Postgres 19 (Tasks 31–35)

### Task 31: Track 19 from RC1

**Files:**
- `ostrium-labs/postgres` `loams/REL_19_STABLE` (from `postgres/postgres` `REL_19_STABLE`; vanilla to start).
- In the `neon` fork: `docs/loams/pg19-format-diff.md`, and `.github/workflows/loams-postgres-minors.yml` (19 added to the nightly).

**Interfaces:**
- From 2026-10-15 (RC1): a nightly vanilla build and `make check` of 19 in `build-tools`.
- **The 18 → 19 diff**, as Task 25, read at RC1 and updated at GA (2026-10-29). It specifically covers the multixact offset width (§51 §8.3) and every change to SLRU, the control file, smgr and WAL records.

Tests: the nightly build; the diff's checklist has no "unclassified" items.

Commit `loams-pg19: track Postgres 19; format diff` (NF-0028).

### Task 32: Port Neon's patch series to 19

**Files:** `ostrium-labs/postgres` `loams/REL_19_STABLE`, and the fork's `LOAMS_PATCHES.md` (one row per ported change).

**Interfaces:**
- Cherry-pick Neon's changes from `loams/REL_18_STABLE` (the commits over `REL_18_STABLE`) in `docs/core_changes.md`'s order. Each picked commit gets `Loams-Patch: NF-19xx` and a `(ported from <18 sha>)` line.
- Changes that 19 made unnecessary are dropped with a reason in the inventory.
- A change that needs a redesign at 19 (for example multixact handling) gets its own row and test.

Tests:
- `make check` and `installcheck-world` on `loams/REL_19_STABLE`.
- Neon's Postgres-side tests that run without the storage (the `neon_test_utils` paths).

Commit (postgres) one per ported change; (neon) `loams-pg19: patch inventory for 19` (NF-0029).

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

Commit `loams-pg19: versioninfo, ffi, decoder, pageserver, neon extension and compute_ctl for 19` (NF-0030).

### Task 34: `compute-node-v19` beta, and Loams at 19

**Files:**
- Fork: the compute Dockerfile and manifest (`core` at 19 only; `included` as each supports 19), and `loams-build-compute.yml` (tags with `-beta`; amd64 only per Q729's default).
- Here: the decoder's rev and `fork-sender-19-*.bin` fixtures; the pin file (`beta = [19]`, by the release PR); `deploy/neon/compose.yaml` (a `compute-v19` service under profile `beta`); and `pg2-e2e.yml` (19 in the matrix with `continue-on-error: true` until Task 35).

**Interfaces:**
- `compute-node-v19:19.<m>-beta-<sha12>` and `beta`.
- `loams-neon` and `pg-control` accept 19 only for projects with `allow_beta_major` (PG2's `CreateProject` gains the field; a note for PG2 Task 1's owner, recorded here).

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

Commit `deploy(neon): Postgres 19 supported`.

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
- Here: `crates/loams-neon/src/pageserver.rs` (`create_timeline_import(t, tl, ImportPgdata { location: AwsS3 { region, bucket, key }, idempotency_key })` and `import_status`), `crates/loams-neon/tests/fixtures/` (the import request and response), and `scripts/pg2/upgrade-branch.sh` (the local and desktop driver).

**Interfaces:**
- **Path 1 of §51 §9:**
  1. a read-only compute of the source branch;
  2. `fast_import pgdata` from the target major's compute image to the bucket prefix `pgupgrade/<tenant>/<new timeline>/`;
  3. the import into a new root timeline of the same tenant;
  4. a `loams-wal` timeline at the import's end LSN;
  5. a new branch `<name>-pg<N>`.
- The swap is PG2's (Task 51's `UpgradeProject`, amended). This task delivers the pieces and the script.
- **`compute_major_mismatch_refused`:** `loams-neon`'s `ComputeSpecBuilder` refuses a compute image whose major differs from the timeline's `pg_version`.

Tests:
- `upgrade_17_to_18_preserves_checksums`: `deploy/neon` with RustFS, a pgbench database at 17, upgraded, with per-table checksums equal.
- `upgrade_failure_keeps_source_serving`: the import is killed mid-way; the source branch is untouched and still writable.
- `upgrade_is_idempotent_by_key`: a retried import with the same key creates one timeline.
- `compute_major_mismatch_refused`

Then update PG2 Task 51 (a note in its plan) to use these pieces, with dump and restore as the fallback.

Commit `neon: upgrade a branch to a new major through fast_import and ImportPgdata`.

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

**Precondition:** PG2 Task 58 has merged. AP1e Tasks 21–23 are already built (PG2 Task 0 ruling 8): `apps/desktop-electron/src/main/sql/{neon.ts,pg.ts}` drive `deploy/neon`, and `pg.ts`'s `PostgresBackend` is the seam.

**Files:** `deploy/neon/compose.yaml` (`compute-v<N>` services generated from the pin file; profiles `pg18` and `beta`), `apps/desktop-electron/src/main/stacks/stacks.ts` (the version marker includes `fork_release`), `apps/desktop-electron/src/main/sql/backend/local-stack.ts`, `web/plugins/postgres/` (the upgrade prompt), and `apps/desktop-electron/test/pg-majors.test.ts`.

**Interfaces:**
- The local-stack backend reads each timeline's `pg_version` and starts the compute of that major. A new tenant uses `majors.default`.
- When a branch's major is below `default`, the page shows "Upgrade to Postgres <N>". On confirmation it runs Task 37's script locally and shows the new branch. The old one stays.
- A 16 timeline pulls the mirrored `compute-node-v16` on demand for export or upgrade.
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
- Here: `docs/runbooks/neon-fork/{release.md,minor-release.md,security.md,sync.md,new-major.md,mvm.md}`.
- Fork: `.github/workflows/loams-*.yml` (`if: vars.MAINTENANCE_MODE != 'mvm'` on the jobs MVM stops, §51 §13.3) and `scripts/loams/tests/test_mvm.py`.

**Interfaces:**
- Each runbook is a numbered procedure with the commands, the expected output and the escalation. Each is executed once by the second maintainer (Q722) and marked verified with the date.
- **`MAINTENANCE_MODE`:**
  - `full` (default) or `mvm`;
  - in `mvm`, weekly sync becomes monthly, extension updates run only on advisories, and new-major jobs are skipped;
  - minors, advisories, the Debian rebuild and the gate are never skipped.
- Record the first quarter's actual hours against §51 §13's estimates in the rulings.

Tests:
- `mvm_mode_keeps_security_jobs`: parses the workflows and asserts that the minor, advisory and gate jobs have no `MAINTENANCE_MODE` condition.
- `mvm_mode_skips_listed_jobs`
- `actionlint`

Commit `docs(neon): fork runbooks; ci: the MVM switch`.

### Task 41: Docs, status and cleanup

**Files:** `docs/design/51-neon-fork.md` (§14 as built, the evidence links), `docs/design/46-loams-postgres-production.md` (§12's fork-release line, §19), `docs/design/28-loams-postgres.md` (§10's header note), `docs/design/13-decision-log.md` (statuses), `docs/plans/README.md` (status), this plan's exit checklist, and the fork's `vendor/postgres-v14` and `-v15` submodules (removed, NF-0031, after 14's EOL on 2026-11-12).

Steps:
1. Tick each exit item with a link.
2. Update the statuses.
3. Remove 14 and 15 from the fork.

Commit `docs(nf1): exit, as built and status`.

---

## Exit criteria (with the owning tasks)

- [ ] **Inventory and governance:** eight forks with branches, protection, CODEOWNERS (two maintainers), DCO, `SECURITY.md`, `NOTICE`; the patch queue enforced: Tasks 3–5.
- [ ] **Mirror:** both pinned images (and v16) mirrored with identical digests and audited; visibility per Q715: Task 1.
- [ ] **Own builds:** `neon`, `compute-node-v16`, `-v17` and `-v18` (and `-v19` beta) multi-arch from `loams/main`; no CDDL, no private source, no `neondatabase` fetch: Tasks 7, 9–11.
- [ ] **Supply chain:** provenance, SBOM, cosign, GPL sources; Loams verifies every pin: Task 12.
- [ ] **Releases:** `nf-YYYY.MM.N` with pin PRs; the parity release; Loams on our images: Tasks 2, 13, 14.
- [ ] **Minors:** 17.11 and 16.15 shipped; 17.12, 18.7 and 16.16 within 7 days of 2026-11-12; nightly tracking; the weekly sync: Tasks 17–19.
- [ ] **Gate:** G0–G3 required on `loams/main`: Task 16.
- [ ] **Security:** advisories daily; targets in force: Task 6.
- [ ] **Extensions:** manifest, licence check, tiers, the `loams` extension, monthly updates: Tasks 20–23.
- [ ] **Postgres 18 supported** in Loams, with the matrix green twice: Tasks 24–30.
- [ ] **Postgres 19 beta** images and Loams support behind `allow_beta_major`; **19 supported** only after Q718's conditions: Tasks 31–35.
- [ ] **Upgrades:** 17 → 18 and 18 → 19 through `fast_import` and import, with checksums equal; PG2 Task 51 uses it; the `pg_upgrade` spike ruled: Tasks 37, 38.
- [ ] **PgDog** results per major: Task 36.
- [ ] **Desktop:** per-timeline majors, the upgrade prompt, beta hidden: Task 39.
- [ ] **`tokio-epoll-uring`:** licence texts and the upstream ask per Q716 (or the fallback build): Task 8.
- [ ] **Ownership:** runbooks verified by the second maintainer; the MVM switch; actual hours recorded: Task 40.

## Self-review

- **Spec coverage.** All ten areas of the owner's brief map to tasks:

  | Area | Design | Tasks |
  |---|---|---|
  | 1. Inventory and governance | §51 §3 | 3–6 |
  | 2. Mirror | §51 §4 | 1, 2 |
  | 3. Own builds | §51 §5 | 7–14 |
  | 4. Sync | §51 §6 | 15–19 |
  | 5. Extensions | §51 §7 | 20–23 |
  | 6. Postgres 18 and 19 | §51 §8, §9 | 24–35, 37, 38 |
  | 7. Private dependencies | §51 §10 | 7 |
  | 8. `tokio-epoll-uring` | §51 §11 | 8 |
  | 9. Integration | §51 §12 | 2, 14, 29, 34, 36, 39 |
  | 10. Staffing | §51 §13 | 40 |

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
  | Q722 (maintainers) | Task 5 |
  | Q724 (security targets) | Task 6 |
  | Q727 (Debian base) | Task 28 |
  | Q729 (arm64 compute) | Task 11 |
  | Q717 (default major) | Task 30 |
  | Q718 (19 GA) | Task 35 |
  | Q725 (`pg_upgrade`) | Task 38 |
  | Q728 (desktop prompt) | Task 39 |
  | Q723 (names) | Cloud GA |
  | Q726 (reproducibility) | Before or at Task 12 |

  The owner actions: the GitHub App, the teams, a packages token, package visibility, billing, and the clone of `ostrium-labs/postgres`.

## Rulings made during execution

*(None yet. Task 0 records the first ones.)*
