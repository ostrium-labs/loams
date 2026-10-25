# HE0 Task 11: iceberg-rust and Iceberg v3 (spike report)

Task 11 of [the HE1 plan](2026-10-21-he1-loams-house-engine.md) (phase HE0) checks design [§75](../design/75-loams-house-engine.md) §4.1 and §9 against running code. The spike did four things:

- Spark wrote Iceberg v3 tables with deletion vectors, row lineage and Variant.
- The newest iceberg-rust release, its 0.11 release candidate and its `main` branch read those tables, and the spike recorded what reads, what fails and what is missing for writes.
- DuckDB, Spark and Trino read today's House v2 tables, which gives the interop baseline.
- The report lists each gap with its upstream issue and the contribution Loams proposes (D1553, Q1171: upstream first, no long-lived fork).

The work was done on 2026-10-21. The spike's code and raw outputs are on branch [`he0-iceberg-v3-spike`](https://github.com/ostrium-labs/loams/tree/he0-iceberg-v3-spike) (`spikes/he0-iceberg-v3/`): three reader crates, each its own Cargo workspace, plus the stack scripts and `results/`. That branch is **never merged**. **Nothing was posted upstream**: §7 lists the proposed issues for the owner's consent. The rulings are R11.1–R11.6 in the plan.

## Summary

