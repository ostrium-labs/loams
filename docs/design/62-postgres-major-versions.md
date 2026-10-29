# 62 — Postgres Major Versions: 16 and 17 Now, 18 Next, 19 After Its First Minor

Status: **Approved** · 2026-10-18 (owner approved all decisions and questions, 2026-10-18). Source: the owner's request of 2026-10-18, **"I want Postgres 16, 17 latest and plan for 18 and 19 support."** Decisions **D1107–D1118** and questions **Q949–Q959** are in the [decision log](13-decision-log.md), all approved by the owner on 2026-10-18. Plan stub: [NF2](../plans/2026-10-18-nf2-postgres-18-19.md). **This document writes no code.**

This is an addendum to [§51](51-loams-postgres-fork.md) §8 (Postgres 18 and 19) and §9 (upgrade paths). It replaces their per-major detail with what was checked on 2026-10-18 against the fork, upstream Postgres and the Loams tree. It also moves NF1's milestones NF1e and NF1f (Tasks 24–35) and NF1 Tasks 36–38 into a new plan, NF2 (D1107). It touches these earlier documents:
- [§46](46-loams-postgres-production.md) §12 and D716: major upgrades of Loams Postgres projects;
- [§54](54-upgrades.md) §9.3 (D945): `loams upgrade` never changes a Postgres major;
- [§57](57-loams-gateway.md) and [§58](58-data-access-and-htap.md) §3: protocol 3.2 and long cancel keys, plan advice on 19, and `loams-wal` against TiKV by role (D1057).

§11 lists every amendment.

Markers, as in §51:
- **(verify)** means the claim has not been checked against a primary source or running code. The task that depends on it checks it first.
- **(estimate)** means the number was judged, not measured.
- **(target)** is a number this document sets as a gate.

---

## 1. Summary

**Where things stand (2026-10-18):**
- **16 and 17 are at their latest minors.** Release `lp-2026.11.0` (fork commit `a213ec44`, published 2026-10-18) carries 16.15 and 17.11. Loams PR #774 pins it for 0.1.0. On postgresql.org, 16.15, 17.11 and 18.6 were released on 2026-08-13 and are the newest minors of each major. 18.5 was never released.
- **18 runs nowhere in Loams Postgres.** Neon's tree at the fork point (`fa504217`) supports 14–17 only. Neon's service runs 18.6, but its storage-side and extension code for 18 is not public. The only public 18 tree is Neon's Postgres branch `REL_18_STABLE_neon` (18.2, frozen 2026-04-08). Our `pgxn/neon` cannot build against it (§3.1).
- **19 is a release candidate.** `REL_19_RC1` was tagged on 2026-10-12 and announced on 2026-10-15. RC2 is planned for 2026-11-12 and **GA for 2026-11-19**. §51 had GA on 2026-10-29, and this document corrects that (D1113). The first minor, 19.1, is due with the quarterly release on 2027-02-11 (verify on the day).

**The plan in one line.** Build 18 now, with the format work done first, and ship it as beta and then as supported. Port 19 only once 18 is in beta. Call 19 supported only after upstream's 19.1 has been merged and gated (the owner's "GA + 1 minor"). Major upgrades go through new timelines, and a user always starts them.

