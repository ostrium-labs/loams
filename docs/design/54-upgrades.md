# 54 — Upgrades: Keeping Data from One Release to the Next

Status: **Approved** (owner: direction 2026-10-13; D923–D945 and the UP1 rulings 2026-10-16) · 2026-10-16. Source: the owner's direction of 2026-10-13 (§1.1), which is binding. Upgrades ship in **v0.1.0-preview.3**, so that preview.2 → preview.3 is the first upgrade that keeps data. Weekly previews follow, and v0.1.0's exit criteria include a tested upgrade path. Decisions **D923–D945** and open questions **Q820–Q834** are declared in the [decision log](13-decision-log.md). Rows that restate the owner's direction are *Approved*; rows that add detail to it are *Proposed* until the owner confirms them. Plan: [UP1](../plans/2026-10-14-up1-upgrades.md). **No code is written by this document.**

This document builds on [§10](10-operations.md) §6–§7 (backups, format version N/N−1, rolling upgrades), [§03](03-storage-formats.md) §6–§7 (format versioning, garbage collection), [§30](30-loams-cli.md) (the CLI) and [CL1](../plans/2026-10-10-cl1-loams-cli.md), [§41](41-multitenant-byoc-control-plane.md) §8 (upgrade waves and rings), [§44](44-unified-api-and-sdks.md) (the API rules and `GetInstance`), and on the per-product upgrade sections: [§45](45-loams-live-production.md) §4.3 (Live), [§46](46-loams-postgres-production.md) §12 and [§51](51-loams-postgres-fork.md) §9 (Postgres), [§47](47-loams-sql-production.md) §5 (SQL bootstrap), [§48](48-loams-graph-production.md) §14.4 (Graph), [§49](49-loams-house-production.md) §19 (House) and [§50](50-loams-desktop-daemon.md) §5.3 (the desktop daemon). §15 below reconciles each of them; none is contradicted.

Markers: **(verify)** means not checked against a primary source; the plan task that depends on the fact checks it first. **(estimate)** means computed or judged, not measured. **(target)** is a number this document sets as a gate.

---

## 1. Summary

### 1.1 The owner's direction (2026-10-13, binding)

1. **Upgrades keep data from v0.1.0-preview.3 on.** preview.2 → preview.3 is the first upgrade that keeps data; preview.1 → preview.2 stays a fresh stack. Weekly previews follow. v0.1.0's exit criteria include a tested upgrade path.
2. **What holds state** is listed once (§3): the bucket (immutable files plus pointer files: collections, the log, Graph snapshots, Git); the metastore (embedded Raft or TiKV); Live on TiKV and SQL on TiDB; Postgres (Neon's pageserver and safekeepers); House (the chDB data directory); Durable (SQLite); `.env` and the compose volumes.
3. **Rule 1, versioned formats.** Every persisted format carries a version. Readers support N and N−1. The engine refuses to start on data it cannot read.
4. **Rule 2, one step at a time.** Upgrades go N → N+1 only, with no skipping. From v0.1.0, any 0.1.x → 0.1.y.
5. **Rule 3, a `migrate` service.** A one-shot compose service runs before the engine. It reads the stored versions, runs idempotent, resumable migrations, and records each step in a migration ledger in the `_system` namespace. A failed migration stops the upgrade with the old data intact.
6. **Rule 4, snapshot first:** the bucket's pointer files plus a metastore backup; TiKV through BR; copies of SQLite and the chDB directory; a Postgres branch.
7. **Rule 5, rollback** restores the snapshot. Each migration is marked reversible or "point of no return", and the release notes say which.
8. **Rule 6, third-party components.** TiKV, TiDB, Loams Postgres (Neon), chDB and Grafeo stay pinned, and are bumped only with a CI-tested upgrade path, never downgrading across their majors.
9. **The CLI:** `loams upgrade check` reports the current and target versions, the migrations and whether each is reversible. `loams upgrade --to <version>` snapshots, pulls the images, runs `migrate`, restarts in order (TiKV/PD → TiDB → engine → fabric/House → the rest) and health-checks. `loams upgrade rollback` restores the snapshot.
10. **Clients.** The hosted console handles engine N−1: the engine reports its version and capabilities, and the console hides features the engine lacks. SDKs stay additive-only (`buf breaking` in CI). The desktop updates through the `desktop-latest` feed, and its bundled engine runs the same `migrate` step.
11. **The release gate.** A new CI job in the full tier (pull requests into `main`) starts the stack at the previous release tag, writes fixture data for every product, upgrades to the new images and verifies every read and write, then rolls back and verifies again. A release cannot merge to `main` without it.

### 1.2 Decisions

| # | Decision | Status | § |
|---|---|---|---|
| D923 | **Data-keeping upgrades start at v0.1.0-preview.3.** A stack at preview.2 upgrades to preview.3 in place; a stack at preview.1 cannot, and starts fresh. Every later release keeps data | Approved (owner) | §2 |
| D924 | **Every persisted format carries a version, and readers support N and N−1** (§10 §7, §03 §6, extended to every store in §3). Loams-owned formats are listed in one registry, owned by `loams-common` (`loams_common::formats`) and re-exported by `loams-migrate`, so the crates that own formats use it without a dependency cycle. **The engine refuses to start** (exit 78, `format_unreadable`) on data whose version it cannot read, or while the ledger shows a migration not `done` | Approved (owner); registry, exit code proposed | §4 |
| D925 | **One step at a time.** A *step* is one release to the next in the release train. Each preview is a step. From v0.1.0 a minor line is one step, and adjacency is **line-based**: a 0.1.y release declares `from = "0.1"`, so any earlier 0.1.x (x < y) is an accepted source, and 0.2.0 declares `from` = the last 0.1 patch. Before v0.1.0 adjacency is **exact-predecessor**. `migrate` refuses any other source | Approved (owner); the step definition proposed | §5 |
| D926 | **preview.2 data is adopted by fingerprint.** preview.2 writes no stack version and no ledger. preview.3's `migrate` recognises a preview.2 stack from what is on disk plus a positive preview.2 marker (a data marker preview.1 never writes, or the stack's declared release, §5.3), writes the baseline ledger entry `0000-adopt-preview-2`, and refuses anything it does not recognise | Proposed | §5.3 |
| D927 | **`migrate` is a one-shot service of the `loams` image** (`loams migrate run`), not a separate image. The engine, the House and `pg-control` start only after it exits 0 (`depends_on: … condition: service_completed_successfully`). On an ordinary restart it finds nothing to do and exits in about a second (target) | Approved (owner); same-image proposed | §6 |
| D928 | **The migration ledger lives in the `_system` namespace of the metastore**: one record per applied migration (`id`, `from`, `to`, `state`, `cursor`, `reversible`, digests, times), the stack record (`_system/upgrade/stack`: release, per-format versions), and a fenced migration lease. A copy of the ledger is written into each snapshot's manifest | Approved (owner); record shape proposed | §6.2 |
| D929 | **The migration contract.** A migration names its store, its source and target format versions and its class (`reversible` or `point_of_no_return`). It is idempotent and resumable from a cursor checkpointed in the ledger. It writes beside the old data (expand) and switches with one atomic step at the end (a pointer CAS, a metastore transaction, a file rename). Migrations are compiled into the binary as a static, ordered list; nothing is downloaded at upgrade time | Proposed | §6.3 |
| D930 | **Snapshot first, with writers stopped.** The snapshot is taken after the stack is quiesced and before any migration: the bucket's pointer files and a metastore backup; TiKV through BR (cold volume copy where BR cannot cover a keyspace, Q821); SQLite through the backup API; the chDB directory as a copy; a branch of every Postgres timeline; `pg-control`'s records; the host files (`.env`, `secrets/`, the compose bundle). A snapshot pins every immutable object its pointers reach as a **GC root** until it is pruned | Approved (owner); method per store proposed | §7 |
| D931 | **Where snapshots live and how long.** In the `upgrade-snapshots` volume (or `LOAMS_UPGRADE_SNAPSHOT_DIR`), with a `snapshot.json` manifest, mode 0700. The most recent snapshot is kept until the next successful upgrade, or until `loams upgrade snapshots prune` (Q822). `upgrade check` estimates the size, and `upgrade` refuses to start with less than 1.5× the estimate free (target) | Proposed | §7.3 |
| D932 | **Rollback restores the snapshot.** `loams upgrade rollback` restores every component from the snapshot and starts the previous release; writes made after the upgrade are lost, and the CLI says so with the time window. **Reversible** means the previous release can open the data after the migration (the gate proves it by starting N's images on N+1's migrated data); **point of no return** means it cannot. Only when every applied migration is reversible may `rollback --keep-writes` swap the images back without restoring (Q824). The release notes list each migration's class | Approved (owner); `--keep-writes` proposed | §8 |
| D933 | **Third-party components stay pinned** (TiKV and PD, TiDB, Loams Postgres, chDB, Grafeo, and the formats they own). A bump lands only with a green upgrade gate from the previous pins; their own upgrade order holds (PD → TiKV → TiDB); a downgrade across a component's major is refused (§41 §8.2's `noDowngrade`); a Postgres **major** never changes through `loams upgrade` (§51 §9) | Approved (owner); details proposed | §9 |
| D934 | **`loams upgrade` is a client group in `loams-cli`** that drives a compose stack from its directory: `check`, `--to <version>`, `rollback`, `status` and `snapshots list\|prune`. It is CL1's second exception to "products are reached through their public API only", beside the stack supervisor. All data-touching work runs inside containers of the target image (`loams migrate …`); the CLI never links a storage crate | Approved (owner); `status`, `snapshots` and the exception proposed | §10 |
| D935 | **The upgrade sequence.** Preflight → fetch and verify the bundle → pull images by digest → **close the door** (stop the Tailscale sidecars) → quiesce → snapshot → third-party restarts in their order → `migrate` → ordered start (TiKV/PD → TiDB → engine → fabric/House → the rest) → verify → **open the door**. A failed verification rolls back automatically while the door is still closed, so no client write is lost (Q823) | Approved (owner) order; door and automatic rollback proposed | §10.3 |
| D936 | **The upgrade path is data in the release.** `release/upgrade-path.toml` names each release, its predecessor, its migrations with their class, and its third-party pins. The release workflow copies it into the signed release manifest and the compose bundle (`loams-compose-<version>.tar.gz`, which `release-github.yml` already publishes); the CLI reads only the signed copy | Proposed | §11 |
| D937 | **Local stacks run the same migrations.** `loams stack restart --upgrade` (§30 §8.3) runs `loams migrate run` against the stack's data directory before it starts the new binary, with a local snapshot of the stack's pointer files, SQLite and metastore | Proposed | §12 |
| D938 | **The hosted console supports engine N and N−1.** It reads `GetInstance` (`server_version`, `features`, `services`, `api_versions`) and hides what the engine does not serve. An engine older than N−1 gets a blocking "upgrade your engine" page with the `loams upgrade` command. The engine fills `min_app_versions` from the release manifest | Approved (owner); the N−2 page proposed | §13.1 |
| D939 | **SDKs and protos are additive-only.** `buf breaking` runs against the **last release tag** (not only `origin/main`) for every package that is not `unstable`; an SDK minor never removes or renames a public symbol (the SDK fixtures check it) | Approved (owner) | §13.2 |
| D940 | **The desktop runs the same `migrate`.** After an update from the `desktop-latest` feed, the daemon (or Electron, before DD1) runs the new bundled `loams migrate run` on the engine's data directory, with a snapshot, before it starts the engine. A bundled engine older than the data's ledger (a downgrade) starts only if every newer migration is reversible; otherwise the app offers the snapshot restore. **This amends §50 §5.3 step 5** | Approved (owner); the downgrade rule proposed | §13.3 |
| D941 | **The release gate is `Upgrade: previous release → candidate`** in `ci.yml`'s full tier, part of `CI required` on pull requests into `main`. It starts the stack at the previous release tag, writes fixtures for every product, upgrades to the candidate images and verifies every read and write, then rolls back and verifies again. It also runs nightly on `dev` | Approved (owner); job shape proposed | §14 |
| D942 | **Release notes carry a generated "Upgrading" section** from `release/upgrade-path.toml`: the predecessor, each migration with its class, third-party bumps, the snapshot estimate and the expected downtime. A repository check fails a release pull request whose CHANGELOG entry lacks it | Proposed | §11.3 |
| D943 | **Clusters and Kubernetes use the same migrations later.** Under the Helm chart and §41's rings, `migrate` runs as a pre-sync Job in the wave before the engine, and §41 §8.2's expand-then-contract rule decides when a point-of-no-return migration may ship. Not built in UP1 | Proposed | §12.3 |
| D944 | **Upgrade security.** Images are pulled by the digests in the signed manifest; snapshots hold secrets and stay on the host with mode 0700, never uploaded; the stack has no client while the door is closed; `migrate` runs with the engine's own credentials and no new ones | Proposed | §16 |
| D945 | **Postgres WAL is not lost across an upgrade.** preview.2's `safekeeper1` keeps its WAL in the container filesystem. Before any step recreates it, the upgrade stops the computes and waits until the pageserver has ingested and uploaded everything the safekeeper holds; preview.3 gives the safekeeper a volume | Proposed | §7.2, §9.3 |