- **Deletion vectors (DVs).**
  - **iceberg-rust 0.10.1, the newest release, cannot read them.** Every Spark table with DVs fails with `Invalid Parquet file. Corrupt footer`, because 0.10.1 opens the Puffin file as a Parquet position-delete file. It fails loudly; it never returns wrong rows.
  - **0.11.0-rc.2 and `main` read DVs correctly:** the same row counts and checksums as Spark, Trino and DuckDB on all four DV tables. That includes a v2 table with position deletes that was upgraded to v3 and then deleted from again.
  - **No iceberg-rust build can write a DV.** There is no DV serializer (#3232 is an open PR) and no action that commits delete files (#2580 is an open issue, #2203 an open PR). On `main`, fast append is the only commit that adds files, and it refuses anything but data files.
- **Row lineage.**
  - 0.10.1 cannot select `_row_id` (`field not found`).
  - **0.11.0-rc.2 and `main` read `_row_id` and `_last_updated_sequence_number` exactly as Spark and Trino do**, also under a filter and on files with DVs.
  - Writes keep lineage valid: **iceberg-rust's fast append on a v3 table assigns `first-row-id` correctly** in all three builds, and Spark reads the appended rows with the right `_row_id`. An upgrade to v3 followed by an append also assigns ids to the pre-upgrade rows.
  - Still open: carrying lineage through rewrites (#3401). The House's compaction will need that.
- **Variant.**
  - 0.10.1 cannot even load a table with a Variant column: the metadata fails to parse.
  - 0.11.0-rc.2 and `main` load the table and read its other columns, but refuse to read a Variant column: `Reading variant columns is not supported yet`; PR #2880 is open.
  - Writing Variant needs the Parquet annotation (#2546, #2581, both open).
- **The REST catalog's format version (#2923).** It is confirmed in all three builds: asking for V3 at create gives a V2 table. Lakekeeper honours `properties["format-version"] = "3"`, so the fix is on the client (PR #3055, open). The House's own REST client already sends the property.
- **Versions (answering Task 10's R10.2).**
  - The 0.11.x release branch was cut on 2026-09-03 and **stays on Arrow 58.4 and DataFusion 54**, not 59. It already holds the DV and lineage reads.
  - 0.11.0 is not released: rc.2 is blocked on licence files, and a maintainer expects nothing before November.
  - The maintainers also say 0.11.x is the last release governed by the Apache Iceberg project, and which crate versions follow is undecided.

  So HE1's DataFusion 54.1 + iceberg 0.11 on Arrow 58 gets DV reads with no DataFusion change. Arrow 59 (DataFusion 55) has no iceberg release on the calendar (§5).
- **Interop baseline.**
  - **DuckDB 1.5.6, Spark 4.0.4 (Iceberg 1.12.0) and Trino 483 all read today's House v2 tables** with the same answers as the House: counts, sums, nulls, timestamps, decimals, lists, maps and UUIDs.
  - The House writes **no delete files at all**. Mutations are refused, and `ReplacingMergeTree` is deduplicated by background compaction, so other engines see duplicates until it runs (about 25 s here).
  - Every reader has to deal with the House's nested namespace (`ns_<id>.<database>`). DuckDB can query it only as one quoted name and does not list its tables.
- **Other engines on the v3 tables.**
  - Spark and Trino agree on everything.
  - **DuckDB 1.5.6 returns wrong `_row_id` values when a filter is pushed into a scan of a data file that has a deletion vector**: 350 of 400 rows wrong in one case. It is correct without the filter. This is a candidate upstream issue (§7).

## 1. What ran

| Piece | Version and pin |
|---|---|
| Object store | `rustfs/rustfs:1.0.0@sha256:8cc98017…58f4d1ff` (the repository's pin) |
| REST catalog | `quay.io/lakekeeper/catalog:v0.13.6@sha256:d6829722…bc30ea5` (2026-09-22; the version the House's REST client was tested with), on `postgres:17`. Warehouses use `flavor = s3-compat`, `sts-enabled = false` and **`remote-signing-enabled = false`** (R11.3). Every client brings its own keys |
| Writer of the v3 tables | `apache/spark:4.0.4@sha256:8ff442c3…ba5423e` (2026-07-24, unmodified) with `iceberg-spark-runtime-4.0_2.13-1.12.0.jar` (sha1 `72ff4d57…4d007cc`) and `iceberg-aws-bundle-1.12.0.jar` (sha1 `64d50122…b318eb0a`) from Maven Central, mounted read-only. Iceberg 1.12.0 was released 2026-09-30; row lineage arrived in 1.10 |
| iceberg-rust, release | `iceberg`, `iceberg-catalog-rest` and `iceberg-storage-opendal` **0.10.1** from crates.io (arrow 58) |
| iceberg-rust, release candidate | **v0.11.0-rc.2**, git commit `859048563bc71a3167d0545cb66756d26c64e4ca` (2026-09-14; arrow 58.4, DataFusion 54) |
| iceberg-rust, `main` | git commit **`000e791d5229d9d136cda2b41fa0d7563fa58e1e`** (2026-10-21, "build: disable unused zstd legacy feature (#3495)"; arrow 59.2) |
| The House | `ghcr.io/ostrium-labs/loams-fabric:0.1.0` (chDB 26.9.2.1) on `--catalog rest:…?warehouse=lakehouse --store s3://house`, `--sandbox=none` for the spike. `fabric/crates/loams-house-lake`, which writes the tables, has no commits between `v0.1.0` and `dev` (`bba24005f`), so the layout is `dev`'s |
| Other readers | DuckDB **1.5.6** (Python wheel; `iceberg` extension `890b78a9c`), `trinodb/trino:483@sha256:db58cc93…534` (the repository's pin; `nested-namespace-enabled=true`) |

The reader is one source file, `reader-common/probe.rs`, compiled three times. For each table it loads the metadata and plans the scan (counting the attached delete files). It then scans everything to Arrow and prints a fingerprint: rows, `sum(id)`, and the count of each marker value. The lineage probe selects `_row_id` and `_last_updated_sequence_number`, with or without a pushed-down filter, and prints `count`, `sum(_row_id)`, `sum(_row_id * id)` and `sum(_last_updated_sequence_number)`. Spark, Trino and DuckDB print the same fingerprints. The write probes create a table with `format_version(V3)`, fast-append a Parquet file to a v3 table, and run `UpgradeFormatVersion` to v3.

## 2. The v3 tables and who reads them

Spark wrote these tables, all `format-version = 3` with merge-on-read deletes and updates unless the table says otherwise (`spark/01-write-v3.sql`, `03`, `09`):

| Table | What it holds | Spark's answer |
|---|---|---|
| `v3.dv` | 1,000 rows in 4 files; two `DELETE`s and one `UPDATE`: **4 DVs** (one Puffin file, offsets 4/139/274/409, 135 bytes each) | 800 rows, `sum(id)` 400,400, 100 updated |
| `v3.dv_part` | partitioned by `p`, 3,000 rows in two appends; `DELETE`, then `MERGE … UPDATE`: 8 DVs | 2,571 rows, `sum(id)` 3,855,858, 514 merged |
| `v3.lineage` | two appends of 50 rows, then a merge-on-read `UPDATE` of 10 rows | 100 rows; `_row_id = id` for every row, `_last_updated_sequence_number` 1, 2 or 3 |
| `v3.lineage_cow` | the same with copy-on-write (lineage stored physically in the rewritten file) | the same lineage |
| `v3.variant` | `VARIANT` column: object, array, string, nested, `NULL` | 5 rows |
| `v3.posdel_v2` | control: the same deletes on a **v2** table (4 Parquet position-delete files) | 900 rows, `sum(id)` 450,000 |
| `v3.upgraded` | v2 with a position delete → `format-version` 3 → append → `DELETE` (the v2 deletes merge into 8 DVs) | 890 rows, `sum(id)` 494,890 |
| `v3.defaults` | `ALTER TABLE … ADD COLUMN d INT DEFAULT 7` **is refused by Spark 4.0 with Iceberg 1.12** ("setting default values in Spark is currently unsupported"), so default values were not tested | 10 rows, no default |

| Read | iceberg-rust 0.10.1 | 0.11.0-rc.2 | `main` @ 000e791 | Spark | Trino 483 | DuckDB 1.5.6 |
|---|---|---|---|---|---|---|
| v3 metadata (`next-row-id`, snapshot `first-row-id`) | yes | yes | yes | yes | yes | yes |
| **DVs** (`dv`, `dv_part`, `lineage`, `upgraded`) | **fails**: `Corrupt footer` (Puffin read as Parquet) | **correct** | **correct** | reference | correct | correct |
| v2 position deletes (`posdel_v2`) | correct, but every delete file attached to every task (20 attachments for 4 files) | correct (4) | correct (4) | reference | correct | correct |
| `_row_id`, `_last_updated_sequence_number`, no filter | `field not found` | correct | correct | reference | correct (`$row_id`) | correct |
| the same, with a pushed filter, files with DVs | — | correct | correct | reference | correct | **wrong** in 3 of 4 cases (§6) |
| Variant: load the table | **fails** (`TableMetadataEnum` does not parse) | yes | yes | yes | yes | yes |
| Variant: read the column | — | refused: `Reading variant columns is not supported yet` | refused (same) | reference | correct (`CAST AS JSON`) | correct |
| Variant table, other columns only | — | correct | correct | | | |

`_last_updated_sequence_number` comes back from iceberg-rust as `RunEndEncoded(Int32, Int64)`, not `Int64`. HE1's engine must accept run-end-encoded arrays or cast them before DataFusion's operators (R11.5).

The lineage fingerprints (`results/spark-08.txt`, `trino-fp.txt`, `lineage-filter.txt`, `read-rc2.txt`) are identical for Spark, Trino, 0.11.0-rc.2 and `main` in all six cases. For example `dv` with `id >= 500` gives 400 rows, `sum(_row_id)` 300,200 and `sum(_row_id * id)` 233,632,200.

## 3. What iceberg-rust cannot write

| Write | 0.10.1 | 0.11.0-rc.2 | `main` | Measured or read |
|---|---|---|---|---|
| Data files to a v3 table (fast append) | **works**: `next-row-id` 10 → 20, snapshot `first-row-id` 10; Spark reads `_row_id` 10–19 | not run (same code as `main` here) | **works**: 20 → 30; Spark reads 20–29 | measured (`results/spark-05.txt`) |
| Upgrade v2 → v3 (`UpgradeFormatVersion`) through Lakekeeper | **works** | — | **works**; the next append assigns ids to the 20 pre-upgrade rows (`next-row-id` 0 → 25), and Spark reads `_row_id` 0–24 | measured (`results/upgrade-append.txt`) |
| Create a v3 table through REST | **gives V2** (#2923) | **gives V2** | **gives V2** | measured; a raw REST call with `properties.format-version = "3"` makes a v3 table on Lakekeeper |
| A DV (Puffin `deletion-vector-v1` blob) | no | no | no: `DeleteVector` has `deserialize` only (#3232 adds `serialize`, open) | read in source |
| Commit delete files (position, equality or DV) | no | no | no: no `RowDelta` or overwrite action (#2580, #2203, #3300 open); fast append refuses non-data files | read in source |
| Puffin with a compressed footer | bug #2419 | bug | bug (#2921, LZ4 footer, open) | read |
| Lineage through a rewrite (compaction) | — | — | not carried (#3401, open) | read |
| Variant | no type | Parquet annotation missing (#2546, #2581) | same | read |
| Default values (`initial-default`, `write-default`) | not tested (Spark cannot write them) | | #3250 open | read |

A caution for writers: `DefaultFileNameGenerator` counts from 0 under the given prefix. Two writers that share a prefix write the same path, and the second **overwrites the first one's committed data file** before its own commit is refused. The spike hit this and fixed the probe. The House's `DataWriter` already takes a unique prefix (`write.rs`).

**What this means for the House.** The House does not use iceberg-rust's transactions. `loams-house-lake` writes its own manifests, manifest lists and `replace` snapshots (`append.rs`, `rewrite.rs`), using iceberg-rust's spec types and Parquet writers. For HE2 (D1552) it therefore needs, from upstream or written in `loams-house-lake`:

1. a DV serializer;
2. a Puffin writer with an uncompressed footer, or the #2419 fix;
3. v3 manifest-entry fields (`first_row_id`, `referenced_data_file`, `content_offset`, `content_size_in_bytes`);
4. compaction that writes `_row_id` and `_last_updated_sequence_number` physically;
5. `next-row-id` bookkeeping, including the ids of pre-upgrade manifests.

Today `DataWriter` refuses v3 tables (`write.rs`). Its message blames "a column with nanoseconds", which is wrong for an upgraded table (side finding S2).

## 4. The interop baseline: today's House v2 tables

The House wrote six tables through ClickHouse HTTP (`house-baseline.sh`). Each is Iceberg v2 in the nested namespace `ns_1.base`, partitioned and sorted as declared:

- `events`: 17 columns, `PARTITION BY toYYYYMM(ts)` becomes a `month` transform, `ORDER BY (user_id, ts)` becomes the sort order; two appends, 5 files.
- `latest`: `ReplacingMergeTree(version, is_deleted)`, two inserts.
- `compacted`: two inserts and `OPTIMIZE … FINAL`.
- `mutated`.
- `ctas`: `CREATE TABLE … AS SELECT`.
- `latest_opt` (first run only).

| Check | House (chDB) | Spark 4.0.4 | Trino 483 | DuckDB 1.5.6 | iceberg-rust 0.10.1 / `main` |
|---|---|---|---|---|---|
| `events`: count, `sum(user_id)`, nulls, `min/max(ts)`, `sum(u32)`, `sum(small)`, `sum(big)` | 20,500 / 1,459,788 / 5,500 / 2026-08-01 00:00 / 2026-11-06 05:13 / 199,990,000 / 9,990,000 / 199,990,599,970,000 | identical | identical | identical | 20,500 rows |
| Month-partition filter `ts >= 2026-10-01` | | 7,951 | 7,951 | 7,951 | |
| `latest` after background compaction | 95 with `FINAL` (150 without, before compaction) | 95 | 95 | 95 | 95 |
| `mutated` (mutations refused) | 100 | 100 | 100 | 100 | |
| Namespace listing | | `ns_1`, `ns_1.base` | `ns_1`, `ns_1.base` | lists `ns_1` only; the tables of `ns_1.base` are not listed | |
| Table name forms | | `house.ns_1.base.events` | `house."ns_1.base".events` | `h."ns_1.base".events` works; `h.ns_1.base.events` fails (`NameListToString NOT IMPLEMENTED`) | `NamespaceIdent ["ns_1","base"]` |

**How the House's ClickHouse types land in Iceberg** (read back the same by all four readers):

| ClickHouse | Iceberg v2 | Note |
|---|---|---|
| `DateTime64(3)`, `DateTime` | `timestamptz` (µs) | Spark shows `timestamp`, Trino `timestamp(6) with time zone` |
| `Date` | `date` | |
| `UInt64` | `decimal(20, 0)` | other engines see a decimal, not an integer |
| `UInt32` | `long`; `UInt8` → `int` | widened, lossless |
| `Int16` | `int`; `Int64` → `long` | |
| `LowCardinality(String)`, `String`, `Nullable(String)` | `string` (required or optional) | the ClickHouse type is in `loams.ch.types` |
| `Decimal(18,4)`, `Float64`, `Float32`, `Bool` | `decimal(18,4)`, `double`, `float`, `boolean` | |
| `Array(String)`, `Map(String,String)` | `list<string>`, `map<string,string>` | elements required |
| `UUID` | `uuid` | Spark shows `string` |

**What the House does not write today** (the baseline's limits, all known, see R0.2):

- `ALTER … DELETE` and `ALTER … UPDATE` answer 62, `DELETE FROM` answers 48, and TTL answers 48. So **no House table has a delete file**, v2 or v3. D1551's "v2 with position deletes" fallback is still to be built (HE2).
- `ReplacingMergeTree` is deduplicated at read time (`FINAL`) and by background compaction. The `replace` snapshot drops superseded and `is_deleted = 1` rows. Before compaction, Spark, Trino and DuckDB see every version of a row; in this run that window was about 25 seconds.
- A table upgraded to v3 by iceberg-rust stays readable by the House (chDB) and by every reader. The House refuses to write to it (S2). Its rows have null `_row_id` until a writer assigns ids on the next commit, as the specification says.

## 5. Versions: the answer to agent D (Task 10, R10.2)

1. **DV reads are not only on `main`.** They are in the 0.11.x branch: `compare 0.11.x...<merge commit>` shows #2866, #2868 and #3035 (the DV reads) and #2922, #2966, #2985 and #3058 (the lineage reads) as ancestors of the branch. 0.11.0-rc.2 reads every DV table correctly (§2). The branch lacks #3145 (validating DV delete tasks), #2958 (DataFusion 55, Arrow 59.2) and #3055 (#2923's fix).
2. **There is no date for 0.11.0.**
   - Tracking issue #3034 is open, rc.1 and rc.2 are tagged, and the release is blocked on licence attribution (PRs #3493, #3495, #3497 and #3486, worked on 2026-10-20).
   - On 2026-09-28 a maintainer wrote that 0.11 releases with DataFusion 54 and that they "wouldn't see one before November" (#3290).
   - On 2026-10-20 they wrote that 0.11.x will be the last release of the DataFusion integration governed by the Apache Iceberg project; the integration has moved out of the Iceberg project (#3491; R10.2 cites #3228). Which crate versions follow is undecided.
   - **0.11 does not bring Arrow 59.** R10.2's "move to DataFusion 55.x with iceberg 0.11 (Arrow 59)" should read "with the first iceberg release on Arrow 59.2 (after 0.11; no date)" (R11.1).
3. **The fallback.** If HE1 needs DV reads before 0.11.0 ships, the sound choice is to **stay on DataFusion 54.1 and pin iceberg (and `iceberg-catalog-rest`, `iceberg-storage-opendal`) as a git dependency at v0.11.0-rc.2 or the `0.11.x` head**. That keeps one Arrow major (58.4), DV and lineage reads are already there, and moving to the released 0.11.0 is a version swap. DataFusion 55 with `main` pinned by commit also works: `main` reads everything 0.11 does and built here on arrow 59.2. But `main` changes fast (between rc.2 and `main`, `FileScanTask`'s fields became private), so every repin is real work. Use it only for something that is on `main` and not in 0.11.x, such as #3145 or a Variant reader once #2880 merges.

## 6. Gap list

Each gap has its upstream issue (existing, or proposed in §7) and Loams' proposed contribution under D1553 and Q1171: upstream first, no long-lived fork. Patches are carried in Loams only until they merge. "Needed by" is the HE phase that cannot ship without it.

| # | Gap | Measured | Needed by | Upstream | Proposed contribution |
|---|---|---|---|---|---|
| G1 | DV reads not in a release; 0.10.1 fails on any table with DVs | yes | HE1 (reading Spark/DuckDB/Trino-written v3 tables; Q1175) | apache/iceberg-rust#3034 (0.11.0 release), #2792 (closed, done) | None to write. Help land 0.11.0 by reviewing the licence PRs (#3493 and the rest) if the owner wants; until then pin rc.2 (§5) |
| G2 | **No DV writer**: no serializer for `deletion-vector-v1` blobs | read | HE2 (D1552: `ALTER … DELETE/UPDATE`, `ReplacingMergeTree`) | #2580 (issue), #3232 (PR, open) | Review and finish #3232 (serialize, with round-trip tests against Spark-written DVs, using this spike's fixtures); then a `DeletionVectorWriter` that emits the Puffin file and its `DataFile` entries (`referenced_data_file`, `content_offset`, `content_size_in_bytes`). The House uses both through `loams-house-lake` |
| G3 | **No commit of delete files** (`RowDelta`); fast append only | read | HE2 upstream path (the House's own manifest writer can ship first) | #2580, #2203 (PR), #3041 (gating actions by format version), #3266 (stateful transactions RFC) | Review #2203 and test it against v3 (one DV per data file, replacing the previous DV) and v2 (position deletes, D1551's fallback). Offer the House's `assert-ref-snapshot-id` retry cases as tests |
| G4 | Puffin compressed footer bug | read | HE2 (only if footers are compressed) | #2419, #2921 (PR) | Review #2921; the House writes uncompressed footers until it merges |
| G5 | **Lineage not carried through rewrites** | read | HE2 compaction; HE4 incremental views on lineage (§9) | #3401, #3046, #3300 | A fix to the copy-on-write rewrite primitive that writes `_row_id` and `_last_updated_sequence_number` physically (Spark's `lineage_cow` table is the oracle); the same rule in `loams-house-lake/rewrite.rs` |
| G6 | `_row_id` reads implemented, but the issues are still open | yes: correct on rc.2 and `main` | HE1 | #2879, #1652 (both open; implemented by #3058, #2966, #2985, #2922, and fast append assigns `first-row-id`) | A comment on each with this spike's Spark/Trino cross-check (proposed §7.2), so they can be closed or narrowed. No code |
| G7 | `_last_updated_sequence_number` is `RunEndEncoded` | yes | HE1 (DataFusion operators) | none needed | None upstream; HE1 casts or supports run-end encoding (R11.5) |
| G8 | **Variant columns cannot be read** | yes | HE4 (Tier 1 `JSON`/Variant, D1545); HE1 only to skip the column | #2880 (PR, read unshredded), #2581, #2188 (done) | Review #2880 with the spike's 5-row Variant table (object, array, string, nested, null) as a test |
| G9 | Variant cannot be written (Parquet annotation) | read | HE4 | #2546, #2581 | After G8: the annotation behind `variant_experimental`, with a Spark/Trino read-back test |
| G10 | **REST create ignores the format version** | yes | HE2 (House-created v3 tables through iceberg-rust's client; the House's own client already works) | #2923, #3055 (PR) | Review #3055 and add a Lakekeeper test: Lakekeeper makes a v3 table when `properties["format-version"]` is `"3"` |
| G11 | v2 manifests declare v3-only fields 142, 144 and 145 | not seen to break any reader | today's House v2 tables | #3399, #3452 (PR) | Review #3452; check the House's manifests after it merges (Spark, Trino and DuckDB read them fine today) |
| G12 | Default values (`initial-default`, `write-default`) untested | Spark refused to write them | HE4 (schema evolution with defaults) | #3250 | Test with DuckDB-written defaults in HE2's interop matrix |
| G13 | **DuckDB 1.5.6: wrong `_row_id` under a pushed filter on files with DVs** | yes (`results/duckdb.txt`) | Suite 6 (D1564); any DuckDB user of House lineage | none found (closest: duckdb/duckdb-iceberg#1308, closed, about pruned files) | **Proposed issue** §7.1 (with the reproduction), after re-checking on the extension's newest build |
| G14 | DuckDB does not list tables in nested namespaces; dotted names fail | yes | Suite 6 and users of the House's `ns_<id>.<db>` | duckdb/duckdb-iceberg#348 (closed), #361 (an `ATTACH` option for nested namespaces) | Try #361's option first; propose an issue only if listing still fails (§7.3) |
| G15 | Lakekeeper's default remote signing: Spark's ranged reads of a Puffin DV fail (`SignatureDoesNotMatch`) against RustFS | yes (with remote signing on) | Deployments that keep Lakekeeper's default | not yet narrowed (Lakekeeper, RustFS or the AWS SDK) | Narrow it to one side with a minimal ranged `GET`, then propose an issue there; the House's compose sets `remote-signing-enabled = false` meanwhile (R11.3) |

The order of contribution work, if the owner agrees (D1553):

1. G2, then G3 (HE2's critical path).
2. G5.
3. G10 and G6 (small).
4. G8 and G9 (HE4).

G1 needs only a version bump when 0.11.0 ships.

## 7. Proposed upstream issues (not posted)

Nothing below has been posted. Each item waits for the owner's consent, relayed by the controller.

1. **duckdb/duckdb-iceberg: "`_row_id` is wrong when a filter is pushed into a scan of a data file with a deletion vector".**
   - **Repro:** Spark 4.0.4 with Iceberg 1.12.0. A v3 table `dv (id BIGINT, v STRING)`, 1,000 rows over 4 files. `DELETE … WHERE id % 10 IN (0, 1)` and an `UPDATE` (merge-on-read) leave one DV per file. In Spark, `_row_id = id` for every row.
   - **Observed in DuckDB 1.5.6 (extension `890b78a9c`):**
     - `SELECT count(*) FILTER (WHERE _row_id <> id) FROM dv` gives 0. Correct.
     - `… FROM dv WHERE id >= 500` gives **350 of 400**. Wrong.
     - On `lineage`, `WHERE id = 49` gives `_row_id` 12 (Spark 49).
     - Copy-on-write tables are correct.
   - Before filing, re-check on the extension's newest build, since lineage fixes landed on 2026-10-10 (#1542–#1553).
2. **apache/iceberg-rust #2879 and #1652 (comments, not new issues).** Report that 0.11.0-rc.2 and `main` read `_row_id` and `_last_updated_sequence_number` identically to Spark and Trino, including with filters and on DV files, and that fast append (0.10.1 onwards) and upgrade-then-append assign `first-row-id` correctly. Ask whether the issues can be closed, or what remains.
3. **duckdb/duckdb-iceberg (conditional): "tables in nested REST namespaces are not listed".** Only if #361's `ATTACH` option does not fix it.
4. **Lakekeeper or RustFS (conditional): remote-signed ranged `GET` fails with `SignatureDoesNotMatch`.** Only after narrowing G15.
5. **apache/iceberg-rust (optional, a docs issue): "`DefaultFileNameGenerator` reuses names across writers".** Two writers that share a prefix overwrite each other's committed files. Ask for a note in the docs or a UUID by default.

Upstream contributions as PR reviews and code (G2, G3, G5, G8–G10) are proposals too. None starts before the owner agrees.

## 8. Side findings for other tasks

- **S1, the House with a store prefix (HS1, Task 9).** With `--store s3://spike/house` (a key prefix), the House's writes succeed but every read answers `499 … HTTP 403 … while reading 'ns/1/house/base/…'`. With `--store s3://house` (a bucket root), reads work. The likely cause, inferred and not confirmed: `house-cache`'s `Upstream::relative` expects the store prefix in the key, and the worker sends keys without it. Today's compose uses a bucket root. The spike ran on one.
- **S2, the House's refusal message on v3 tables.** `writes to a format 3 table (a column with nanoseconds) are not supported yet` is shown for any v3 table, including one upgraded with no nanosecond column (`loams-house-lake/src/write.rs`). A one-line fix for HE2 or sooner.
- **S3, Lakekeeper's warehouse defaults.** It defaults to `remote-signing-enabled: true`. iceberg-rust ignores the signer settings, so a client without its own keys cannot read. The House always brings keys.

## 9. What HE1 and HE2 take from this

- HE1 reads v3 with iceberg 0.11 (DV reads included) on DataFusion 54.1 and Arrow 58. Variant columns are refused with `48`, or skipped when not projected, until G8 lands.
- HE1's `loams-house-iceberg` tests reuse this spike's Spark tables as fixtures. They are regenerated by Spark in CI, never committed as binaries; this is suite 6, D1564.
- HE2's critical path is G2 plus G3, or the same pieces inside `loams-house-lake`, carried only until upstream merges.
- D1551's fallback (v2 with position deletes) needs the House to write position-delete manifests itself. Position-delete files are written by iceberg-rust's `PositionDeleteFileWriter` (#2986, on `main`).
- Upgrading today's v2 tables in place works with iceberg-rust's `UpgradeFormatVersion` through Lakekeeper. The House's next commit after an upgrade must assign `first-row-id` to the existing manifests, as iceberg-rust's fast append already does.