| # | Decision | Status |
|---|---|---|
| D1107 | **Plan NF2 takes over the Postgres-major work.** It supersedes NF1 Tasks 24–35 (NF1e, NF1f) and takes over Tasks 36–38 (PgDog per major, branch upgrades, the `pg_upgrade` spike). NF1 keeps Tasks 0–23 and 39–41, except Task 41's removal of 14 and 15, which NF2 Task 16 brings forward | Proposed |
| D1108 | **The support window** (§7.1): at most **three supported majors** (new projects allowed), **one maintenance major** (minors, exports and upgrades only, no new projects) and **one beta major** (opt-in). **16 becomes supported** again until 19 is supported, then maintenance until its EOL on 2028-11-09 (**amends D813**'s "16 is maintenance-only"; Q949) | Proposed |
| D1109 | **The 18 branch** `loams/pg18` starts at `REL_18_6`. Our 17 patch series (`loams/pg17` minus `REL_17_11`, 104 commits) is ported onto it in `docs/core_changes.md` order, one `Loams-Patch` id per change. Neon's `REL_18_STABLE_neon` is a **reference for conflict resolutions only, never a base**, for the same reason Task 17's recut retired `loams/REL_17_STABLE`. `loams/pg19` is made from `loams/pg18` the same way (§3.1, Q950) | Proposed |
| D1110 | **Asynchronous I/O at 18 and 19:** computes run `io_method = sync`, set and locked by `compute_ctl`. `pgxn/neon`'s smgr implements the new callbacks (`smgr_startreadv`, `smgr_maxcombine`, `smgr_fd`) by completing each read synchronously through the communicator. Native AIO is a later, measured project. Neon's service makes the same choice for 18 (§4.2, Q951) | Proposed |
| D1111 | **Data checksums stay off** on timelines bootstrapped at 18 and later (`initdb --no-data-checksums`), as at 17. Pages the pageserver reconstructs carry no valid checksum today. The import paths normalise the setting, and `pg_upgrade` needs both clusters to match. Checksums are a separate later project (§4.3, Q952) | Proposed |
| D1112 | **The fork keeps its heap WAL format at 18 and 19,** as option (a) did at 17.11 (`docs/loams/pg17-vm-clear.md`). Heap records stay `RM_NEON_ID` with no visibility-map block reference (upstream `b01c31eef9` is not taken). The pageserver derives VM changes from the record flags, including 19's `XLHP_VM_ALL_VISIBLE`/`XLHP_VM_ALL_FROZEN` prune flags, which replace the removed `XLOG_HEAP2_VISIBLE` (§5.2, Q953). **Amended 2026-10-29 (owner: "Yes, revert at 18"):** 18.6 back-patched the VM block references (`f581fa729d`), and the 18 port reverts them as 17.11 did (§4.1) | Proposed |
| D1113 | **Promotion rules** (§7.2). A major is **beta** once its compute images pass G0–G3. It is **supported** once (1) upstream has released at least one minor after the GA (x.1), (2) §8's matrix has passed twice on different days, (3) the beta has run 30 days on Loams' test cluster with no data-loss bug, and (4) the upgrade from the previous major is green. 18 already meets (1). For 19 it means **no earlier than 2027-03-13** (19.1 on 2027-02-11 plus the soak). **Corrects 19's GA date** to 2026-11-19 (planned) and refines D813 and Q718 | Proposed |
| D1114 | **pg-control's major surface** (§9). The offered majors come from the pin file through a generated `crates/loams-postgres/src/majors.rs`, not from `ServiceConfig`'s hard-coded `vec![17]`. Other changes: `CreateProjectRequest.allow_beta_major`; an output-only `Branch.pg_version`; `ListPgVersions`; `UpgradeProject` scoped to one branch with a method and a check-only mode; and the reasons `unsupported_pg_version`, `beta_major_requires_opt_in` and `compute_major_mismatch_refused` | Proposed |
| D1115 | **Major-upgrade paths** (§10). Path 1, `fast_import pgdata` into a new root timeline (D815), ships first, with 18. Path 3, **`pg_upgrade` on a new timeline** (Neon's open RFC 039 design: a new-major timeline that keeps reading the old major's relation pages), is a spike that **replaces NF1 Task 38's Path 2 spike** (`pg_upgrade --link` on a materialised data directory). Dump and restore stays the fallback. Answers Q725 in part; Q955 | Proposed |
| D1116 | **No `preview` list in the pin file.** A new major enters `[majors] beta` once its images pass the gate, so `pins.py`'s schema (`supported`, `beta`, `maintenance`, `default`) stays as built. This amends NF1 Tasks 28 and 29, which named `preview = [18]` | Proposed |
| D1117 | **Minor releases** (§7.3). Every Postgres minor, quarterly or out of cycle, ships in our images **within 7 days** for every supported, maintenance and beta major (D805's target, extended to non-security minors). The fork's bot runs the nightly try-merge, the merge job, the gate, the release and the pin PR. A maintainer resolves conflicts in Neon's patched areas and approves the pin PR. 14 and 15 leave the fork's build after 14's last minor on 2026-11-12 (Q956) | Proposed |
| D1118 | **The gate per major** (§8). G2 runs one leg per major in the pin file, beta included. G3's `nf1-candidate.yml` runs `pg2-e2e.yml` per major, not only on `compute-v17`. A new G2 job, `upgrade`, runs Path 1 from each supported major to the next. Each new major gets a WAL-format guard test like `test_loams_vm_clear` | Proposed |

## 2. What was checked, and where

| Fact | Value | Source (2026-10-18) |
|---|---|---|
| Latest minors and EOLs | 18.6 (EOL 2030-11-14), 17.11 (2029-11-08), 16.15 (2028-11-09), 15.19 (2027-11-11), 14.24 (2026-11-12, last release on that day) | postgresql.org `versions.json` |
| 19 schedule | Beta 1 2026-06-01 … Beta 4 2026-09-21; RC1 tagged 2026-10-12 (`0b8bff04`), announced 2026-10-15; RC2 planned 2026-11-12; **GA planned 2026-11-19** | `postgres/postgres` tags; the RC1 announcement |
| Next minors | 2026-11-12, 2027-02-11, 2027-05-13, 2027-08-12 | postgresql.org roadmap |
| The fork's `main` | `a213ec44` = `lp-2026.11.0`; `vendor/revisions.json`: 17.11 `aef0fc52` (`loams/pg17`), 16.15 `23f0d68a` (`loams/pg16`), 15.13 and 14.18 still vendored; no `vendor/postgres-v18`; `Makefile` `POSTGRES_VERSIONS = v17 v16 v15 v14`; the compute Dockerfile has `case` arms for v14–v17 only | `ostrium-labs/loams-postgres` |
| Rust majors | `PgMajorVersion` is PG14–PG17; `PgVersionId::from_full_pg_version` panics outside `140000..180000`; `postgres_ffi`'s bindgen and `for_all_postgres_versions!` cover v14–v17; `walingest.rs`'s per-version array is bounded PG14..=PG17 | `libs/postgres_versioninfo`, `libs/postgres_ffi`, `pageserver/src/walingest.rs` |
| `pgxn/neon` at 18 | No `PG_MAJORVERSION_NUM >= 18` conditional anywhere; no `io_method`, `read_stream` or `pgaio` reference. `pagestore_smgr.c` has 32 version conditionals | `pgxn/neon` |
| Postgres branches | `loams/pg16` (16.15, 107 commits over `REL_16_15`), `loams/pg17` (17.11, 104 over `REL_17_11`); `REL_18_STABLE_neon` `a616eefe` (18.2, 106 over `REL_18_2`); `loams/REL_18_STABLE` `05e445f1` retired | `ostrium-labs/postgres` |
| Neon in public | No commit since `fa504217` (2026-08-31). No 18 work in `neondatabase/neon` except a benchmarking PR closed unmerged (#12818). Issue #12945 ("Plans for PostgreSQL 18 support?") has no maintainer answer. Neon's service offers 14–18 and runs 18 with `io_method = 'sync'` | GitHub; neon.com's version policy and its Postgres 18 AIO page |
| Loams | `Project.pg_version` and `CreateProjectRequest.pg_version` exist (0 means the default); `ServiceConfig` defaults to `pg_versions: vec![17]`; `UpgradeProject` returns `NotBuilt`; `loams-wal-decoder` maps majors 14–17 only and has fixtures at 17 only; `loams-safekeeper` is version-agnostic and refuses a major change on a timeline; `it-pageserver-loams-wal.sh` hard-codes 17; `pg2-e2e.yml` hard-codes `compute-v17`; the desktop's `stacks.ts` has `PG_MAJOR = 17` | This repository at `dev` `46449f66` |

## 3. Branches, bases and the patch series

### 3.1 Why `loams/pg18` starts at `REL_18_6` (D1109)

NF1 Task 17 found that Neon's `REL_<N>_STABLE_neon` heads carry Postgres-side changes that pair with `pgxn/neon` changes Neon never published. Examples: the removed `set_lwlsn_block_*` hooks, `LastWrittenLsnLock` moved into the extension, the `neon_storage_token` GUC, and `f_smgr` without `smgr_read_slru_segment`. Our `pgxn/neon` did not build against them, so Task 17 recut 16 and 17 from the revisions `main` pinned (rulings 7 and 8). `REL_18_STABLE_neon` comes from the same later Neon tree, so it has the same problem.

| Option | Base | For | Against |
|---|---|---|---|
| **A (default)** | `REL_18_6` + our 17 series ported | Pairs with the `pgxn/neon` we have; every patch is ours and listed; starts at the newest 18 minor | We redo the conflict work Neon did for 18 (most of the 104 commits apply with small changes; the smgr, AIO and read-stream areas do not) |
| B | `REL_18_STABLE_neon` + merge `REL_18_6` | Neon's 18 resolutions come for free | It needs the unpublished `pgxn/neon` half, which we would have to rewrite blind |
| C | Plain `REL_18_6`, no Neon patches | Nothing to port | Impossible: the pageserver needs the WAL-redo mode, the smgr hooks and the WAL-logged relmapper, slots and snapshots |

With A, `REL_18_STABLE_neon` and Neon's 18 pull requests in `neondatabase/postgres` are still read as references. Those are #772 (no OS readahead for Neon), #822 (fewer FPIs), #882 (an injection point for testing the 18 smgr), `get_pin_limit_hook` and `extension_is_visible_hook`. Each piece taken from them is cherry-picked with a `(cherry picked from neondatabase/postgres <sha>)` line and its own id.

### 3.2 Layout

| Ref | Meaning |
|---|---|
| `loams/pg18` | `REL_18_6` + the ported series + our 18 patches; `vendor/postgres-v18` points at it (`.gitmodules` `branch = loams/pg18`) |
| `loams/minor-pg18-REL_18_<m>` | Each minor's merge, made by `loams-postgres-merge.yml` and promoted by fast-forward (as at 16 and 17) |
| `loams/pg19` | `REL_19_<first minor or RC>` + the 18 series ported. Created at Phase B (§6) |
| `loams/REL_18_STABLE` | Retired and kept protected, as today |

Merge, never rebase, and no force-push (D821). The series is ported **once per major** by cherry-pick in `docs/core_changes.md`'s order. After that, minors come in by merge.

### 3.3 The series by area (from `docs/core_changes.md`)

| Area | Changes | Risk at 18 | Risk at 19 |
|---|---|---|---|
| smgr | The smgr interface open to extensions; `relpersistence` in `smgropen()`; smgr and `dbsize_hook` for sizes | **High:** 18 adds `smgr_startreadv`, `smgr_maxcombine` and `smgr_fd` (§4.2) | Medium: the smgr API is unchanged from 18 (verify), but hint-bit and write-out changes touch the write path (§5.4) |
| Prefetch and LFC | Prefetching; EXPLAIN for prefetch and LFC | High: the read stream replaces the old prefetch call sites | Medium: read-ahead scheduling changed (§5.4) |
| WAL contents | `t_cid` in heap records; marking unlogged index builds; WAL-logging an all-zeros page as a hole; WAL-logged relmapper, replication slots and snapshots | Medium: `xl_heap_inplace` changed (§4.1) | **High:** prune records widened, `XLOG_HEAP2_VISIBLE` removed, VM block references, the multixact width (§5) |
| LSN tracking and startup | Last-written page LSN; startup without the checkpoint record; `XLogWaitForReplayOf()` | Medium | Medium: the checkpoint record and `XLOG_CHECKPOINT_REDO` changed (§5.3) |
| WAL redo | Replaying only the target page; `predefined_sysidentifier`; `pg_waldump` flags; SLRU on-demand download | Low | **High:** multixact SLRU and records (§5.1) |
| Compute behaviour | Disabled sequence caching; backpressure; walproposer shutdown order; on-demand extensions; publication superuser checks | Low | Low |

## 4. Postgres 18: what changes, and where

### 4.1 WAL, control file and SLRU

These constants were read from each branch's headers:

| Constant | 16 | 17 | 18 | 19 (`REL_19_STABLE`, RC1) |
|---|---|---|---|---|
| `XLOG_PAGE_MAGIC` | `0xD113` | `0xD116` | `0xD118` | `0xD122` |
| `PG_CONTROL_VERSION` | 1300 | 1700 | 1800 | 1905 |
| `CATALOG_VERSION_NO` | 202307071 | 202406281 | 202506291 | 202610061 (moves until GA) |

Each change is classified as in §51 §8.3.

| Change at 18 | Upstream | Classification | Work |
|---|---|---|---|
| New `XLOG_PAGE_MAGIC` `0xD118` | — | decoder | A new arm in `wal_decoder`, `postgres_ffi` and `loams-wal-decoder`. An unknown magic stays an error (`unknown_page_magic_is_error`) |
| `xl_xact_stats_item` grows from 12 to 16 bytes (the stats objid is 64-bit, stored as two `uint32`) | `b14e9ce7d5` | **decoder** | Commit and abort parsing walks the `xinfo` sections in order. A wrong item size shifts every later section: two-phase GID, origin and dropped relations. Without this change, dropped relations would be **silently missed** |
| `xl_heap_inplace` carries `dbId`, `tsId`, `relcacheInitFileInval`, `nmsgs` and invalidation messages (`MinSizeOfHeapInplace`) | `243e9b40f1` | decoder, redo | The decoder treats it as a block record (verify that nothing reads past the fixed part). Neon's `t_cid` patch does not cover in-place updates (verify) |
| Control file: `default_char_signedness` | `44fe30fdab` | walingest, basebackup | The bindgen struct changes. The pageserver keeps initdb's control file and rewrites the checkpoint fields, so the layout must come from v18's headers. `pg_upgrade --set-char-signedness` matters for Path 3 (§10) |
| rmgr list | unchanged (22 rmgrs) | none | — |
| SLRU on disk | no format change found (64-bit page numbers and long segment names came in 17) | none (verify) | — |
| Heap, btree and multixact records | no other format change | none | `decode_all_rmgrs_pg18` proves it |
| Heap records register the visibility-map buffer and can carry VM full-page images (18.6's back-patch `f581fa729d` of 17.11's `c0d9864f`; found by NF2 Task 1, item F27) | `f581fa729d` | redo | **Reverted in `loams/pg18`** (D1112 as amended 2026-10-29, owner: "Yes, revert at 18"), in `heapam.c` and `heapam_xlog.c` with the `pg_walsummary` and `pg_combinebackup` test changes, as at 17.11. `test_loams_vm_clear` runs at 18 |

### 4.2 Asynchronous I/O and Neon's smgr (D1110)

**What 18 does.** Reads through shared buffers go through the read-stream API and `StartReadBuffers`. Those call the new `smgrstartreadv(PgAioHandle *, ...)`, which md.c implements by handing a file descriptor (`smgr_fd`) to the AIO subsystem. `io_method` is `worker` (the default), `io_uring` or `sync`. `io_combine_limit` and `io_max_combine_limit` size combined reads, and `smgr_maxcombine` tells the reader how many blocks one call may cover. AIO covers reads only.

**Why it is the hardest item.** Neon's smgr has no file descriptors. It fetches pages over the communicator, with its own prefetch queue and the LFC. AIO completions run in AIO workers or in critical sections that hold interrupts, and a page-service request cannot safely wait there. A community port of Neon to 18 hit exactly this. Neon's own service still runs 18 with `io_method = 'sync'`, because its sequential-scan and prefetch code "isn't yet optimized for Postgres 18".

**The ruling.**
1. `compute_ctl` writes `io_method = sync` into every compute's configuration at 18 and later, and marks it a setting that users cannot override in the compute spec.
2. `pgxn/neon` implements:
   - `smgr_startreadv`, which issues the reads through the existing prefetch path (`communicator_prefetch_register_bufferv`), waits for them, then completes the handle with the result, with no file descriptor involved;
   - `smgr_maxcombine`, which returns the prefetch window, capped by `io_combine_limit`;
   - `smgr_fd`, which reports "no descriptor" so that no AIO method tries one.

   Whether `sync` with a descriptor-less target is supported by 18's AIO core without a patch is the first spike of NF2 Task 3. If it is not, the patch is ours, with an id.
3. Tests:
   - G2's LFC and prefetch tests at 18;
   - `read_stream_seqscan_pg18`: a 10 GB sequential scan returns the right checksum, and its prefetch hit rate is recorded;
   - `io_method_locked`: a spec that sets `io_method` is refused.

**Cost.** Sequential scans at 18 lose AIO's overlap, but keep Neon's prefetch, which already overlaps page-service round trips. So 18 performs about like 17 (estimate; measured in Task 3). A native AIO backend (an AIO target whose completions come from the communicator) is later work, NF3, started only if the measurements say it pays.

### 4.3 Data checksums (D1111)

- **Today.** `libs/postgres_initdb` runs `initdb` without `--data-checksums`, so every Loams Postgres timeline has checksums off. Nothing in `pgxn/neon`, `neon_walredo` or the pageserver sets or checks page checksums. Full-page images in WAL are copied from shared buffers, where the checksum field is stale.
- **At 18,** `initdb` turns checksums on by default. Bootstrapping 18 timelines unchanged would make every page the pageserver serves fail verification in the compute.
- **The ruling.**
  - `postgres_initdb` passes `--no-data-checksums` from 18 on.
  - `fast_import pgdata` creates its target cluster the same way.
  - `import_datadir` and `ImportPgdata` refuse a data directory with checksums on, with the reason `import_checksums_enabled`. A directory that came from elsewhere is converted by the importer (`pg_checksums --disable`, offline) before upload.
  - `pg_upgrade` requires both clusters to match, so this also keeps Path 3 open (§10).
- **Later.** Checksums would need the WAL-redo process, image layer creation and basebackup to set them, and the compute to keep them on full-page writes. This is a separate design. It has no date.

### 4.4 Protocol 3.2, OAuth and authentication

- **Protocol 3.2** (256-bit cancel keys) is offered by 18's server. libpq still asks for 3.0 unless `max_protocol_version` is set. §58 §3 already rules that the gateway accepts 3.2 from clients and talks 3.0 to PgDog until PgDog is verified, issuing its own 32-byte cancel keys. NF2 Task 12 runs §51 §12.4's PgDog checks at 18. The other places 3.2 must work are:
  - the compute's pgbouncer (verify that its pinned version negotiates 3.2 or falls back cleanly);
  - `local_proxy`, which is frozen and only parses startup and cancel messages (verify the key length).
- **OAuth** (`oauth` in `pg_hba`, `oauth_validator_libraries`, libpq built with libcurl). Loams already exchanges OIDC and agent tokens at PgDog (§46 §8). A Loams validator module inside the compute would put token validation in two places. In NF2:
  - the compute is built **without** `--with-libcurl`;
  - `compute_ctl` never writes an `oauth` line;
  - `oauth_validator_libraries` stays empty.

  A validator module (`pgxn/loams_oauth`) is reconsidered when the gateway's `http` and `mcp` plans need direct compute auth (Q957).
- **MD5:** 18 warns and D711 already refuses it. No change.

### 4.5 SQL-visible changes that touch Loams

| Change | Where it matters | Work |
|---|---|---|
| Virtual generated columns (now the default kind) | PgDog's parser (routing); HTAP phase 1's pgoutput bridge (virtual columns are never replicated; stored ones only with `publish_generated_columns`) | The PgDog inventory row; an HTAP test that a virtual column is computed on the lake side or omitted (§58 §6.2) |
| `RETURNING OLD/NEW`, `WITHOUT OVERLAPS` and `PERIOD` | PgDog routing | Inventory rows (§51 §12.4) |
| `NOT NULL` constraints in `pg_constraint` | `loams-postgres` and the console read the catalog for schema views | Catalog queries tested at 18 |
| `uuidv7()` built in | The extension catalogue: `pg_uuidv7` is dropped from 18 on (§51 §7.1, unchanged) | — |
| `EXPLAIN ANALYZE` shows `BUFFERS` by default | Console and Studio plan viewers | Viewer fixture at 18 |
| `pg_stat_io` byte columns and WAL rows; `track_wal_io_timing` | The compute exporter's queries | The exporter's queries per major |

### 4.6 Logical replication and decoding at 18

- New settings: `idle_replication_slot_timeout`, `max_active_replication_origins`, `streaming = parallel` by default, `two_phase` changeable by `ALTER SUBSCRIPTION`, conflict statistics, and generated columns in publications.
- **Neon's patches WAL-log replication slots and snapshots,** so the pageserver keeps a slot's state across compute restarts. `idle_replication_slot_timeout` invalidates a slot, and that invalidation must persist across a compute move too. A G2 test covers it (`slot_invalidation_survives_restart_pg18`).
- `compute_ctl`'s logical-replication settings keep their 17 semantics. `wal2json` 2.7 (2026-10-09) is the first release that supports 18 and 19. pgoutput is unchanged for HTAP phase 1.

### 4.7 `compute_ctl`, the compute image and extensions

- **`compute_ctl`** gains per-major configuration templates. At 18 they set `io_method = sync` (§4.2) and leave `md5_password_warnings` at its default. Its `pg_hba` writer ignores the new `oauth` method. `output_plugin_libraries` carries over unchanged from 17.11's release.
- **`loams-postgres-compute-v18`** is built on Debian 13 `trixie` (Q727's default, unchanged). The `loams-postgres-storage` image gains `/usr/local/v18` (and later `/usr/local/v19`) for WAL redo and `initdb`, and loses v14 and v15 after 2026-11-12 (D1117).
- **Extensions at 18.** Each is listed with the version that first supports 18, checked 2026-10-18; Task 9 rechecks each one against the 14-day rule.

| Extension (tier) | Version that supports 18 | 19 |
|---|---|---|
| pgvector (core) | 0.8.1 (2025-09-04); latest 0.8.7 (2026-10-01) | Not announced (verify) |
| PostGIS (core) | 3.6.x (3.6.1, 2025-11-12) | 3.7.0rc2 only (2026-09-08) |
| wal2json (core) | 2.7 (2026-10-09; the 14-day rule allows it from 2026-10-23) | 2.7 |
| pg_cron (core) | 1.6.6 (2025-09-04) | 1.6.8 (2026-09-08) |
| pg_partman (core) | 5.3.0 (2025-10-09); 5.5.0 changes the background worker's role default | Not confirmed |
| pgaudit (core) | 18.0 (2025-09-24) | `19beta4` tag only |
| pg_hint_plan (core) | `REL18_1_8_1` (2026-10-15) | `REL19_1_9_0` (2026-10-15) |
| hypopg (core) | 1.4.2 | 1.4.3 |
| pg_repack (core) | 1.5.3 (2025-10-26) | Merged upstream, not released |
| pg_ivm (core) | 1.12 | 1.16 or later |
| pg_session_jwt (core, our fork) | Needs **pgrx 0.16** (from 0.12); our patch | Needs **pgrx 0.19** |
| TimescaleDB, Apache-only (included) | 2.23.0 (2025-10-29) | **None:** 2.30.2 supports 16–18 |
| plv8 (included) | 3.2.4 | 3.2.5 (verify the date) |
| Other pgrx extensions: `pg_jsonschema`, `pg_graphql`, `pg_tiktoken`, `pgx_ulid` (included) | pgrx 0.16 releases (verify each) | pgrx 0.19 (supports 19's betas; verify for GA) |

A `core` extension that lacks a release for a major blocks that major's promotion, not its beta (§51 §7.3). For 19, that means pgvector, PostGIS 3.7.0 and pgaudit 19.0 must ship before Loams 19 is supported. The `included` extensions that lag (TimescaleDB at 19) are listed in the release notes as "not yet at 19".

## 5. Postgres 19: what changes, and where

19's WAL format moved from `0xD118` to `0xD122` in nine steps. The ones that touch the pageserver's ingest, the decoder or WAL redo are below (commits on `REL_19_STABLE`; re-read at GA and at 19.1).

### 5.1 Multixacts: 64-bit offsets

- **Upstream** (`bd8d9c9bdf`, 2025-12-09, "Widen MultiXactOffset to 64 bits"):
  - `MultiXactOffset` is `uint64`, so `xl_multixact_create.moff` is 64-bit;
  - the `pg_multixact/members` SLRU uses long segment names (offsets does not);
  - `pg_upgrade` rewrites the multixact files.
- **Also in 19:**
  - `xl_multixact_truncate` is restructured into `oldestMultiDB`, `oldestMulti` and `oldestOffset` (`c4b71e6f60`);
  - `pg_upgrade` checks that oldestXID and oldestMulti are consistent (`5cd84ed627`);
  - SLRUs are allocated through the new shared-memory request API (`2e0943a859`);
  - the control file gains `slru_pages_per_segment` (`3e0ae46d90`).
- **Where it lands:**
  - `walingest.rs`'s multixact create and truncate paths, including Task 17's `MultixactOffsetCreatePair` (the next-multixid offset fix, which 19 also has, `789d65364c`);
  - the SLRU key layout for members segments;
  - `neon_walredo`'s SLRU redo;
  - `basebackup.rs`, which writes SLRU segments under their 19 names;
  - the decoder.
- **Risk: high.** A width or naming mistake corrupts multixact members, which surfaces as row-lock and `FOR SHARE` errors long after the write. Tests:
  - `multixact_wraparound_pg19`: pgbench with `SELECT ... FOR SHARE` from many sessions until the offsets pass 2^32, with `vacuum_multixact_freeze_*` low, then a pageserver restart and a branch;
  - `basebackup_slru_names_pg19`, which compares a basebackup against vanilla 19's `pg_multixact` layout.

### 5.2 Heap records: prune, visibility and VM block references (D1112)

| Upstream 19 | Effect here |
|---|---|
| `xl_heap_prune.flags` widened from `uint8` to `uint16`, with `XLHP_VM_ALL_VISIBLE`/`XLHP_VM_ALL_FROZEN`; vacuum's phase III sets VM bits through the prune record (`4a8fb58671`, `add323da40`) | The decoder reads 16-bit flags at 19. The pageserver turns the new flags into the VM-set operation it now derives from `XLOG_HEAP2_VISIBLE` |
| **`XLOG_HEAP2_VISIBLE` and `xl_heap_visible` removed** (`a881cc9c7e`) | `walingest.rs`'s handler for it is kept for 14–18 and unreachable at 19. Covered by `decode_all_rmgrs_pg19` and a VM test |
| **Heap insert, delete, update and lock records register the VM buffer** as block ids 1–3 and can carry VM full-page images (`b01c31eef9`) | **Not taken** (D1112): it is 17.11's `c0d9864f` again, in the main line. The fork's heap records are `RM_NEON_ID` (`xl_neon_heap_*`). The decoder takes an update's old block from `decoded.blocks.last()`, so an extra VM block would be misread. The VM clear is derived from the `*_ALL_VISIBLE_CLEARED` flags, and computes WAL-log every VM page they evict. `test_loams_vm_clear` moves to 19 and asserts that no heap record registers a VM block |
| `XLH_DELETE_NO_LOGICAL` for `REPACK CONCURRENTLY` (`28d534e2ae`) | A flag the decoder passes through. A G2 test runs `REPACK CONCURRENTLY` across a branch |
| `XLP_BKP_REMOVABLE` page-header flag removed (`f68d7e7483`) | `postgres_ffi`'s page-header constants per major |

Upstream's tests for the VM registration (the 19 equivalents of `pg_walsummary` `002_blocks.pl` and `pg_combinebackup` `012_vm_consistency.pl`) are reverted with it, as at 17.11. The 19 port records each revert in `LOAMS_PATCHES.md`.

### 5.3 Checkpoint, logical decoding and generic records

| Upstream 19 | Effect here |
|---|---|
| `XLOG_CHECKPOINT_REDO` carries `xl_checkpoint_redo { int wal_level }` (`097ab69d17`) | The pageserver parses checkpoint records and keeps `CheckPoint` under its own key. The 19 struct comes from bindgen |
| `CheckPoint` gains `logicalDecodingEnabled`; a new record `XLOG_LOGICAL_DECODING_STATUS_CHANGE` (0xF0) (`67c20979ce`) | The pageserver keeps the flag in its stored checkpoint, so a compute started from a basebackup sees the right state |
| `wal_level = replica` with logical decoding switched on when the first logical slot is created (`effective_wal_level`) | Neon computes turn on logical replication with `wal_level = logical` and a restart. At 19 the restart could go, but only once the slot WAL-logging patch and the status record work together. Phase B keeps the explicit setting; switching to the automatic mode is Q958 |
| `XLOG_GIST_ASSIGN_LSN` becomes the generic `XLOG_ASSIGN_LSN` (0xC0) (`d774072f00`) | Decoder arm |
| Database-specific logical snapshots (`0d3dba38c7`) | Reverted upstream (`01a80f0621`); nothing to do. Re-check at GA |

### 5.4 Compute-side changes at 19

- **Write-out.** Buffers are no longer copied during write-out, because hint bits are not set during I/O (`41d3d64e87`). `pgxn/neon`'s write path (`neon_wallog_page`) and Neon's proposed "don't mark pages dirty on hint bits" patch meet here. Risk: medium.
- **Asynchronous I/O.** I/O workers scale automatically (`io_min_workers`, `io_max_workers`), and read-ahead scheduling changed. With `io_method = sync` (D1110), this affects only how the read stream sizes requests, which `smgr_maxcombine` caps.
- **Defaults.** JIT is off, `max_locks_per_transaction` is 128, `log_lock_waits` is on, `standard_conforming_strings` is forced on, TOAST compression is `lz4` when built with lz4, and `wal_compression = on` picks the best method. `compute_ctl`'s 19 template keeps upstream's defaults. The compute is built with lz4 and zstd.
- **Gone at 19:** RADIUS and `MULE_INTERNAL`. `get_relation_info_hook` is replaced by `build_simple_rel_hook`, and index access methods use a static `IndexAmRoutine`. These break C extensions that have not been updated, which is why `core` extensions gate promotion.
- **New SQL:**
  - `WAIT` (read-your-writes on standbys): for Loams read replicas, which replay WAL from `loams-wal`, it must wait on the replica's replay LSN. A test covers it on a read-only endpoint.
  - `REPACK`.
  - Parallel autovacuum.
  - Sequence replication (`ALL SEQUENCES`).
  - Publication `EXCEPT`.
  - `retain_dead_tuples`.
- **Plan advice.** The contrib modules `pg_plan_advice` and `pg_stash_advice` arrive at 19. §58 D1040 already moves DA1b's plan pins to them on 19 (DA1 Task B6, now gated on NF2b instead of NF1f).
- **Protocol.** There is no new protocol version: `PG_PROTOCOL_LATEST` stays 3.2 (3.1 is reserved). Grease (`3.9999`, `_pq_.test_protocol_negotiation`) was sent only during beta. There is no `GoAway` message, which matches §58 Q914's "PG20 commitfest".
- **SNI on the server** (`pg_hosts.conf`). Loams terminates or splices SNI at `loams-gateway` (§57) and leaves it off in the compute.
- **`btree_gist` `inet`/`cidr` indexes block `pg_upgrade` to 19.** §10's pre-check finds them and asks the user to rebuild them as GiST `inet_ops`.

### 5.5 What 19 did not change

- **Online data-checksum enable and disable** was committed and then reverted from 19 (`c05d5ce123`, 2026-09-16). D1111 is unaffected. Whether `data_checksum_version_init` is still in 19's control file is checked in the format diff (verify).
- The smgr API is unchanged from 18 (verify), and no page-format change was found.

## 6. Phases

| Phase | What | Starts | Ends (estimate, at D824's staffing of about 1.25 engineers) |
|---|---|---|---|
| **NF2a: Postgres 18** | The format diff, `loams/pg18`, the Rust side, `pgxn/neon` with D1110, `compute_ctl`, `loams-postgres-compute-v18`, Loams at 18, the PgDog checks, Path 1 for 17 → 18 | Now, alongside the 2026-11-12 minors (17.12, 16.16, 18.7) | **18 beta** in about 2026-12; **18 supported** in about 2027-01 to 2027-02, by D1113 |
| **NF2b: Postgres 19** | Track `REL_19_STABLE` nightly, vanilla (now); the 18 → 19 format diff at GA (2026-11-19); `loams/pg19` ported from `loams/pg18` once 18 is in beta; the Rust side and the compute; Loams behind `allow_beta_major`; Path 1 for 18 → 19 | The diff now; the port after the 18 beta | **19 beta** in about 2027-02; **19 supported no earlier than 2027-03-13** (19.1 on 2027-02-11 plus the 30-day soak) |
| **NF2c: Upgrades and policy** | `UpgradeProject` by branch (Path 1); the Path 3 spike; the console, CLI and desktop prompts; runbooks; 14 and 15 removed; the support-window automation | With NF2a's Path 1 | Path 3 ruled on after the 18 promotion |

**What to do first (in order):**
1. **Write `docs/loams/pg18-format-diff.md`** in the fork from §4.1, with every header in §51 §8.3's list classified. This starts the moment the plan is approved, needs no build, and turns the risks in §4 into tests.
2. **Create `loams/pg18` at `REL_18_6`** through the REST API, and cherry-pick the 17 series in `core_changes.md` order, in CI (`loams-postgres-merge.yml` gains a `port` mode). Run `make check` on vanilla-plus-patches before any Rust work.
3. **The AIO spike** (§4.2): is `smgr_startreadv` with `io_method = sync` workable without a core patch?
4. **The Rust side**, in order: `PgMajorVersion::PG18`, `postgres_ffi` bindgen and constants, `wal_decoder` (the `xl_xact_stats_item` size first), then `walingest.rs`, basebackup and WAL redo. Use the `wal_craft` fixtures at 18.
5. **Ingest tests before any compute work:** `decode_all_rmgrs_pg18`, then `pgbench_checksums_survive_restart_and_branch_pg18` on vanilla 18 WAL fed through the decoder.
6. Then `pgxn/neon`, `compute_ctl`, the image, and Loams.

Beside these, starting now: the vanilla nightly build of `REL_19_STABLE` and the 18 → 19 diff draft, because 19's ingest changes (§5.1–§5.3) are bigger than 18's.

## 7. Policy going forward

### 7.1 The support window (D1108)

| State | New projects | Minors | Images | Exit |
|---|---|---|---|---|
| **beta** | Only with `allow_beta_major` | Every minor (or RC) within 7 days | `-beta` tags, amd64 only (Q729) | Promotion (§7.2), or removal if the port is abandoned |
| **supported** (at most 3) | Yes; one of them is `default` | Within 7 days | All tiers, amd64 and arm64 | To maintenance when a fourth major is promoted |
| **maintenance** (at most 1) | No | Within 7 days, until upstream EOL | Compute image kept for export and upgrade | Removed at upstream EOL. The last 3 months before EOL show an "upgrade before" notice, and the console and CLI warn on every connect for the last 30 days |

**Applied:**

| From | supported | default | maintenance | beta |
|---|---|---|---|---|
| 0.1.0 (`lp-2026.11.0`) | 17 (16 after Q949) | 17 | 16 (none after Q949) | — |
| 18 beta (about 2026-12) | 16, 17 | 17 | — | 18 |
| 18 supported (about 2027-01 to 02) | 16, 17, 18 | 17 for one quarter, then 18 (Q954) | — | — |
| 19 beta (about 2027-02) | 16, 17, 18 | 18 | — | 19 |
| 19 supported (no earlier than 2027-03-13) | 17, 18, 19 | 18 for one quarter, then 19 (Q954) | 16 (to 2028-11-09) | — |
| 20 (upstream about 2027-09 to 11) | Same rules: beta from Loams' port, supported after 20.1 (about 2028-02) | | 16 leaves at its EOL | 20 |

The build carries at most five majors at once (three supported, one maintenance, one beta). The storage image's WAL-redo binaries and G2's legs grow with that number.

### 7.2 Promotion (D1113)

A major moves from beta to supported only when all of the following hold:
1. Upstream has released x.1 or later, and it is merged and gated.
2. §8's matrix is green twice, on different days, on the release candidate's digests.
3. The beta has run 30 days on Loams' test cluster with no data-loss or corruption bug open against it.
4. Path 1 from the previous supported major is green (`upgrade_<prev>_to_<N>_preserves_checksums`).
5. PgDog's inventory at the major has no `error` or `differs` row in a component Loams uses (§51 §12.4).
6. Every `core` extension builds and passes its smoke at the major.

`default` moves to the newest supported major one quarter after its promotion (Q954). That is the next fork release on or after the following quarterly minor, so a new default never ships in the same release as its own promotion.

### 7.3 Minor releases (D1117)

- **Target.** Every Postgres minor, quarterly or out of cycle, ships in `loams-postgres-compute-v<N>` and `loams-postgres-storage` **within 7 days** for every supported, maintenance and beta major. Security minors keep the same 7 days (D805), and the other minors adopt it. A minor that misses it escalates to the owner (§51 §13.3).
- **What runs by itself.** This uses the `LOAMS_BOT_TOKEN` organisation secret.
  1. Each night, `loams-postgres-minors.yml` (NF1 Task 18, not built yet; it must land before the 2026-11-12 minors, and NF2 Task 15 extends it to beta majors and the release dispatch) try-merges `postgres/postgres` `REL_<N>_STABLE` into `loams/pg<N>` for each major in the pin file, keeps a draft PR updated, and opens a conflict issue with the files.
  2. When `REL_<N>_<m>` is tagged, the job runs `loams-postgres-merge.yml` (`push`), opens the release issue with its due date (`sla.py`), and, once the merge PR is green, dispatches `loams-release.yml`.
  3. `loams-release.yml` runs G0–G2, builds, runs G3 on the candidate's pin document, tags, signs, and opens the pin PR on `ostrium-labs/loams`.
- **What a person does.**
  - A maintainer resolves conflicts in Neon's patched areas on `loams/minor-pg<N>-<tag>`. Expected effort is 0.5–2 days per major per minor (estimate, from Task 17: 5 to 10 conflicted files per major).
  - A WAL-format change like 17.11's VM registration is never settled inside a merge. It goes to the owner as a ruling (as option (a) did).
  - The maintainer approves the Loams pin PR, which merges on a green `CI required`.
- **An owner action that blocks this.** The pin PR job needs `LOAMS_BOT_TOKEN` to have the `workflow` permission on `ostrium-labs/loams`, because `pins.py` rewrites workflow files. Without it, `lp-2026.11.0`'s pin PR had to be opened by hand (#774).
- **Quarterly dates.** 2026-11-12 (17.12, 16.16, 18.7, and 14's last minor), then 2027-02-11 (with 19.1), 2027-05-13 and 2027-08-12. `lp-2026.11.1` is due by 2026-11-19.
- **14 and 15.** 14's last minor ships on 2026-11-12. 15 is not offered (D813). Both leave `Makefile`'s `POSTGRES_VERSIONS`, the storage image and `vendor/` in NF2 Task 16 (NF1 Task 41's cleanup, brought forward). Their branches stay as history.

## 8. The gate per major (D1118)

The tiers are NF1 Task 16's, as built: G0 lint and policy; G1 Rust tests (once, generic over majors); G2 `test_runner` per major; G3 Loams' integration. NF2 extends them as follows.

| Tier | Today | With NF2 |
|---|---|---|
| G0 | Every PR | Unchanged |
| G1 | `postgres_ffi`, `wal_decoder`, `pageserver_api`, `compute_api`, `compute_tools`, the pageserver with both I/O engines; Postgres built for every major | Adds `decode_all_rmgrs_pg<N>` and `xlog_page_magic_v<N>` for each built major, and `unknown_page_magic_is_error` |
| G2 | 21 tests per major at 16 and 17 (`gate-subset.txt`) | **One leg per major in the pin file, beta included.** Adds `test_loams_vm_clear` at every major, `test_multixact` (and `multixact_wraparound_pg19` at 19), `read_stream_seqscan_pg18`, `slot_invalidation_survives_restart_pg<N>`, and a new job, **`upgrade`**: Path 1 from each supported major to the next, with per-table checksums |
| G3 | `nf1-candidate.yml`: `pg2-e2e.yml` (on `compute-v17`), the decoder job, `candidate-smoke.sh` per major | **`pg2-e2e.yml` per major** (its kind test reads `compute-v<N>` from the candidate's majors instead of hard-coding `compute-v17`); the decoder's golden tests at every major; `it-pageserver-loams-wal.sh` per major; the desktop's local stack at the default major |

A beta major's G2 and G3 legs are **required for its own images** and **advisory for the other majors' releases**. A broken 19 beta never blocks a 17 security minor. The gate runs nightly, on every release and by hand (Q721's answer), never on pull requests.

## 9. Loams: the pg-control API, `loams-wal` and the stacks (D1114)

### 9.1 The offered majors come from the pin file

- `scripts/nf1/pins.py set` writes `crates/loams-postgres/src/majors.rs` (the generator exists as `render_rust`, but the file does not, so nothing generates it today). The file holds `SUPPORTED`, `BETA`, `MAINTENANCE` and `DEFAULT`.
- `loams-pg-control`'s `ServiceConfig::default()` reads them instead of `default_pg_version: 17` and `pg_versions: vec![17]`.
- The `--compute-image <major>=<ref>` override stays for tests.
- `runtime/images.rs` already maps `supported` majors to compute images through `include_str!` of the pin file. It also maps `beta` and `maintenance` ones.

### 9.2 API changes (`loams.postgres.v1`, additive only)

| Change | Shape | Rule |
|---|---|---|
| `CreateProjectRequest.allow_beta_major` | `bool allow_beta_major = 7;` | A beta major without it is refused with `beta_major_requires_opt_in` |
| Unsupported majors | — | `pg_version` outside `SUPPORTED ∪ BETA` is refused with **`unsupported_pg_version`** (today it is a generic `InvalidArgument` on the field). The error's details list the offered majors. A maintenance major is refused for new projects with the same reason and a hint to use the newest supported major |
| `Branch.pg_version` | `uint32 pg_version = 16;` (output only) | After an upgrade, one project has branches of two majors. `Project.pg_version` means the major of the default branch |
| `ListPgVersions` | `rpc ListPgVersions(ListPgVersionsRequest) returns (ListPgVersionsResponse)`; each entry has major, state (`SUPPORTED`, `BETA`, `MAINTENANCE`), `is_default`, minor, and EOL date | For the console, the CLI (`loams pg versions`) and the desktop. Read from `majors.rs` plus the pin file's minors |
| `UpgradeProject` | Adds `string branch_id = 4;` (empty means the default branch), `UpgradeMethod method = 5;` (`IMPORT` = Path 1, the default; `DUMP_RESTORE`; later `PG_UPGRADE` = Path 3), `bool check_only = 6;` (pre-checks only: extension availability at the target, `btree_gist` `inet` indexes at 19, reg* columns, size estimate) | Creates `<branch>-pg<N>`. The swap (`SetDefaultBranch` and the endpoint move) needs a second call. The old branch is kept 7 days, protected |
| `compute_major_mismatch_refused` | — | `ComputeSpecBuilder` refuses a compute image whose major differs from the timeline's `pg_version` (§51 §9) |

Each reason is registered in `docs/api/reasons.md`. The proto changes are additive, so `buf breaking` stays green.

### 9.3 `loams-wal`, the decoder and TiKV

- **`loams-safekeeper`/`loams-wal`** is already version-agnostic. It stores the full `pg_version` in `ServerInfo` per timeline and refuses a greeting whose major differs from the stored one. Tests `greeting_accepts_pg18` and `greeting_accepts_pg19` pin this down.
  - An upgrade creates a **new** `loams-wal` timeline at the new root timeline's start LSN, never a major change on an old one.
  - Path 1 needs `loams-wal` to accept a timeline whose first LSN is the import's end LSN (verify, as §51 §9 says; R2.10's idempotent create suggests it does).
- **`loams-wal-decoder`'s `major()`** maps 14–17. NF2 adds 18 and 19. Its fork rev moves to a `loams-*`-tagged `main` commit with the 18 decoder, and the fixtures `wal-18.bin.zst`, `fork-sender-18-*.sha256` (then 19) are recorded the way 17's were.
- **The interpreted sender** runs the fork's `wal_decoder` in process (PG2 Task 31), so it follows the decoder.
- **TiKV.** D1057 keeps `loams-wal`'s votes, leases and timeline metadata in TiKV. The per-timeline record already has `pg_version`, and an upgraded branch is a new timeline record, so no TiKV schema changes. The bucket offload (`.lwal`) stores bytes and is not affected by the major.
- **HTAP phase 3** (§58 D1054) decodes the WAL tail through `loams-wal-decoder`, so it is available at a major only once the decoder supports it.

### 9.4 The stacks and the desktop

- `deploy/loams-postgres-dev/compose.yaml` gains one `compute-v<N>` service per major in the pin file, generated by `pins.py` (profiles `pg<N>` and `beta`).
- `deploy/release/compose.yaml` takes its compute images from the same generator.
- The desktop's `PG_MAJOR = 17` in `stacks.ts` becomes the pin file's `default`. Its `pg_major_mismatch` guard then becomes "start the compute of that timeline's major" (NF1 Task 39, unchanged, which stays in NF1).

## 10. Upgrades between majors (D1115)

Two rules are unchanged: data is never upgraded without the user's action, and a compute never attaches to another major's timeline.

| Path | How | Downtime | Disk | Risk | When |
|---|---|---|---|---|---|
| **1. Import** (D815) | Read-only endpoint on the source → `fast_import pgdata` from the **target** major's compute → `ImportPgdata` into a new root timeline of the same tenant → a `loams-wal` timeline at the end LSN → branch `<name>-pg<N>` | Writes frozen for the dump's length, or none with a logical-replication catch-up (later) | The bucket only | Low: a logical copy, checked by checksums | **NF2a, with 18** |
| 2. `pg_upgrade --link` on a materialised data directory (NF1 Task 38) | Basebackup at a quiesced LSN → local data directory → `pg_upgrade` → upload → import | About Path 3's, plus the copy | **The database's size on local disk** | Medium | **Dropped** in favour of Path 3 (Q955) |
| **3. `pg_upgrade` on a new timeline** (Neon's open RFC 039, `docs/rfcs/039-single-click-pg-upgrade.md` in neondatabase/neon#9060, never merged) | (a) Quiesce the source at X. (b) `pg_upgrade --check`, then `pg_dump --binary-upgrade` of the catalog from an old-major compute. (c) Restore it into a scratch new-major cluster made with `initdb --no-data-checksums`. (d) Import **only the catalog and the converted SLRUs** as a new timeline that branches from the source at S = MAXALIGN(X + 1). (e) User relations keep their relfilenodes (`pg_upgrade` preserves them), so the new timeline reads their pages from the ancestor | The catalog's restore, independent of data size (estimate: seconds to minutes) | The catalog only | **High**: see below | A spike after 18 is promoted |

**What Path 3 needs that the pageserver does not have:**
- **WAL redo by the record's major, not the timeline's.** Pages below S are reconstructed with the ancestor's WAL-redo binary. Today `walredo.rs` picks the process from the timeline's `pg_version`. The RFC's prototype claims this works. We verify it with a test that reads, at the new major, a page whose last record is old-major WAL.
- **An import that keeps the parent's history.** `ImportPgdata` creates root timelines today. Path 3 needs an "import into a child at S" mode that skips non-catalog relations.
- **SLRU conversion:** `pg_xact` is copied, and at 19 the multixact files must be rewritten to the 64-bit layout (§5.1), so Path 3 to 19 must import the rewritten SLRUs.
- **The relation size cache and `pg_control`** of the new timeline, set at S.
- **The LSN gap** between X and the restore's own LSNs, which the RFC leaves to "the storage team".

If the spike passes, Path 3 becomes `UpgradeMethod::PG_UPGRADE` in a follow-up plan. Otherwise Path 1 stays the only path and the spike's findings are recorded.

**Pre-checks for every path** (`check_only`):
- every installed extension exists at the target major in the compute image;
- 19's `btree_gist` `inet` indexes;
- `reg*` columns and other `pg_upgrade` blockers;
- the estimated time from the logical size.

**After every path:**
- `ANALYZE` (18's `pg_upgrade` carries statistics over, but Path 1 does not);
- per-table checksums against the source at X, before the swap is offered.

## 11. Amendments to earlier decisions

| Earlier | Change | By |
|---|---|---|
| D813 (16 maintenance; 18 after NF1e; 19 beta, GA 2026-10-29) | 16 supported until 19 is supported, then maintenance. 19's upstream GA is 2026-11-19, and Loams calls 19 supported no earlier than 19.1 plus the soak | D1108, D1113 |
| Q718 (19 GA in Loams) | Refined by D1113's six conditions | D1113 |
| Q725 (the `pg_upgrade --link` spike) | The spike is Path 3, not Path 2 | D1115 |
| D815 (Path 1, then Path 2) | Path 1 first; Path 3 replaces Path 2 | D1115 |
| D805 (7 days for security minors) | 7 days for every minor | D1117 |
| NF1 Tasks 24–38 | Moved to NF2; Tasks 28 and 29's `preview` list removed | D1107, D1116 |
| NF1 Task 15 (`loams/REL_18_STABLE` from `REL_18_STABLE_neon`) | Already retired by Task 17's recut; the base is `REL_18_6` | D1109 |
| §46 D716, PG2 Task 51 | `UpgradeProject` is branch-scoped with a method; Path 1 first; dump and restore is the fallback method | D1114, D1115 |
| §58 D1040, DA1 Task B6 | `pg_plan_advice` waits on NF2b's 19 images instead of NF1f's | D1107 |
| §54 §9.3 (D945) | Unchanged: `loams upgrade` never changes a major, and the pin file's `default` moves only by a fork release one quarter after a promotion | — |

## 12. Effort and risk

Engineer-weeks (estimate). §51 §13 had NF1e at 4–7 and NF1f at 5–9. Both are re-estimated here because no Neon storage code for 18 is public, and 19's ingest changes are bigger than 18's.

| # | Item | Major | Effort | Risk | Why |
|---|---|---|---|---|---|
| 1 | Format diff (18 now; 19 at GA, re-read at 19.1) | 18, 19 | 0.5 + 1 | Low | Reading headers, but it decides everything after |
| 2 | `loams/pg18`: port 104 commits onto `REL_18_6` | 18 | 2–3 | Medium | smgr, read stream and prefetch conflicts |
| 3 | AIO: `smgr_startreadv`, `maxcombine` and `fd` with `io_method = sync` | 18 | 1.5–3 | **High** | Interrupt-holding completions; no public reference implementation |
| 4 | `postgres_versioninfo`, `postgres_ffi` (bindgen, constants, control file) | 18, 19 | 0.5 + 0.5 | Low | Mechanical, plus the `from_full_pg_version` panic |
| 5 | `wal_decoder` and `walingest.rs` (xact stats item, inplace, control file) | 18 | 1–1.5 | Medium | A silent misparse of dropped relations if missed |
| 6 | `wal_decoder` and `walingest.rs` (prune flags, `HEAP2_VISIBLE` gone, checkpoint redo, logical status) | 19 | 1.5–2.5 | **High** | VM state is derived, not logged; a mistake loses visibility information |
| 7 | 64-bit multixact offsets: ingest, SLRU keys, redo, basebackup | 19 | 1.5–2.5 | **High** | Corruption surfaces late |
| 8 | `loams/pg19`: port the 18 series; revert `b01c31eef9` (D1112) | 19 | 2–3 | Medium | Heap code moved again |
| 9 | `pgxn/neon`, `neon_rmgr`, `neon_walredo`, the walproposer | 18, 19 | 1.5–2 + 1–1.5 | Medium | Version conditionals in 32 places in the smgr alone |
| 10 | `compute_ctl` templates, compute images (trixie), the extension matrix and pgrx 0.16 and 0.19 for `pg_session_jwt` | 18, 19 | 1.5 + 1 | Medium | pgrx jumps two minor series; heavy image builds |
| 11 | Loams: `majors.rs`, the API (§9.2), the decoder, fixtures, stacks, CI matrices | 18, 19 | 1.5 + 0.5 | Low | Additive, well-tested surfaces |
| 12 | PgDog per major, protocol 3.2, cancel keys | 18, 19 | 0.5 + 0.5 | Medium | PgDog is unmodified (D708); a gap needs a workaround |
| 13 | Path 1 (`fast_import` and `ImportPgdata`), `UpgradeProject` by branch | 17→18 | 2–3 | Medium | Import at RustFS endpoints (verify); the `loams-wal` start LSN |
| 14 | Path 3 spike | 17→18, 18→19 | 2–4 | **High** | Mixed-major WAL redo; the LSN gap; SLRU conversion at 19 |
| 15 | Minor automation (Task 18), 14 and 15 removed, runbooks | all | 1–1.5 | Low | Built pieces exist (`loams-postgres-merge.yml`) |
| | **Total** | | **NF2a 12.5–17.5, NF2b 9.5–13, NF2c 3–5.5: about 25–36 engineer-weeks** | | Was 9–16 for NF1e and NF1f |

**At D824's 1.25 engineers,** NF2a alone is about 10–14 calendar weeks. That pushes "18 supported" toward 2027-02 rather than 2027-01. §6's dates assume NF2a starts in the week of 2026-10-19 and that the 2026-11-12 minors take no more than a week. The ongoing load (§51 §13.2) grows by one major's minors a year (3 to 4 majors).

**The main risks:**

| # | Risk | Mitigation |
|---|---|---|
| 1 | The AIO integration does not work with `sync` and needs a core patch in a sensitive area | Task 3 is a spike before any other compute work. A small core patch (bypass AIO for descriptor-less smgrs) is acceptable as an `LP-` patch with a drop condition |
| 2 | A 19 ingest change (VM, multixact) is handled subtly wrongly and corrupts data only under load | The format diff first; `decode_all_rmgrs_pg19`; the wraparound test; the 30-day beta soak; checksums after restart and branch |
| 3 | `core` extensions are late for 19 (pgvector, PostGIS 3.7, pgaudit 19.0) | They gate promotion, not beta. The beta builds without them, and the release notes say so |
| 4 | Staffing: three majors' minors plus two ports in one season | MVM triggers (§51 §13.3); NF2b starts only after the 18 beta; Path 3 is optional |
| 5 | Users expect 19 at upstream's GA | Every Loams document and the console say "beta" until D1113's conditions hold; `ListPgVersions` reports the state |

## 13. Open questions (Q949–Q959)

| # | Question | Default |
|---|---|---|
| Q949 | Should 16 accept new projects again (supported, not default) instead of maintenance only? | **Yes.** The owner named 16, its images and G2 legs already exist, and it helps users importing from managed 16. It moves to maintenance when 19 is supported, and leaves at EOL (2028-11-09) |
| Q950 | The base of `loams/pg18`: `REL_18_6` plus our 17 series ported, or Neon's `REL_18_STABLE_neon` (18.2)? | **`REL_18_6` plus our series**, with Neon's branch as a reference only (D1109) |
| Q951 | AIO at 18 and 19: `io_method = sync`, locked, with the smgr completing reads synchronously, and native AIO only after measurement? | **Yes** (D1110), as Neon's service does |
| Q952 | Data checksums: off at bootstrap from 18 on (`--no-data-checksums`), with imports normalised, and checksums a separate later design? | **Yes** (D1111) |
| Q953 | 19's heap VM block references (`b01c31eef9`): keep the fork's format, as option (a) at 17.11? | **Yes** (D1112), and at 18 too (owner, 2026-10-29: "Yes, revert at 18"). The other choice changes `neon_rmgr`, the decoder, ingest and every fixture |
| Q954 | When does `default` move to a newly supported major: at promotion (Q717's default), or one quarter later? | **One quarter later.** 18 becomes default with the first fork release after the minor that follows its promotion. Amends Q717's default; 18 is still the default for new projects before 19 is |
| Q955 | Upgrade paths: Path 1 first, Path 3 (`pg_upgrade` on a new timeline) as the spike, Path 2 dropped? | **Yes** (D1115) |
| Q956 | Minors: 7 days for every minor, not only security ones; the bot automates merge, gate, release and pin PR; a maintainer resolves conflicts and approves? And will the owner give `LOAMS_BOT_TOKEN` the `workflow` permission on `ostrium-labs/loams`? | **Yes**, and yes (an owner action) |
| Q957 | PG18 OAuth in the compute (`oauth` in `pg_hba`, a Loams validator module)? | **Not in NF2.** Auth stays at PgDog and the gateway; revisit with the gateway's `http` and `mcp` plans |
| Q958 | At 19, let `wal_level = replica` switch logical decoding on automatically when a slot is created (no restart), instead of Neon's explicit `wal_level = logical`? | **Not at 19's beta.** Turn it on in a later fork release once the pageserver ingests the status record and `slot_invalidation_survives_restart_pg19` passes |
| Q959 | The support window: at most three supported majors, one maintenance and one beta, with promotion at x.1 plus the gate and a 30-day soak? | **Yes** (D1108, D1113) |

## 14. Sources

- **Loams at `dev` `46449f66`:**
  - designs §46, §51 (all), §54 §9, §58 §3, §4 and §8;
  - the NF1 plan, Tasks 15–41 and the rulings of Tasks 15–17;
  - `proto/loams/postgres/v1/postgres.proto`;
  - `crates/loams-pg-control/src/{service/mod.rs,service/projects.rs,runtime/images.rs}`;
  - `crates/loams-wal-decoder/{Cargo.toml,src/lib.rs,tests/}`;
  - `crates/loams-safekeeper/src/{proto.rs,acceptor.rs,types.rs}`;
  - `scripts/nf1/pins.py`, `scripts/pg2/{pg-headers.sh,it-pageserver-loams-wal.sh}`;
  - `.github/workflows/{pg2.yml,pg2-e2e.yml,nf1-candidate.yml}`;
  - `apps/desktop-electron/src/main/stacks/stacks.ts`;
  - PR #774.
- **The fork at `a213ec44`** (local read-only clone `~/Documents/Ostriumlabs/neon`):
  - `vendor/revisions.json`, `Makefile`, `compute/compute-node.Dockerfile`;
  - `libs/{postgres_versioninfo,postgres_ffi,postgres_initdb,wal_decoder}`;
  - `pageserver/src/{walingest.rs,basebackup.rs,walredo.rs,config.rs,http/routes.rs}`;
  - `pgxn/neon/*`;
  - `docs/core_changes.md`, `docs/loams/pg17-vm-clear.md`, `docs/loams/postgres-branches.md`.
- **`ostrium-labs/postgres` branches through `gh api`:** `loams/pg16`, `loams/pg17`, `REL_18_STABLE_neon`, `loams/REL_18_STABLE`.
- **Upstream Postgres:**
  - tags `REL_18_0`…`REL_18_6`, `REL_19_BETA1`…`REL_19_RC1`;
  - the commits cited in §4 and §5;
  - `versions.json`;
  - the 18 and 19 release notes;
  - the 19 RC1 announcement;
  - the roadmap.
- **Neon in public (read only):**
  - `neondatabase/neon` (no commit after 2026-08-31; PR #12818; issue #12945; RFC PR #9060);
  - `neondatabase/postgres` (`REL_18_STABLE_neon`; PRs #772, #822, #882);
  - neon.com's Postgres version policy and its Postgres 18 asynchronous-I/O page.
- **Extension releases** on their GitHub repositories and changelogs, read 2026-10-18 (§4.7).