### 1.3 What it looks like

```
 loams upgrade --to 0.1.0-preview.3                       (on the stack's host; BYOC over SSH)
   │ 1 preflight: current release, path, free space, health      ── loams upgrade check
   │ 2 fetch the signed bundle, pull images by digest            (the stack is still serving)
   │ 3 close the door: stop tailscale, house-tailscale           (no client can write)
   │ 4 quiesce: stop engine, House, computes, pg-control
   │ 5 snapshot ─► upgrade-snapshots/<id>/                       loams migrate snapshot (target image)
   │      bucket pointers + GC root · metastore · TiKV (BR) · SQLite · chDB dir
   │      Postgres branches · pg-control records · .env, secrets/, the old bundle
   │ 6 third-party restarts in their order: PD → TiKV (→ TiDB later)
   │ 7 migrate (one-shot) ─► reads _system/upgrade/stack, runs migrations, writes the ledger
   │ 8 ordered start: TiDB → loams → house → Postgres stack, pg-control → agentd, console
   │ 9 verify: versions, health, read probes per product
   │10 open the door                       ── on failure at 6–9: automatic rollback (door still closed)
   ▼
 loams upgrade rollback ─► stop all ─► restore every component from the snapshot ─► start the previous release
```

## 2. Scope

**In scope:**
- the self-hosted compose stack (`deploy/release`, locally or on a VM through BYOC);
- the CLI's local stacks (§30 §8);
- the desktop's bundled engine and its stacks (§50);
- the console's and SDKs' compatibility with an engine one release behind;
- the release gate.

**Out of scope:** zero-downtime rolling upgrades of multi-node clusters (§10 §7's M2 gate, D943); Kubernetes and §41's rings (D943); Postgres major versions (`UpgradeProject`, §51 §9); application-level data migrations inside a Live app (§45 §4.3: user mutations, unchanged); Loams Cloud.

**Downtime.** A compose upgrade is an offline upgrade: the door is closed from quiesce to verification (§10.3). The target for a single-node stack with under 10 GiB outside the bucket, excluding image pulls, is **5 minutes** (target; Q833).

## 3. What holds state

Each row is a store that survives `docker compose down` (without `-v`). The *version today* column is what preview.2 writes; §17 has the as-built detail.

| # | Store | Where (compose) | Format owner | Version carried | Snapshot (Rule 4) | Restore (Rule 5) |
|---|---|---|---|---|---|---|
| S1 | **Bucket, immutable objects**: WAL objects, segments, Lance files, Tantivy splits, collection manifests, delete bitmaps, PK deltas, Graph change sets and snapshots, Git packs | `loams-data` volume (`file:///var/lib/loams/bucket`), or the user's S3 bucket | Loams | `magic ‖ u16 format_version` on every Loams envelope (§03 §3.4); Lance 2.1 and Tantivy pinned (§03 §6) | Not copied: immutable. Pinned as a GC root by the snapshot manifest | Nothing to restore; the pointers decide what is live |
| S2 | **Pointers**: the heads of collections, links, graph manifests and the graph catalog (metastore records, §17), and the bucket's mutable objects (Git's catalog documents `_names/`, `_deleting/` and `<root>/head` hints) | metastore; `loams-data` | Loams | per object (§17) | The records with S3; the bucket objects copied into the snapshot (small) | Copied back |
| S3 | **Metastore** | TiKV keyspace `loams_meta` (compose), or embedded Raft (`meta.redb` plus snapshots in the bucket) for local stacks and the desktop | Loams | TiKV: `FORMAT = 1` per record; Raft: snapshot envelope 5–8, unversioned log entries (§17); the stack record (new, D928) | TiKV: with S4 (BR). Raft: a forced Raft snapshot plus a copy of the redb file | TiKV: with S4. Raft: replace the redb file and snapshot pointer |
| S4 | **TiKV and PD**: Live (`loams_live_dev`), the metastore, TiDB's keyspaces | `tikv-data`, `pd-data` | PingCAP | TiKV's own (pinned v8.5.8) | BR backup, or a cold copy of both volumes (Q821) | BR restore into a fresh cluster at the snapshot's TiKV version, or the volumes copied back |
| S5 | **Live** on TiKV | in S4 | Loams | a protobuf `format = 1` in each record; `JournalEntry` unversioned (§17) | with S4 | with S4 |
| S6 | **Loams SQL** on TiDB | in S4 (TiDB is stateless) | PingCAP (patched) | TiDB's bootstrap version per keyspace (§47 §5) | with S4 | with S4 |
| S7 | **Postgres storage**: the pageserver and its remote storage | `pageserver-data`, `rustfs-data` (project `loams-neon`) | Loams Postgres (the fork) | the pageserver's `index_part` and layer formats (the fork's own) | A branch of every timeline at its last LSN, through `pg-control` (`CreateBranch`) or, without it, the pageserver API | `RestoreBranch` (pg-control), or without it the computes pointed back at the branch (verify the pageserver API in UP1 Task 0) |
| S8 | **Postgres safekeepers** | the container filesystem of `safekeeper1` in preview.2 (no volume); a volume from preview.3 | Loams Postgres | the fork's own | None needed once drained (D945) | — |
| S9 | **`pg-control`'s records** (`--store=local`), and `pg-runner`'s compose state files | `pg-control-data` and `pg-runner-data` (profile `postgres-control`, already in preview.2; UP1 ruling T0-9) | Loams | its record encoding (§17) | File copy | File copy back |
| S10 | **House**: the data directory (`spool/`, `catalog.sqlite`, `bucket/`, `workers/` with chDB's paths), `house.key` | `house-data` | ClickHouse (chDB) plus Loams' catalog | none today; UP1 adds a `FORMAT` file and `catalog.sqlite`'s `user_version` (§17) | Copy of `/var/lib/loams-house` with the House stopped | Copy back |
| S11 | **Durable** (Resonate) on SQLite | `loams-data` (`/var/lib/loams/…`) | Resonate's schema (`sqlx` migrations) | `_sqlx_migrations` (§17). Today SQLite migrates implicitly when the engine opens it; UP1 moves that into `migrate` (§6.1) | SQLite online backup API into the snapshot | File copy back |
| S12 | **Graph local data**: Grafeo's working files | `loams-data` (`/var/lib/loams/graph`) | Grafeo | Grafeo's file format | Not copied: a cache rebuilt from the bucket (§48 §14.4) | Deleted; rebuilt on open |
| S13 | **Host files**: `.env`, `secrets/` (API keys, `pg-api-keys.json`, `engine.env`), the compose bundle, local overrides | the stack directory | the user and Loams | the bundle's version (`LOAMS_VERSION`) | Copied into the snapshot | Copied back |
| S14 | **Tailnet identity**: `tailscale-state`, `house-tailscale-state` | volumes | Tailscale | Tailscale's own | Not touched by any migration; not copied | — |
| S15 | **agentd** (`--profile agentd`) | `agentd-data` | Loams | the daemon's store (§50) | Copy | Copy back |

**What "the bucket's pointer files" are in the code.** The owner's direction names pointer files for collections, the log, Graph snapshots and Git. In the code, the heads of collections, links, graph manifests and the graph catalog are **metastore records** (`collection/<cid>`, `link/<id>`, `graph-manifest/<gr>`, `graph-catalog`), each holding an object path with a CAS version (§03 §3.3, §17); the log's segment index is in the metastore too; Git keeps its catalog documents and head hints as mutable bucket objects. So "the bucket's pointer files plus a metastore backup" is, concretely, the metastore backup (which holds most pointers) plus a copy of Git's mutable objects. With writers stopped, that captures a consistent view of S1–S3.

## 4. Rule 1: versioned formats (D924)

### 4.1 Every format has a version

- **Loams-owned formats** keep §03 §3.4's envelope (`magic ‖ u16 LE format_version ‖ body ‖ crc32c`) and §10 §7's rule. Formats without the envelope get a version in their own encoding: metastore records through a versioned value prefix (as §10 §7 says of item formats), SQLite through `PRAGMA user_version`, JSON documents through a `"v"` field, directories through a `FORMAT` file.
- **One registry, one owner.** `loams_common::formats` is the only definition. `loams-migrate` re-exports it and never defines a format of its own; every crate that owns a format depends on `loams-common` (which it already does), never on `loams-migrate`. It lists every Loams format as `FormatSpec { id, store, current: u16, min_read: u16 }`, with `min_read = current − 1` unless a release says otherwise. Each crate that owns a format asserts its constant against the registry in a unit test (`format_constants_match_registry`), so a format bump that forgets the registry fails CI.
- **Third-party formats** (TiKV, TiDB's bootstrap, the pageserver, chDB, Grafeo, Lance, Tantivy, SQLite itself) are versioned by their pins (§9), not by Loams.
- **Readers check a range, not an equality.** Today almost every reader accepts only its exact current version (§17); only Git's segment and checkpoint readers and the metastore's snapshot decoder accept older versions. Every reader moves to one helper, `formats::check(id, found)`, which accepts `min_read..=current`. While every format is at version 1 this changes no behaviour, and it is what lets release N+1 read release N's data.
- **Unversioned formats get a version** before they first change: the metastore's Raft log entries and `meta.redb` records (bare postcard today), pointer values, the graph catalog document, Live's `JournalEntry`, Git's `.lpk` footer, the House data directory and `catalog.sqlite`. "No version" reads as version 1, so preview.2's data needs no rewrite.

### 4.2 The stack record

`_system/upgrade/stack` records the release the data was last migrated to and, per format, the version currently **enabled for writing**:

```
StackRecord { release: "0.1.0-preview.3", formats: { "collection.manifest": 1, "live.keys": 2, … },
              migrated_at, migrated_by: { image_digest, host } }
```

A release can read `min_read..=current` and writes the enabled version. On a single-node compose stack every process is upgraded together, so `migrate` enables new write formats in the same step. On a cluster (D943), §10 §7's rule holds: a new format is enabled only after every node runs a release that reads it.

### 4.3 Refusing to start

At startup, before any listener opens, the engine (and `loams-fabric house`, `pg-control`, the desktop's engine) reads the stack record and the ledger:
- a format whose enabled version is outside `min_read..=current` → exit **78** (`EX_CONFIG`), log `format_unreadable` with the format id, the stored version and the readable range, and the hint `run loams upgrade` (newer data) or `restore the snapshot or upgrade this binary` (older binary);
- a ledger entry in `running` or `failed` → exit 78, `migration_incomplete`, naming the migration;
- no stack record and a non-empty store that is not a preview.2 fingerprint → exit 78, `stack_unrecognised`.

An empty store is a new stack: the engine writes the stack record at the current release and an empty ledger.

## 5. Rule 2: one step at a time (D925, D926)

### 5.1 The release train

`release/upgrade-path.toml` (D936) lists every release with its predecessor:

```toml
[[release]]
version = "0.1.0-preview.3"
from    = "0.1.0-preview.2"
migrations = ["0000-adopt-preview-2", "0001-safekeeper-volume", …]

[[release]]
version = "0.1.0-preview.2"
from    = "fresh"          # preview.1 → preview.2 keeps no data
```

`migrate` refuses (`upgrade_step_not_adjacent`) a stored release that is not the target's `from`, and names the chain. `loams upgrade --to` a release more than one step ahead walks the chain one step at a time, each step a complete upgrade with its own snapshot and verification, stopping at the first failure (Q825). Nothing skips a step: every migration in between runs, in order, under the binary that wrote it.

### 5.2 After v0.1.0

From v0.1.0 a minor line is one step, and adjacency is line-based. A patch release 0.1.y declares `from = "0.1"` in `release/upgrade-path.toml`; `migrate` accepts a stored release 0.1.x with x < y (and refuses x > y, another line, or a pre-release of the line). Patch releases carry migrations of their own, and the 0.1.y binary carries every migration of the line; the ledger says which are `Done`, so 0.1.x → 0.1.y runs exactly the missing ones, in order, in one `migrate`. Previews before v0.1.0 use exact-predecessor adjacency (§5.1). The first minor of a new line, 0.2.0, declares `from` = the last 0.1 patch, exactly. A point-of-no-return migration may ship in a patch release only with the owner's sign-off in the release pull request (D942's section names it). The next minor's `from` is the last patch of the previous line.

### 5.3 preview.2 has no version stamp (D926)

preview.2 is released without a stack record or ledger (its release pull request, #518, is open at the time of writing; Q820 asks whether to add a stamp to it). preview.3's `migrate` therefore **adopts** a preview.2 stack by fingerprint, as migration `0000-adopt-preview-2`:
- the metastore is TiKV keyspace `loams_meta` (or a `meta.redb` with an `OPNMETA` snapshot of version 5–8) whose records carry `FORMAT = 1` and decode with preview.2's encodings (a sample of each key tag);
- every pointer's target object opens at the version preview.2 writes (`OPCM` 1, graph `MANIFEST_FORMAT` 1, Git `DOC_VERSION` 1);
- Live's keyspace `loams_live_dev` decodes, with records at `format = 1`;
- the Durable SQLite database's `_sqlx_migrations` lists exactly Resonate's `0001_initial`;
- `house-data`, if present, has `catalog.sqlite` with the `namespaces` and `tables` tables and no `FORMAT` file;
- `pg-control.redb`, if present, has records at `FORMAT = 1`.

A store that matches writes the stack record at `0.1.0-preview.2` and the ledger entry; anything else stops with `stack_unrecognised` and the hint to start a fresh stack. **preview.1 is harder to tell apart**, because every format is still at version 1 in both. UP1 Task 0 looks for data only preview.2 writes (for example graph streams named `_graph.<gr>` after `ensure_graph_stream`, or Live deployment records from LV1 Tasks 8–10) and, if it finds a reliable marker, the fingerprint refuses preview.1 stacks (D923). Adoption **always requires a positive preview.2 marker**, never only "everything decodes": either a data marker Task 0 proves preview.1 never writes, or the stack's declared release (the CLI passes `--declared-release` from the stack directory's `LOAMS_VERSION=0.1.0-preview.2`; the desktop passes its `.loams-stack-version`). With neither, `migrate` refuses with `stack_unrecognised`, so a preview.1 stack is never adopted by accident (D923).

## 6. Rule 3: the `migrate` service (D927–D929)

### 6.1 In the compose stack

```yaml
  migrate:
    image: ${LOAMS_REGISTRY:-ghcr.io/ostrium-labs}/loams:${LOAMS_VERSION}
    command: [migrate, run, --meta=tikv://pd:2379/loams_meta, --live-store=tikv://pd:2379/loams_live_dev,
              --bucket=${LOAMS_BUCKET:-file:///var/lib/loams/bucket}, --data-dir=/var/lib/loams,
              --house-dir=/var/lib/loams-house, --pg-control-dir=/var/lib/loams-pg-control]
    restart: "no"
    volumes: [loams-data:/var/lib/loams, house-data:/var/lib/loams-house, pg-control-data:/var/lib/loams-pg-control]
    depends_on: { pd: {condition: service_healthy}, tikv: {condition: service_healthy} }
  loams:
    depends_on: { migrate: {condition: service_completed_successfully}, … }
```

`house` and `pg-control` gain the same `depends_on`. `migrate` has no network listener, no published port and no Tailscale sidecar. It mounts the Loams-owned volumes (S1–S3, S9–S11); a volume of a profile that never ran is empty, and `migrate` treats an empty store as new. Third-party stores (S4's internals, S6, S7, S8) are upgraded by their own binaries under Rule 6; `migrate` only checks their versions.

`loams migrate` (a server command of the `loams` binary, beside `durable migrate`) has these subcommands: `run`, `plan` (what `run` would do, as JSON; `loams upgrade check` reads it), `status` (the ledger), `snapshot` and `restore` (§7, §8), `verify` (the read probes of §10.4). `loams durable migrate` stays, and becomes one migration step of `run` (Resonate's own migrations, S11). The engine then opens SQLite with Resonate's `migrate = false` and checks the schema, as it already does for MySQL stores (`mysql::check_schema`), so a newer Resonate never migrates a database implicitly at serve time.

### 6.2 The ledger (D928)

Records in the `_system` namespace of the metastore (the TiKV backend or embedded Raft, through the `MetaStore` trait). No `_system` namespace exists yet (§17); UP1 reserves the name, so a user cannot create it and no API lists it:

| Key | Value |
|---|---|
| `_system/upgrade/stack` | `StackRecord` (§4.2) |
| `_system/upgrade/lease` | the migration lease: holder, epoch, expiry (30 s, renewed every 10 s). Fenced: every ledger write checks the epoch |
| `_system/upgrade/ledger/<seq:08>-<migration id>` | `LedgerEntry { id, release_from, release_to, store, class: Reversible\|PointOfNoReturn, state: Pending\|Running\|Done\|Failed, cursor: Option<bytes>, attempts, started_at, finished_at, error: Option<{code, message}>, image_digest, code_digest }` |
| `_system/upgrade/snapshots/<snapshot id>` | the snapshot's manifest digest and location (§7.3) |

The ledger is append-only except for a migration's own entry while it runs. `code_digest` is a hash of the migration's identity (id, version pair, class) compiled into the binary, so a binary whose migration of the same id differs refuses to resume someone else's run (`migration_mismatch`).

### 6.3 The migration contract (D929)

```rust
pub trait Migration: Send + Sync {
    fn id(&self) -> &'static str;                   // "0003-live-index-keys-v2"
    fn store(&self) -> Store;                       // Bucket | Meta | Live | Durable | House | PgControl | Host
    fn formats(&self) -> &'static [(FormatId, u16, u16)];   // (format, from, to)
    fn class(&self) -> Class;                       // Reversible | PointOfNoReturn
    fn estimate(&self, cx: &Cx) -> Result<Estimate>; // rows, bytes, duration (estimate)
    async fn apply(&self, cx: &mut Cx, cursor: Option<Cursor>) -> Result<Step>; // Step::Continue(cursor) | Step::Done
    async fn verify(&self, cx: &Cx) -> Result<()>;
}
```

Rules every migration follows (each one is a named test in UP1 Task 3):
- **Idempotent.** Running a finished migration again changes nothing (`rerun_done_migration_is_noop`).
- **Resumable.** `apply` returns after bounded work with a cursor, and the runner checkpoints it in the ledger before the next call. A crash at any point resumes from the last checkpoint and converges to the same result (`crash_at_every_failpoint_converges`).
- **Expand, then switch.** New data is written beside the old (new keys, new objects, a new file). The switch is one atomic step at the end: a pointer CAS, a metastore transaction, a `rename`. Until the switch, the old release still reads everything; that is what keeps "the old data intact" on failure.
- **No network beyond the stores.** A migration never calls a service API (TiDB, `pg-control`, the House) and never downloads anything.
- **Bounded.** `estimate` is required; `upgrade check` shows it.
- **Classified.** A migration is `Reversible` only if the previous release opens the data after it; the gate proves the claim (§14).

### 6.4 Failure

A migration that returns an error marks its entry `Failed` with the error, releases the lease and exits non-zero. Compose then does not start the engine (its `depends_on` is unmet). The data is the old data plus whatever the failed migration wrote beside it, never switched, and the snapshot is intact. `loams upgrade` reports the failure and rolls back automatically (D935). Running `migrate run` again retries the failed migration from its cursor; `loams migrate run --abandon <id>` discards a failed migration's partial writes (its `abandon` step) before a rollback.

## 7. Rule 4: snapshot first (D930, D931, D945)

### 7.1 Quiesce, then copy

The snapshot is taken with every writer stopped: the engine, the House, `pg-control`, `pg-runner`'s computes, agentd. PD, TiKV and the Postgres storage stay up where the method needs them. The snapshot runs as `loams migrate snapshot` in a container of the **target** image (it can read the old formats, by Rule 1), so the old release needs no snapshot code; preview.2 has none.

### 7.2 Per store

| Store | Method | Consistency |
|---|---|---|
| S2, S3 (Raft) | Pointer records go with the metastore (TiKV below; for embedded Raft, force a Raft snapshot and copy it and `meta.redb`). Copy Git's mutable bucket objects (`_names/`, `_deleting/`, `<root>/head`) into `<snapshot>/bucket-pointers/` | Writers stopped |
| S1 | Record the snapshot as a GC root in the metastore (`_system/upgrade/snapshots/<id>`), listing the pointer copies; GC (§03 §7) treats every object reachable from a root as live until `snapshots prune` removes it | GC reads roots before deleting |
| S3–S6 (TiKV) | BR backup of the cluster to `<snapshot>/tikv/` (the snapshot volume is mounted into `tikv`, as BR's `local://` storage needs; verify BR v8.5.8 with API V2 keyspaces), or a cold copy of `pd-data` and `tikv-data` with PD and TiKV stopped (Q821) | BR's backup TS; or both stopped |
| S7 | For every timeline: create a branch named `upgrade-<snapshot id>` at its last record LSN, through `pg-control` when it runs (`CreateBranch`, TTL = the snapshot's life), else the pageserver's timeline-create API with an ancestor. When the release changes the Loams Postgres pins, also a cold copy of `pageserver-data` and `rustfs-data` with the Postgres stack stopped (Q829) | Computes stopped; the branch LSN is the last record LSN |
| S8 | Not copied. **Drain first (D945):** with computes stopped, lower each tenant's `checkpoint_timeout` (the pageserver's checkpoint API is testing-only in the pinned image, UP1 ruling T0-9), wait until each timeline's `remote_consistent_lsn` reaches the safekeeper's `flush_lsn` (pageserver and safekeeper status APIs), with a timeout (5 min, target), then restore the setting. Refuse to recreate `safekeeper1` until then | Drained |
| S9 | Copy the record file | `pg-control` stopped |
| S10 | Copy `/var/lib/loams-house` | House stopped |
| S11 | SQLite online backup API (`rusqlite::backup`) into the snapshot | Engine stopped |
| S13 | The CLI copies `.env`, `secrets/`, the compose bundle and any `compose.override.yaml` into `<snapshot>/host/` on the host | — |
| S15 | Copy | agentd stopped |

### 7.3 The snapshot manifest and its location (D931)

`snapshot.json`: `{ id (ULID), created_at, release, target_release, components: [{ store, method, path, bytes, sha256 }], ledger: [...copy...], stack_record }`. The snapshot is complete only when `snapshot.json` is written last and its digest is recorded in the metastore; `restore` refuses an incomplete snapshot (`snapshot_incomplete`).

Location: the `upgrade-snapshots` volume, mounted into `migrate`, `tikv` (for BR) and the CLI's helper containers. With an `s3://` bucket, the bucket-pointer copies also go to `<bucket>/_system/upgrade/snapshots/<id>/`. Retention: the latest snapshot is kept until the next upgrade has verified, then pruned; `loams upgrade snapshots prune` removes it earlier (and its GC root); a chain (§5.1) keeps the first step's snapshot until the whole chain verifies.

## 8. Rule 5: rollback (D932)

- **`loams upgrade rollback`** (the default and always available while a snapshot exists): close the door, stop everything, restore every component of the snapshot (§3's restore column), bring back the old bundle and `.env`, start the previous release, verify, open the door. Writes made since the upgrade are lost; before asking for confirmation the CLI prints the window (`writes between 10:02 and 10:41 UTC will be lost`).
- **Reversible and point of no return.** A migration is `Reversible` when the previous release reads its result **and everything the new release writes on top of it**: it only added keys, objects or optional fields the old reader ignores, or it wrote a new format version the old release already reads, and it enabled no write format that N−1 cannot read. It is `PointOfNoReturn` when the previous release cannot open the data afterwards: a format enabled that N−1 does not read, a contract step (a key removed, a field made required), or a third-party bump that refuses downgrades. The class is declared in code and **proved by the gate**: the gate starts the previous release on the migrated data, after the candidate has written new data of every kind, and reads both the fixtures and those new writes (§14), before it restores the snapshot. A wrong `Reversible` claim fails the release.
- **`rollback --keep-writes`** (Q824): only when every migration applied since the snapshot is `Reversible` and no third-party pin changed. It swaps the images back and keeps the data, so writes made after the upgrade survive. Otherwise it exits 6 (`rollback_needs_snapshot`).
- **After a rollback** the restored metastore holds the pre-upgrade ledger. The CLI appends the attempt (target, failure, time) to `<stack dir>/.loams-upgrade/history.jsonl` on the host so the failed attempt is not forgotten.
- **Third-party downgrades.** Restoring across a TiKV, PD or TiDB pin change restores into a fresh cluster at the snapshot's version (BR restore, or the volumes copied back); no component is started at an older version on newer files (§41 §8.2's `noDowngrade`).

## 9. Rule 6: third-party components (D933, D945)

### 9.1 The pins

| Component | Pin today | Where | Its own upgrade rules |
|---|---|---|---|
| PD, TiKV | `v8.5.8` | `deploy/release/.env.example` (`TIKV_TAG`), `release/companions.toml` (CL1) | PingCAP's order: PD, then TiKV, then TiDB; no downgrade across minors once upgraded (verify for v8.5) |
| TiDB | `pingcap/tidb:v8.5.8` (official, D2450) | `release/sqldb-images.toml`, `TIDB_IMAGE` | Bootstrap and upgrade DDL per keyspace on start or in `CreateDatabase` (§47 §5; verify which for a version bump) |
| Loams Postgres storage and computes | digests in `deploy/pins/loams-postgres-images.toml` | `deploy/loams-postgres-dev/compose.yaml` | The fork's release rules (D810, D811); a Postgres major only through `UpgradeProject` (§51 §9) |
| chDB | the `chdb-core` pin (§49 §19, D779) | fabric's manifest | Stable releases only, ≥ 14 days old; storage is Iceberg in production, so an engine bump migrates no data there; the preview's local data directory is snapshotted |
| Grafeo | `ostrium-labs/grafeo` `v0.5.43-loams.1` | the workspace `Cargo.toml` | A file-format change rebuilds local files from the portable snapshot on open (§48 §14.4) |
| Lance, Tantivy | Lance file format 2.1; the pinned Tantivy fork | the workspace | A change is a Loams format change under Rule 1 (§03 §6) |
| Tailscale, Headscale, RustFS, nginx | tags in `.env.example` and compose | — | Stateless for Loams, or state that no migration touches (S14) |

### 9.2 The bump policy

1. A pin changes in its own pull request into `dev`, which names the previous and new versions and the component's upgrade notes (read from the primary source).
2. The upgrade gate (§14) runs on that pull request by label (`ci:upgrade`), from the previous release's pins to the new ones, and again on the release pull request.
3. A bump across a component's major needs a migration entry in `release/upgrade-path.toml` with class `PointOfNoReturn` (its files cannot go back) and a release-notes line.
4. Never downgrade: `loams upgrade` refuses a target whose pin of any component is lower than the stack's (`component_downgrade`), and so does rollback without a snapshot.
5. Restart order for third-party components follows their vendor: PD → TiKV → TiDB, then the Loams processes (§10.3).

### 9.3 Postgres specifics (D945)

- `loams upgrade` never changes a Postgres **major**: a compute image of another major is refused by `compute_major_mismatch_refused` (§51 §9), and the pin file's `default` major must not change in a step.
- preview.2's safekeeper has no volume (§17). Migration `0001-safekeeper-volume` gives it one; the drain of §7.2 runs first, so recreating the container loses no WAL. The migration is `Reversible`: preview.2's compose file ignores the volume, and its safekeeper starts empty on drained timelines.
- With `--profile postgres-control` (new in preview.3), branches and restores go through `pg-control`. Without it, the CLI uses the pageserver's management API on the compose network, from a helper container.

## 10. The CLI (D934, D935)

### 10.1 Where it fits

`upgrade` is a client group in `loams-cli` (CL1 Task 1's crate), beside `stack`, `self-update` and the product groups. It manages a **compose stack**, which §30's `stack` group does not: a compose stack is a directory with `compose.yaml` (`name: loams`) and `.env`. The stack directory is `--dir`, else `LOAMS_STACK_DIR`, else the current directory if it holds such a `compose.yaml`. `self-update` updates the CLI binary; `upgrade` updates a stack; `stack restart --upgrade` updates a local stack (§12).

`--bundle DIR` takes a local bundle instead of the signed download, with its images already present locally; the CLI says the bundle is unverified. CI (§14) and git checkouts at the target's tag (Q827) use it.

CL1's global constraint "products are reached through their public API only" gets a second named exception: `loams upgrade` reads the stack directory and drives `docker compose` (or `podman compose`), as the stack supervisor reads its stack. Every step that touches data runs inside a container of the target `loams` image (`loams migrate …`), so `loams-cli` keeps its no-server-dependency guard.

**The helper-container boundary.** The exception covers exactly two helper images, both pinned by digest in the signed manifest: the target `loams` image (`loams migrate snapshot|restore|run|plan|verify`, and the cold volume copy) and, where Q821 rules it in, BR (`pingcap/br`). Every helper runs with `cap_drop: ALL`, `no-new-privileges`, a read-only root filesystem, no published port, no container runtime socket and no host bind mount except the snapshot directory; it joins only the stack's compose network (to reach PD, TiKV, the pageserver and `pg-control` on it), never the `house` or `pg-runner` networks and never a host network. Its volumes are only the data volumes of the step it performs and `upgrade-snapshots`. Its only credentials are the ones the engine already has (`secrets/engine.env` for an `s3://` bucket, the `pg-api-keys.json` key for `pg-control`), passed as read-only mounts; it creates none. The pageserver and safekeeper API calls (§7.2) run inside the `loams migrate` helper; no other image reaches them.

```
loams upgrade check  [--dir D] [--to V|latest]                     read-only
loams upgrade        --to V|latest [--dir D] [--bundle DIR] [--yes] [--no-auto-rollback] [--keep-snapshot]
loams upgrade rollback [--dir D] [--snapshot ID] [--keep-writes] [--yes]
loams upgrade status [--dir D]                                     the ledger and the last attempt, read-only
loams upgrade snapshots list|prune [--dir D] [--yes]
```

Exit codes and error codes follow §30 §6.3:

| Code | Error codes |
|---|---|
| 3 | `confirmation_required` (every mutating form without `--yes` and without a TTY) |
| 4 | `stack_not_found`, `release_not_found`, `snapshot_not_found` |
| 5 | `upgrade_step_not_adjacent`, `component_downgrade`, `insufficient_space`, `stack_unhealthy` (refused before it starts), `migration_incomplete`, `upgrade_in_progress`, `snapshot_incomplete` |
| 6 | `rollback_needs_snapshot`, `stack_unrecognised`, `platform_unsupported` (no compose) |
| 7 | `timeout`, `network`, `registry` |
| 9 | `checksum_mismatch`, `signature_invalid`, `manifest_invalid` (the bundle or an image digest) |
| 10 | `upgrade_failed_rolled_back`: the upgrade failed and the automatic rollback succeeded (new; the data is as before) |
| 11 | `rollback_failed`: needs a person (new; the CLI prints the snapshot's location and the manual steps) |

### 10.2 `loams upgrade check`

Reads the stack directory, asks the running engine for `GetInstance` (or reads the stack record through a `loams migrate plan` container when the engine is down), fetches the signed manifest for the target, and prints:

```json
{"current": "0.1.0-preview.2", "target": "0.1.0-preview.3", "path": ["0.1.0-preview.3"],
 "migrations": [{"id": "0000-adopt-preview-2", "store": "meta", "class": "reversible", "estimate": {"seconds": 2}},
                {"id": "0001-safekeeper-volume", "store": "host", "class": "reversible", "estimate": {"seconds": 30}}],
 "components": [{"name": "tikv", "from": "v8.5.8", "to": "v8.5.8"}],
 "snapshot_estimate_bytes": 2147483648, "free_bytes": 53687091200,
 "downtime_estimate_seconds": 240, "point_of_no_return": false, "blockers": []}
```

`blockers` lists what would make `upgrade` refuse: an unadjacent step (with the chain), a downgrade, too little space, an unhealthy stack, a migration left `failed`.

### 10.3 `loams upgrade --to <version>` (D935)

1. **Preflight**: `check`'s blockers must be empty; the stack must be healthy (every service's healthcheck, the engine's `/ready`), so a broken stack is not snapshotted as "good".
2. **Fetch and verify** the target's compose bundle and manifest (D936); verify the signature and checksums (exit 9 on failure).
3. **Pull** every image of the target bundle by digest. The stack still serves; a pull failure changes nothing.
4. **Close the door**: stop `tailscale` and `house-tailscale`. From here no client reaches the stack.
5. **Quiesce**: stop agentd, the console, the computes (`pg-runner`'s projects), `pg-control`, the House, the engine, TiDB. Drain the safekeeper (§7.2).
6. **Snapshot** (§7).
7. **Install the bundle**: unpack the target bundle into the stack directory, keep `.env` and `secrets/`, apply the release's `.env` changes (renamed or new variables, listed in the manifest; old values kept), set `LOAMS_VERSION`.
8. **Third-party restarts** in their order: PD, then TiKV, when their pins changed.
9. **Migrate**: `docker compose up migrate`; wait for exit 0.
10. **Ordered start**: TiDB → `loams` → `house` → the Postgres stack, `pg-control`, `pg-runner` → agentd, console. Each waits for the previous one's healthcheck.
11. **Verify**: `loams migrate verify` (the read probes: the stack record equals the target, the engine's `GetInstance.server_version` equals the target, a read of each product that has data, §10.4).
12. **Open the door**: start the Tailscale sidecars.

On a failure in steps 8–11, the CLI rolls back automatically (unless `--no-auto-rollback`), with the door still closed, and exits 10. A failure in steps 1–3 changes nothing. A failure in 4–7 restarts the old release from the unchanged data.

### 10.4 What "verified" means

`loams migrate verify` runs, inside the stack: the engine's `/ready`; `GetInstance`; for each product with data, a read that touches the migrated formats (list namespaces and collections and run one search; list Live tables and read a document; open each graph; `SELECT 1` and a table count on TiDB; a query on each Postgres timeline's endpoint if one is running; `SELECT version()` on the House; read a Durable promise). It reports per product `ok`, `empty` or `failed`. The release gate's fixture checks (§14) are stricter; `verify` is what every user's upgrade runs.

### 10.5 BYOC

`deploy/release/byoc/upgrade.sh -i KEY user@vm --to V` copies the CLI binary for the VM's architecture from the target bundle, runs `loams upgrade --to V --yes --dir <the bundle dir>` over SSH, and streams its output (Q832). It needs nothing listening on the VM beyond SSH.

## 11. Releases (D936, D942)

### 11.1 `release/upgrade-path.toml`

The source of truth for §5.1: each release, its `from`, its migrations (id, store, class, a one-line description), its third-party pins, its `.env` changes and its minimum free-space factor. A repository check (`scripts/release/check-upgrade-path.sh`) verifies that every migration id compiled into `loams` is listed exactly once, under the release that introduced it, that `from` chains back to `fresh`, and that pins match the pin files.

### 11.2 In the signed release

`release-github.yml` already publishes `loams-compose-<version>.tar.gz` with `SHA256SUMS`. UP1 adds `upgrade-path.toml` to the bundle and an `upgrade` object to the signed release manifest (`loams-release.json`, D292): `{from, migrations, components, env_changes, images: {name: digest}}`. The CLI trusts only the signed copy; the bundle's own file is informational.

### 11.3 Release notes (D942)

`scripts/release/upgrade-notes.sh <version>` renders the "Upgrading" section: the `from` release, the command (`loams upgrade --to <version>`), each migration with **Reversible** or **Point of no return** and its description, third-party bumps, the snapshot estimate and the downtime estimate. `release-github.yml` appends it to the generated notes; the CHANGELOG entry carries it; a repository check fails a pull request into `main` whose CHANGELOG entry for the release lacks it.

## 12. Other ways Loams runs

### 12.1 CLI local stacks (D937)

`loams stack restart --upgrade` (§30 §8.3) stops the stack, runs `loams migrate snapshot` and `loams migrate run` with the new binary against the stack's data directory and metastore, and starts it. Data stays only through `migrate`, never through an implicit format change. `stack describe` shows `migration_pending` when the installed binary's release is newer than the stack record.

### 12.2 The desktop (D940)

See §13.3.

### 12.3 Clusters, Kubernetes and BYOC rings (D943)

Not built in UP1. When the Helm chart and §41's rings exist: `migrate` is a Job in the sync wave before the engine; the snapshot is the bucket's versioning plus a BR backup to object storage; a point-of-no-return migration follows §41 §8.2's expand-then-contract rule (its contract step ships only after the expand step has been in `stable` for a full ring cycle); `noDowngrade` markers come from the migration classes.

## 13. Clients

### 13.1 The hosted console (D938)

- `console.loams.dev` always runs the newest console. It must work with the newest engine (N) and the one before (N−1), because a self-hoster upgrades on their own schedule.
- The console already reads `GetInstance` (`server_version`, `features`, `api_versions`, `services`) at boot (§17). UP1 adds a capability map: each plugin and page declares the service and minimum version it needs, and the shell hides what the engine does not serve (no dead pages, no error toasts for a missing RPC).
- An engine older than N−1, or one without `GetInstance`, gets a blocking page: the engine's version, the console's supported range and `loams upgrade --to <latest>`.
- The engine fills `min_app_versions` (field 12) from the signed manifest, so an app older than the engine supports is told so.
- The console's own tests run against two engine fixtures, N and N−1 (`console_supports_previous_engine`).

### 13.2 SDKs and protos (D939)

- `buf breaking` already runs (`app-protos`, against `origin/main`). UP1 adds a second run against the **previous release tag**, so a break that slipped into `main` between releases is still caught at the release.
- A package leaves the `unstable` ignore list only by a deliberate change in `buf.yaml`; from then on it is additive-only.
- The SDK fixtures (`sdks/fixtures/manifest.json`) gain a "removed symbol" check per supported SDK: a public symbol present in the previous release's SDK and absent now fails, unless the package is `unstable`.

### 13.3 The desktop (D940)

- The desktop updates through the `desktop-latest` feed (`desktop-promote.yml`, `electron-updater` with signed manifests, no downgrade).
- At the first launch after an update, before the engine starts, the daemon (DD1; Electron's `EngineSupervisor` until DD1 lands) runs `resources/bin/loams migrate snapshot` and `migrate run` against `userData/engine`, with the snapshot in `userData/upgrade-snapshots/`. A failure keeps the engine stopped, shows the error, and offers **Restore the previous version's data** (the snapshot) and **Report**.
- **Downgrades.** §50 §5.3 step 5 says a daemon newer than the app is replaced, because the bundled version is authoritative. For the engine's **data** that becomes: a bundled engine older than the data's stack record starts only if every migration newer than it is `Reversible`; otherwise the app refuses to start the engine and offers the snapshot restore. The daemon binary itself is still replaced as §50 says.
- The desktop's compose stacks (`userData/stacks`, re-copied on version change) follow the compose rules of §6–§10, driven by the app instead of the CLI.

## 14. The release gate (D941)

**Job:** `Upgrade: previous release → candidate`, in `ci.yml`, full tier (`needs.changes.outputs.full == 'true'`), on the compose-capable heavy runner, in `CI required`. It also runs in the nightly on `dev`, and on a pull request into `dev` with the `ci:upgrade` label (a pin bump, §9.2).

1. **Previous.** Read the previous release from `release/upgrade-path.toml` (`from` of the candidate), download its signed compose bundle, and start it with its published images. The CI override `deploy/release/ci/compose.upgrade.yaml` replaces the Tailscale sidecars with a loopback port and sets `LOAMS_AUTH_MODE=apikey`.
2. **Fixtures.** `conformance/upgrade/` writes fixture data for every product the previous release ships: collections (native, Qdrant and Elasticsearch APIs, with vectors and text), streams and links, Live (tables, documents, an index), Graph (a graph with nodes and edges, snapshotted), SQL (a database, tables, rows), Postgres (a tenant, a timeline, rows, a branch), House (what the previous release accepts: lake tables and DDL until `INSERT` ships), Durable (promises in each state), and the API keys file. Each fixture records its expected reads.
3. **Upgrade.** Build the candidate images from the pull request's head (`deploy/release/build.sh --platforms linux/amd64`, loaded locally under the candidate version), run `loams upgrade --to <candidate> --bundle <the candidate's deploy/release> --yes`, and check: every fixture reads back identically through every API it was written with; a new write of each kind succeeds and reads back; the ledger lists exactly the release's migrations, all `done`.
4. **Reversibility.** When the release's migrations are all claimed `Reversible`: start the previous release's images on the migrated data, which by now holds step 3's new writes, and read both the fixtures and those new writes. A failure fails the job and names the migrations claimed `Reversible`.
5. **Rollback.** Run `loams upgrade rollback --yes` and check that every fixture reads back as in step 2 and that the step-3 writes are gone. When every migration is `Reversible`, also check `rollback --keep-writes` on a fresh run of steps 1–3: the previous release reads the fixtures and the step-3 writes.
6. **Artifacts.** The ledger, `snapshot.json`, `check`'s JSON, the timings (snapshot, migrate, downtime) and every service's logs.

**Bootstrapping.** The first previous release that can run step 1 is preview.2. On the preview.3 release pull request the gate runs preview.2 → preview.3 for real; before that, the job runs on `dev` from preview.2's bundle as soon as UP1 Tasks 1–14 and 19 land, so failures show up before the release.

**Release flow.** `CI required` already gates `main`; the job's presence in it is what makes "a release cannot merge to `main` without it" hold. Admin merges into `main` must not bypass it (Q826).

## 15. Reconciliation with earlier designs

| Earlier | What it says | How §54 fits |
|---|---|---|
| §03 §6 and §10 §7 in the code | Readers support N and N−1 | Almost every reader checks the exact version today (§17). UP1 Task 1 moves them to the registry's range; the rule is unchanged |
| §10 §7 | Format N/N−1; a node refuses formats it cannot read; new formats enabled only after every node reads them; metastore migrations through the Raft log, `sqlx` or item-format versions | Kept and extended to every store (§3, §4). On single-node compose all processes upgrade together, so `migrate` enables formats in one step; clusters keep §10 §7's two-phase enable (D943). The ledger records metastore migrations; Raft-backed stacks apply them as Raft commands |
| §10 §6 | Backups are the bucket plus metastore snapshots | The upgrade snapshot (§7) is a narrower, local, pre-upgrade copy; it does not replace §10 §6 |
| §03 §6–§7 | Formats rolled forward by compaction, opt-in per object; GC by reachability | Compaction still rolls objects forward; a migration may instead rewrite pointers. Snapshots add GC roots (§7.2) |
| §45 §4.3 (Live) | App data migrations are user mutations; no server-side migration framework in LV1 | Unchanged for app data. `migrate` covers only Live's own storage formats (system keys, index layouts), which are Loams' |
| §46 §12 (Postgres) | Compute images apply on next start; pageservers and `loams-wal` roll one at a time; majors through `UpgradeProject` | Unchanged for the managed product. The compose stack is single-node, so its components restart in §10.3's order after a drain (D945). Majors never through `loams upgrade` (D933) |
| §51 §9 | Data is never upgraded to a new Postgres major without an explicit user action | Kept: `loams upgrade` refuses a major change (§9.3) |
| §47 §5 (SQL) | TiDB bootstrap runs once per keyspace in `CreateDatabase`, with upgrade DDL | A TiDB version bump is a third-party bump (§9); TiDB bootstraps, and upgrades, each keyspace's system tables when its `tidb-server` starts (UP1 ruling T0-8) |
| §48 §14.4 (Graph) | Rolling; change-set and manifest formats versioned, new readers read every older version; Grafeo format changes rebuild from the portable snapshot | Kept. Grafeo's local files are a cache (S12), deleted on restore. "Reads every older version" is stronger than N−1 and stays Graph's own rule |
| §49 §19 (House) | Engine upgrades migrate no data, because storage is Iceberg | True in production. The preview's single-node House keeps a local data directory (S10), which the snapshot copies |
| §41 §8 | Rings; expand-then-contract; `noDowngrade`; rollback is a channel revert | Kept for clusters (D943). The migration class supplies `noDowngrade` |
| §50 §5.3 step 5 | A daemon newer than the app is replaced; the bundled version is authoritative | Amended for the engine's data (D940): an older engine starts only on reversibly-migrated data |
| §30 §8.3 | `stack restart --upgrade` keeps data "under the format N/N−1 rules of §10 §7" | Amended (D937): it runs `migrate` first |
| CL1 Global Constraints | Products only through their public API; the supervisor is the one exception | A second exception, `loams upgrade` (D934) |
| §30 §6.3 | Exit codes 0–9 and 130 | Two codes added for `upgrade` only: 10 (`upgrade_failed_rolled_back`) and 11 (`rollback_failed`); every other code keeps its meaning (D934) |
| `docs/release/preview.md` | "No upgrade path between previews"; "back up the compose volumes yourself" | True for preview.1 → preview.2. From preview.3 the page documents `loams upgrade` (UP1 Task 23) |
| §12 (roadmap) | Rolling upgrades and format checks are M2 → v1.0 gates | Unchanged; §54 adds the offline compose upgrade now, which v0.1.0's exit needs |

## 16. Security (D944)

- **Images by digest.** The CLI pulls every image by the digest in the signed manifest, never by tag. A digest mismatch exits 9.
- **The secret-bearing snapshot stays on the host.** The snapshot directory holds secrets (`.env`, API key hashes, `house.key`, TiKV data, SQLite, the House directory) in a volume or directory of mode 0700 owned by the stack's user, and Loams never uploads it. The one exception is §7.3's copies of Git's mutable bucket objects (`bucket-pointers/`), which with an `s3://` bucket are also written to `<bucket>/_system/upgrade/snapshots/<id>/`: they are copies of objects that already live in that bucket and carry no secret from the host.
- **The door is closed** from quiesce to verification: the Tailscale sidecars are the stack's only ingress (preview.md's security model), so stopping them leaves no reachable listener.
- **No new credentials.** `migrate` uses the engine's own configuration and secrets mount; it opens no listener; it has no container runtime socket. The CLI's helper containers get the snapshot volume and the data volumes, nothing else.
- **No downgrade attacks.** The CLI refuses an older target (`component_downgrade`, D933) and a manifest whose signature does not verify.

## 17. Where things are today (checked on `dev` at `7957cfb0`, 2026-10-13)

Read on `dev` at `7957cfb0`. Paths are from the repository root. UP1 Task 0 (the plan's rulings T0-1 to T0-12, on `dev` at `3ac1e866`) adds formats this table misses (`text.checksums`, `hnsw.flat`, `graph.recovery_stamp`, `pg-control`'s secrets file and CA, the split trailer's third reader) and records the third-party facts marked (verify) here.

**Format versions.** Most Loams formats already carry a magic and a version, but **almost every reader accepts only the exact current version**, so Rule 1's "N and N−1" is not yet true in code. UP1 Task 1 replaces the exact checks with the registry's range.

| Format | Constant | Reader today |
|---|---|---|
| WAL object, log segment | `VERSION: u16 = 1` (`crates/loams-log/src/wal.rs`, `segment.rs`); Kafka batch magic 2 | exact |
| Collection manifest `OPCM`, PK delta `OPPD`, dead letters `OPDL` | `MANIFEST_FORMAT_VERSION = 1` (`crates/loams-collection/src/manifest.rs`), v1 envelopes (`pkindex.rs`, `deadletter.rs`) | exact |
| Record codec, PK index value | `CODEC_VERSION = 0x01` (`codec.rs`), `VALUE_VERSION = 0x01` (`pkindex.rs`) | exact |
| Tantivy split trailer, delete bitmap `OPDB` | `TRAILER_VERSION = 1` (`crates/loams-text/src/split.rs`, `crates/loams-hot/src/splits.rs`), `DELETE_BITMAP_VERSION = 1` | exact |
| Hot artifacts `OPHD`/`OPHC`, hot owner record | `ARTIFACT_FORMAT_VERSION = 1` (`crates/loams-hot/src/artifact.rs`); `"v1"` (`registry.rs`) | exact |
| Link counter data and manifest | `FORMAT_VERSION = 1` (`crates/loams-link/src/counter.rs`) | exact |
| Lance, Tantivy | Lance file format 2.1 (`lance =12.0.0`), `tantivy =0.26.2` | the library's |
| **Git WAL segment, checkpoint** | `SEGMENT_VERSION = 1`, `SEGMENT_MIN_VERSION = 1`; `CHECKPOINT_*` (`crates/loams-git/src/format/`) | **range (N, N−1)**, the policy in `format/mod.rs` |
| Git pack `.lpk`, Git catalog documents, Git `<root>/head` hint | no version (a reserved zero word); `DOC_VERSION = 1` (`catalog.rs`, exact); unversioned JSON, ignored when unreadable | — |
| Metastore Raft snapshot `OPNMETA` | `SNAPSHOT_FORMAT_VERSION = 8` (`crates/loams-meta/src/codec.rs`) | **accepts 5–8** |
| Metastore Raft log entries and local records (`meta.redb`) | bare postcard; the `Command` enum's variant order is the format | none |
| Metastore on TiKV | `FORMAT: u8 = 1` per record (`crates/loams-meta-tikv/src/keys.rs`) | exact |
| Pointers (`collection/<cid>`, `link/<id>`, `graph-manifest/<gr>`, `graph-catalog`) | metastore entries holding an object path and a CAS version (`crates/loams-meta/src/state/pointers.rs`) | no format version |
| Graph manifest, change-set record, portable snapshot | `MANIFEST_FORMAT = 1`, `RECORD_VERSION = 1`, `PORTABLE_VERSION = 1` (`crates/loams-graph/src/…`) | exact; a native snapshot of another Grafeo `engine_version` falls back to the portable one, and `cache.json` with another engine version is rebuilt |
| Graph catalog document | `struct Document { graphs }` (`crates/loams-graph/src/catalog.rs`) | no version |
| Live records on TiKV | protobuf `format = 1` in `DocumentRecord`, `TableDef`, `TableJobRecord`, `AppDef`, `DeploymentRecord`, `IdempotencyRecord` (`proto/loams/live/v1/`) | exact; `JournalEntry` has no format field; the local Live store is `<data-dir>/live/store.redb` |
| Durable on SQLite | Resonate's `sqlx::migrate!` (`0001_initial.sql`, pinned rev `e3606698`), table `_sqlx_migrations` | SQLite opens with `migrate = true` (`crates/loams-durable/src/config.rs`), so serving migrates implicitly; MySQL/TiDB stores are migrated only by `loams durable migrate` and checked by `mysql::check_schema` |
| House data directory | `spool/`, `catalog.sqlite` (`CREATE TABLE IF NOT EXISTS`, no `user_version`), `bucket/`, `workers/`, `house.key` (`fabric/crates/loams-fabric/src/house.rs`); `CHDB_VERSION = "26.9.0"`, `CLICKHOUSE_VERSION = "26.9.2.1"` | no stamp |
| `pg-control` (`--store=local`) | `<data-dir>/pg-control.redb` on `loams-kv`, keyspace `loams_pg`; `FORMAT: u8 = 1` per record (`crates/loams-pg-control/src/model.rs`) | exact |
| agentd store | `schema_migrations` table and an append-only `MIGRATIONS` list (`crates/loams-agentd-store/src/store.rs`); `WORKSPACE_SCHEMA_VERSION = 2` | its own ledger: the pattern `loams-migrate` follows |

**Migrations that exist.** Only `ensure_graph_stream` (it renames `graph.<gr>` to `_graph.<gr>` in place, `crates/loams-common/src/meta/store.rs`), `loams durable migrate` (`crates/loams/src/main.rs`), and agentd's own. There is no stack version, no ledger and **no `_system` namespace** (`_system:*` is the name of Live's built-in functions, and House rewrites `system.*` to `__loams_system`; neither is a namespace). UP1 Task 2 reserves `_system`.

**The CLI.** `crates/loams/src/main.rs` has `dev`, `standalone`, `cluster`, `warm`, `durable migrate`, `auth new-key`, `live-worker`, `pg-control`, `pg-api-key` and `pg-runner`. `crates/loams-cli` (CL1) does not exist yet. Nothing drives a compose stack except `pg-control`'s compute runtime.

**The engine's version.** `GetInstance` (`proto/loams/instance/v1/instance.proto`, `crates/loams/src/api/connect.rs`) returns `server_version` (the crate version), `api_versions` and `services`; `features` and `min_app_versions` are empty. `/health` and `/ready` return status codes only. The console reads `GetInstance` at boot (`web/packages/console-host/src/boot.ts`) and shows `serverVersion`; the hosted console probes `/health`.

**The compose stack.** `deploy/release/compose.yaml` (project `loams`): volumes `pd-data`, `tikv-data`, `loams-data` (the bucket at `file:///var/lib/loams/bucket`, Durable SQLite, Graph files), `house-data`, `tailscale-state`, `house-tailscale-state`, `headscale-data`, `agentd-data`, `pg-control-data`, `pg-runner-data`, `pg-runner-token`. The metastore is TiKV keyspace `loams_meta`, Live is `loams_live_dev`. It includes `deploy/loams-postgres-dev/compose.yaml` (project `loams-neon`): `rustfs` (`rustfs-data`, the Postgres bucket), `pageserver` (`pageserver-data`), `storage_broker`, **`safekeeper1` with no volume** (its WAL is in the container filesystem), and computes. Images are pinned by digest from `deploy/pins/loams-postgres-images.toml`.

**Releases.** `release-images.yml` builds images on a `v*` tag; `release-github.yml` publishes `loams-compose-<version>.tar.gz` (`deploy/release` plus `deploy/loams-postgres-dev`) with `SHA256SUMS` and inline release notes. `release-compose.yml` checks the compose file's configuration only. **No CI job starts the release stack.** `CHANGELOG.md`'s preview.2 entry says "no upgrade path: start a fresh stack". `buf breaking` runs in `ci.yml`'s `app-protos` against `origin/main`, ignoring the unstable packages listed in `buf.yaml`.

**The desktop.** `apps/desktop-electron`: the engine's data is `userData/engine` (`src/main/paths.ts`); compose stacks are copied to `userData/stacks` and re-copied when the app version changes (`.loams-stack-version`); `electron-updater` 6.8.9 with `allowDowngrade = false`; the `desktop-latest` feed is written only by `desktop-promote.yml`. A Postgres major mismatch is reported (`pg_major_mismatch`) and the only remedy is a reset.

### 17.1 As built (UP1, 2026-10-16)

The table above is the state before UP1. What UP1 built, and where it differs from §1–§16 (the rulings are in
the plan's "Rulings made during execution"):

- **`_system` already existed in preview.2** (the auth principal directory). The ledger shares it under
  `upgrade/*` pointers. The stack record is the commit point, because the metastore has no multi-key
  transaction (T0-2).
- **The client is `crates/loams-upgrade`**, flattened into `loams` as `loams upgrade` (CL1's `loams-cli` had
  not landed, T0-3). It also builds as `loams-upgrade` for CI.
- **TiKV:** BR `backup txn` when the stack's `tikv` mounts the snapshot volume (preview.3 on). A stack without
  that mount, such as preview.2, gets a cold copy of `pd-data` and `tikv-data` as one archive.
- **The host files** of a snapshot live in `<stack>/.loams-upgrade/snapshots/<id>/host/`, beside the history
  file, not in the snapshot volume.
- **The signed manifest (CLI2) does not exist yet.** A downloaded bundle is checked against the release's
  `SHA256SUMS`; the manifest's `upgrade` object is the asset `loams-upgrade-<version>.json` (T11-1, T12-1).
- **`loams-postgres`** (the pageserver and safekeeper HTTP client) is in the default build through
  `loams-migrate`, because the release `loams` image runs `migrate` (T9-1).
- **An older release over newer data** that only `Reversible` migrations changed is accepted by both the
  startup gate and `migrate run` (a no-op). This is what makes `rollback --keep-writes` work (T14-2).
- **The safekeeper volume** cannot be guarded by compose alone, because the included Postgres file cannot
  depend on `migrate`. A hand-run `compose up` across preview.2 → preview.3 is unsupported (T10-1).
- **The console** gates plugins on `requires.api` and the new `requires.engine`. `GetInstance.features`
  announces `feature.*` and `role.*` (T16-1 to T16-3).

## 18. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | preview.2's data does not match the fingerprint on some real stack (an older image, a hand-edited stack), or a preview.1 stack passes it | Medium | Medium | `stack_unrecognised` stops before anything changes; the fingerprint is tested on stacks built from preview.2's bundle in CI; Task 0 looks for a preview.2-only marker; Q820 |
| 2 | BR does not cover API V2 keyspaces on v8.5.8, or its `local://` storage is awkward in compose | Medium | Medium | Cold volume copy as the fallback (Q821); UP1 Task 0 decides |
| 3 | A migration wrongly marked `Reversible` | Medium | High | The gate's reversibility step (§14 step 4) proves each claim |
| 4 | Safekeeper WAL lost when its container is recreated | Medium | High | The drain (D945) before any recreate; preview.3 adds a volume |
| 5 | Snapshots fill the disk | Medium | Medium | The size estimate and the 1.5× free-space check; one snapshot kept; `snapshots prune` |
| 6 | A weekly cadence leaves no time for the gate's runtime | Medium | Medium | The gate runs nightly on `dev` too, so the release pull request rarely finds a new failure; timings are tracked |
| 7 | Users run `docker compose pull && up -d` and skip `loams upgrade` | High | High | `migrate` still runs (it is a compose dependency) and either migrates or refuses (`upgrade_step_not_adjacent`); only the snapshot is missing, and `migrate` refuses to run a `PointOfNoReturn` migration without a recorded snapshot unless `--no-snapshot-i-accept-data-loss` is set |
| 8 | The hosted console ships a change that breaks N−1 engines | Medium | Medium | `console_supports_previous_engine` in CI against both fixtures |
| 9 | A third-party bump (TiKV, chDB) has an upgrade bug we cannot fix | Low | High | Pins; the gate on every bump; the snapshot; restore into a fresh cluster at the old version |
| 10 | GC deletes an object a snapshot still needs | Low | High | Snapshot GC roots read before any delete; a test that runs GC between snapshot and restore |

## 19. Open questions (Q820–Q834)

| # | Question | Default | Owner | Needed by |
|---|---|---|---|---|
| Q820 | preview.2 (#518) is not released yet. Add a stack-version stamp to it before the tag, or ship it as is and adopt by fingerprint? | Ship as is; preview.3 adopts by fingerprint (D926). A stamp would change a frozen release | Owner | UP1 Task 5 |
| Q821 | TiKV snapshot: BR, or a cold copy of `pd-data` and `tikv-data` on single-node compose? | BR if UP1 Task 0 shows it covers both keyspaces on v8.5.8 with `local://` storage in compose; else the cold copy | Eng | UP1 Task 9 |
| Q822 | How many snapshots to keep, and for how long? | The latest one, until the next upgrade verifies; `snapshots prune` earlier | Owner | UP1 Task 8 |
| Q823 | Roll back automatically when verification fails? | Yes, while the door is still closed; `--no-auto-rollback` to keep the failed state for diagnosis | Owner | UP1 Task 13 |
| Q824 | Offer `rollback --keep-writes` for fully reversible upgrades? | Yes, gated by the ledger and proved by the gate | Owner | UP1 Task 14 |
| Q825 | `--to` more than one step ahead: walk the chain automatically, or refuse and print the chain? | Walk it, one verified step at a time | Owner | UP1 Task 13 |
| Q826 | Admin merges into `main` must not bypass the upgrade gate. Make `CI required` non-bypassable for release pull requests? | Yes: release pull requests into `main` merge only with `CI required` green | Owner | UP1 Task 20 |
| Q827 | Upgrade from a git checkout of the repository, not a release bundle? | Supported only when the checkout is at the target's tag and clean; otherwise use the bundle | Eng | UP1 Task 13 |
| Q828 | `migrate` in the `loams` image, or a separate `loams-migrate` image? | The `loams` image (D927) | Eng | UP1 Task 7 |
| Q829 | Postgres snapshot: branches always, plus a cold copy of the Postgres volumes when the Loams Postgres pins change? | Yes | Eng | UP1 Task 9 |
| Q830 | The console's support window: N−1, or N−2 as well? | N−1; older engines get the upgrade page | Owner | UP1 Task 16 |
| Q831 | The desktop snapshots its engine data before every engine upgrade (disk cost)? | Yes, one snapshot kept | Owner | UP1 Task 18 |
| Q832 | BYOC: `byoc/upgrade.sh` copies the CLI to the VM and runs `loams upgrade` there? | Yes | Eng | UP1 Task 22 |
| Q833 | The downtime target for a single-node compose upgrade | 5 minutes excluding pulls, for under 10 GiB outside the bucket | Owner | UP1 Task 20 |
| Q834 | Do 0.2.0 previews keep data from 0.1.x, under the same rules? | Yes | Owner | Before 0.2.0 |

## 20. Sources

**Loams (read on `dev` at `7957cfb0`, 2026-10-13):** design §03, §10, §12, §30, §41, §44, §45, §46, §47, §48, §49, §50, §51; plans CL1, PG2, NF1, MM1 (format); `deploy/release/compose.yaml`, `.env.example`, `build.sh`; `deploy/loams-postgres-dev/compose.yaml`; `deploy/pins/loams-postgres-images.toml`; `.github/workflows/{ci,nightly,release-github,release-images,release-compose,desktop-promote}.yml`; `docs/ci/README.md`; `docs/release/{preview,publishing,desktop}.md`; `CHANGELOG.md`; `proto/loams/instance/v1/instance.proto`; `crates/loams/src/main.rs`; `apps/desktop-electron/src/main/**`; and the crates named in §17.

**Third-party (to verify in UP1 Task 0):** TiDB/TiKV v8.5 BR documentation (backup and restore of API V2 keyspaces, `local://` storage); PingCAP's upgrade order for TiDB clusters; Neon's pageserver timeline API (`POST /v1/tenant/{t}/timeline` with `ancestor_timeline_id`, `remote_consistent_lsn`) and safekeeper status API, as forked in `ostrium-labs/loams-postgres`; SQLite's online backup API (`rusqlite` `backup`); Docker Compose `depends_on.condition: service_completed_successfully` (Compose v2.20+, which the bundle already requires).
