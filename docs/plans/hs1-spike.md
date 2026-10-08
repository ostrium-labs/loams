# HS1 Task 1: the chDB, Iceberg and sandbox spike

Task 1 of [the HS1 plan](2026-10-08-hs1-house-production.md) measures the facts that §49 §4, §6.1, §7 and §13.2 rest on. No product code was written. The results were recorded on 2026-10-08 on branch `backend/hs1`. The rulings drawn from them are R1.1–R1.14 in the plan's "Rulings made during execution". The measured numbers alone are repeated in [`docs/house/performance.md`](../house/performance.md).

**The stop condition was not hit.** chDB reads Iceberg through a custom loopback endpoint, so D766 stands (§2).

## 0. Setup

| What | Value |
|---|---|
| Machine | The build machine: Intel Core Ultra 5 225H, 14 cores, 15 GiB RAM, Linux 7.2.4-3-cachyos. It was shared with other agents' cargo builds, with a load average of 4–6 throughout, so the timings below are upper bounds rather than reference-hardware numbers |
| libchdb | `linux-x86_64-libchdb.tar.gz` from `chdb-io/chdb-core` v26.9.0, SHA-256 `c6398bcc…670543b`, which matches `loams-chdb-sys/build.rs`. It unpacks to `libchdb.so` (554 263 672 bytes) **plus `chdb.h` and `chdb.hpp`**. FL2 spike §2 says the tarball ships no header. That is wrong at this digest: the shipped `chdb.h` is byte-identical to the vendored `fabric/crates/loams-chdb-sys/chdb.h` once the 15-line vendoring comment is removed |
| Versions | `chdb_version()` = `26.9.0`; `SELECT version()` = **`26.9.2.1`**, the ClickHouse version, which `versions.rs` and the reference-server digest must use |
| Harness | A C probe for boot timing (`dlopen` + `chdb_connect` + `chdb_query`), and Python `ctypes` over the shipped `chdb.h` signatures for the SQL probes. Every signature was copied from the header and none was guessed (FL2 Ruling 1). Everything lived in an untracked `scratch/` directory in the worktree |
| Stub S3 | A 150-line path-style S3 server written for the spike: GET with ranges, HEAD, ListObjectsV2, PUT, multipart and DELETE over a directory, bound to `127.0.0.1:0` (port 41887 in the runs below), with **every request logged**. The log is what proves pruning and the absence of LIST requests |
| Iceberg tables | Written by pyiceberg 0.12.0 + pyarrow 25.0.1 (a SQLite catalog, warehouse `s3://bkt/wh` on the stub), so the metadata carries absolute `s3://` paths, as Lakekeeper-managed tables do. Delete files and the v3 table were made by hand (§2.4, §2.6) because pyiceberg writes neither |

## 1. Worker boot and idle memory

Each run is one process: `exec`, `dlopen("libchdb.so", RTLD_NOW)`, `chdb_connect(["clickhouse", "--path=<fresh dir>"])` and `chdb_query("SELECT 1", "TSV")`. Timestamps come from `CLOCK_MONOTONIC`, read by the parent before `Popen` and by the child at each step. There were 50 sequential runs, each with a fresh `--path`, and the page cache was warm after the first run of the session.

```
$ python3 boot_bench.py boot 50
first run (after page cache state at start): {"total": 132.2, "exec_to_main": 1.3, "dlopen": 40.8, "connect": 86.1, "select1": 3.7}
total         p50=   136.5 ms  p95=   154.2 ms  min=  118.8  max=  154.5
exec_to_main  p50=     1.4 ms  p95=     1.7 ms
dlopen        p50=    40.5 ms  p95=    46.1 ms
connect       p50=    89.7 ms  p95=   103.1 ms
select1       p50=     4.2 ms  p95=     5.2 ms
```

The very first run of the session, with the 554 MB library cold in the page cache, took **1 068 ms**, of which 884 ms was `dlopen`. A node therefore pays that cost once per boot or image pull, and every later worker boots in about 140 ms. §16's gate (p95 ≤ 1 s) holds with a margin of about 6×.

Idle RSS was measured with N workers alive at once after `SELECT 1`, reading `/proc/<pid>/smaps_rollup` 3 s later:

```
workers= 1 per-worker Rss≈265 MiB private≈262 MiB (Private_Dirty 104) shared≈3 MiB   Pss≈262 MiB; sum Pss=262 MiB
workers= 4 per-worker Rss≈258 MiB private≈97 MiB  (Private_Dirty 97)  shared≈161 MiB Pss≈137 MiB; sum Pss=546 MiB
workers=16 per-worker Rss≈227 MiB private≈66 MiB  (Private_Dirty 66)  shared≈161 MiB Pss≈76 MiB;  sum Pss=1212 MiB
```

About 160 MiB of libchdb text is shared between workers. Each idle worker adds **66–104 MiB of private dirty memory**, so 16 idle workers cost about 1.2 GiB in PSS. The machine-wide `MemAvailable` drop agreed within noise (1.13 GiB for 16 workers).

Sandbox cost was measured in a separate session of 30 runs each, at a load average of 7–9, through `sh -c 'ip link set lo up; exec boot …'`:

```
plain (via sh -c)                              p50= 146.3 ms p95= 155.2 ms
+ worker --config-file (users file, grants)    p50= 148.0 ms p95= 163.3 ms
+ unshare -Urn (user+net ns, lo up)            p50= 147.2 ms p95= 166.1 ms
+ unshare -Urnmpf (user+net+mount+pid)         p50= 147.8 ms p95= 163.4 ms
```

The users file and four new namespaces add no measurable boot time (≤ 11 ms at p95, within noise). **One trap:** when the users file restricts `default` to `<networks><ip>127.0.0.1</ip></networks>`, `chdb_connect` *fails* (returns NULL) in a network namespace whose `lo` is still down. It succeeds once `lo` is up, or with `::/0`. The worker therefore brings `lo` up before it connects.

A second chDB connection in the same process, with the same server arguments and different query-level settings (`--readonly=2 --max_threads=2`), took **1.1–1.4 ms** (§5.3).

## 2. chDB reads Iceberg through a loopback endpoint

The table function was `icebergS3('http://127.0.0.1:41887/bkt/wh/db/<t>/', 'dummy', 'dummy' [, SETTINGS …])`. Every row below was taken from the stub's request log for that query only. The full output is reproduced in §2.7.

| Probe | Result | S3 requests |
|---|---|---|
| Unpinned read (`count(), sum(v)`) | 3000 rows, correct sum | 27 requests, **3 LIST** of `metadata/` (latest version by listing) |
| **Pinned** `SETTINGS iceberg_metadata_file_path = 'metadata/00001-….metadata.json'` (inside the table function) | 1000 rows: the first snapshot only | 10 requests, **0 LIST** |
| Pinned to `00003-…` | 3000 rows | 14 requests, 0 LIST, 6 of 6 data files |
| The same setting as a query-level `SETTINGS` | **`115 UNKNOWN_SETTING`**: it exists only as a table-function or engine setting | — |
| Partition pruning `WHERE part = 'a'` (identity partition) | 1500 | **3 of 6** data files |
| `use_iceberg_partition_pruning = 0` | 1500 | 6 of 6 |
| Min/max pruning `WHERE id < 500` | 500 | **2 of 6** (only the first append's files) |
| Min/max `WHERE id BETWEEN 1500 AND 1600` | 101 | 2 of 6 |
| Min/max on `timestamptz` `WHERE ts >= '2026-01-01 00:40:00'` | 600 | 2 of 6 |
| `count()` with no filter | Answered from manifest row counts | **0 data GETs** (an unpinned read made 0 data GETs too) |
| Time travel `SETTINGS iceberg_snapshot_id = <first>` (query level) | 1000 | It works, but LISTs `metadata/` (3) unless also pinned. Pinned and snapshot id together: 0 LIST |

### 2.1 Pinning

`iceberg_metadata_file_path` is a path relative to the table URL, and pinning removes every ListObjectsV2 from the read path. Every pinned query in this spike made **zero** LIST requests, so `house-cache` does not need LIST to serve reads. It still needs it for the front's own catalog work, and for unpinned reads, which the House never generates.

### 2.2 Types

`timestamptz` → `Nullable(DateTime64(6, 'UTC'))`. `timestamp` → `Nullable(DateTime64(6))` with no zone, which displays in the session time zone. `long` → `Nullable(Int64)`. Every column is `Nullable` unless the Iceberg field is `required`, so the view DDL of §7 must restore the ClickHouse types (`loams.ch.types`) with casts, as §7 already says.

### 2.3 Schema evolution

Table `evo` was written with `(id int, name string, x float)`, then evolved by adding `extra string`, renaming `name` to `label`, and widening `id` to `long` and `x` to `double`, and then appended again:

```
1	Nullable(Int64)	one	1.5	Nullable(Float64)	\N
2	Nullable(Int64)	two	2.5	Nullable(Float64)	\N
3	Nullable(Int64)	three	3.5	Nullable(Float64)	\N
4	Nullable(Int64)	four	4.25	Nullable(Float64)	e4
5	Nullable(Int64)	five	5.25	Nullable(Float64)	e5
1099511627776	Nullable(Int64)	big	1e300	Nullable(Float64)	e6
```

Old files are read by field id: the renamed column carries its old values, the widened columns widen, and the added column is NULL in old files.

### 2.4 v2 position and equality deletes

Table `dels` holds ten rows (`id` 0–9) in one file. A second snapshot adds a delete manifest with a position-delete file (`file_path`, `pos` 0 and 1, field ids 2147483546/2147483545) and an equality-delete file (`equality_ids = [1]`, `id = 5`). It was written with a subclass of pyiceberg's `_FastAppendFiles` whose `ManifestWriterV2.content()` is `DELETES`. chDB answers `[2,3,4,6,7,8,9]`, which is correct for both kinds. pyiceberg's own reader refuses the table ("does not yet support equality deletes").

### 2.5 The in-memory caches, and no disk cache

```
-- system.server_settings (defaults at 26.9.2.1)
iceberg_metadata_files_cache_size 134217728   max_entries 1000   policy SLRU   (query setting use_iceberg_metadata_files_cache = 1)
parquet_metadata_cache_size       536870912   max_entries 5000   policy SLRU   (query setting use_parquet_metadata_cache = 1)
query_condition_cache_size        104857600                                    (use_query_condition_cache = 1)
page_cache_max_size 0; use_page_cache_for_object_storage 0; filesystem_caches_path ''; cache_size_to_ram_max_ratio 0.5
```

After pinned reads: `IcebergMetadataFilesCacheHits 25 / Misses 5`, `IcebergMetadataFilesCacheBytes 51228`, `IcebergMetadataFilesCacheFiles 5`, `ParquetMetadataCacheHits 9 / Misses 6`.

**Nothing is cached on disk.** The reads were run under `bwrap --ro-bind / /` with only `--path` and a scratch `/tmp` writable. They succeeded, including with `SETTINGS enable_filesystem_cache = 1`. `system.filesystem_cache` had 0 rows, `FilesystemCacheSize` and `FilesystemCacheElements` were 0, the scratch `/tmp` stayed empty, and the only file under `--path` while the process was alive was the 60-byte `status`. After exit only an empty `tmp/` remained. These caches are keyed by immutable object paths, so they never serve stale data for pinned reads. They are, however, per-process memory, up to 128 + 512 + 100 MiB per worker, and the worker should lower them with `--config-file` server settings (R1.4).

### 2.6 Nanosecond timestamps (format v3)

pyiceberg 0.12 refuses to write v3, so the table was made by hand. The Parquet file was written with `TIMESTAMP(NANOS)` columns (field ids 1–3) and appended without statistics. The new `metadata.json` was then rewritten to `format-version: 3` with `timestamp_ns` and `timestamptz_ns` fields, and the schema embedded in each manifest was rewritten with fastavro.

```
DESCRIBE …  tns Nullable(DateTime64(9))   tzns Nullable(DateTime64(9, 'UTC'))
1  2026-01-01 00:00:00.123456789  2026-01-01 00:00:00.123456789  1767225600123456789  1767225600123456789
2  2026-01-01 00:00:00.999999999  2026-01-01 00:00:00.000000001  1767225600999999999  1767225600000000001
WHERE tzns > '2026-01-01 00:00:00.5' -> 0   (correct)
```

chDB reads v3 nanosecond timestamps exactly. Whether iceberg-rust 0.10.1 can *write* v3 metadata is Task 10's to check: §5.3's `DateTime64(p > 6)` → `timestamp_ns` mapping depends on it.

### 2.7 Raw output

The full probe output, for the record:

```
## pin to metadata v1 (1 snapshot)
>> SELECT count(), max(id) FROM icebergS3('http://127.0.0.1:41887/bkt/wh/db/basic/', 'dummy', 'dummy', SETTINGS iceberg_metadata_file_path = 'metadata/00001-….metadata.json')
1000	999
   [rows_read=1 s3: 10 req, 0 LIST, data GETs=2 distinct data files=2]
## pin as query-level SETTINGS
ERR: Code: 115. DB::Exception: Unknown setting 'iceberg_metadata_file_path': Maybe you meant ['use_iceberg_metadata_files_cache','iceberg_metadata_log_level']. (UNKNOWN_SETTING)
## partition pruning part='a'
1500
   [rows_read=1 s3: 8 req, 0 LIST, data GETs=3 distinct data files=3]
## min/max pruning id < 500
500
   [rows_read=1 s3: 6 req, 0 LIST, data GETs=2 distinct data files=2]
## deletes: position (pos 0,1) + equality (id=5)
[2,3,4,6,7,8,9]
## time travel by snapshot id
1000	999
   [rows_read=1 s3: 9 req, 3 LIST, data GETs=2 distinct data files=2]
```

## 3. chDB's Iceberg writer (evidence for Q688)

| Probe | Result |
|---|---|
| `INSERT INTO TABLE FUNCTION icebergS3(…)` with default settings | **`344 SUPPORT_IS_DISABLED`: "Insert into iceberg is in beta. To allow its usage, enable setting allow_insert_into_iceberg"**. `system.settings` lists `allow_insert_into_iceberg` as tier **Beta** and `allow_experimental_iceberg_compaction` as **Experimental** |
| The same with `allow_insert_into_iceberg = 1`, into the pyiceberg table | It works, and writes `data/data-<uuid>.parquet`, a manifest, a manifest list and **`metadata/v4.metadata.json`** beside pyiceberg's `00003-<uuid>.metadata.json`. **No catalog is involved**: the SQLite catalog still points at `00003`, and the commit is an unconditional PUT of the next version number, with no compare-and-swap |
| `CREATE TABLE … ENGINE = IcebergS3(…)` with a `LowCardinality(String)` column | `36 BAD_ARGUMENTS: Unsupported type for iceberg LowCardinality(String)` |
| `CREATE TABLE chw (id Int64, s String, t9 DateTime64(9,'UTC'), t6 DateTime64(6), u UInt64, d Decimal(20,0))` | It works, with no flag needed for CREATE. The Iceberg schema written is **`t9 timestamp`** (microseconds, with the zone dropped), `t6 timestamp`, **`u long`** (UInt64 written as signed) and `d decimal(20, 0)`, all at format-version 2. Reading `t9` back gives `DateTime64(6)` with the nanoseconds truncated |
| `ALTER TABLE chw DELETE WHERE id = 3` | It works, and writes `<uuid>-deletes.parquet`, a v2 position-delete file, in a new snapshot |
| `OPTIMIZE TABLE chw` with `allow_experimental_iceberg_compaction = 1` | It rewrote the data into new files, wrote **`v0.metadata.json`** with a single `append` snapshot (sequence number 1), and then **DELETEd every earlier metadata file, manifest, manifest list, data file and delete file** (11 `DELETE` requests in the stub log). History, time travel and any reader pinned to an older snapshot are destroyed, and the metadata version number goes backwards |

These results support the default answer to Q688: Loams commits with iceberg-rust, and chDB's writer is never reachable. The writer bypasses the catalog, loses types, and its compaction deletes history. The worker's profile must also pin `allow_insert_into_iceberg`, `allow_experimental_iceberg_compaction` and `allow_iceberg_remove_orphan_files` to 0, and grant no `WRITE ON S3` (§5).

## 4. `FORMAT Native` framing and `chdb_stream_insert`

**Native framing at 26.9.2.1, with default settings.** The output is a sequence of blocks with no `BlockInfo` header and no per-column serialization byte. Each block is `varUInt ncols`, `varUInt nrows`, then for each column `String name`, `String type` and the column data:

```
02 80 80 04 | 01 6e | 06 55 49 6e 74 36 34 | 00 00 00 00 00 00 00 00 | 01 00 00 …
ncols=2  nrows=65536  "n"    "UInt64"          first value 0             value 1
buffered: 2 888 977 bytes, 4 blocks, rows/block = [65536, 65536, 65536, 3392]
streamed (chdb_stream_query + chdb_stream_fetch_result): 4 chunks of [906416, 948598, 983062, 50901] bytes;
every chunk parses as whole blocks (chunk boundaries are block boundaries)
```

**`chdb_stream_insert`** was tested into `CREATE TEMPORARY TABLE … ENGINE = Memory`:

| Body | Result |
|---|---|
| `Native`, 2.9 MB appended in **7 777-byte chunks that are not block-aligned** | `rows_written = 200000`, and count, sum and distinct match |
| `Parquet`, 11.3 MB in about 1 MiB chunks | `rows_written = 2000000`, sum matches. **No file was created** under `--path` (only `status`), so the body is buffered in memory, not in a temporary file |
| Truncated Parquet (first 1000 bytes) | A clean error: `117 INCORRECT_DATA: Not a Parquet file (wrong magic bytes at the end of file)` |
| `INSERT INTO t SELECT … FROM input('…')` as the stream's statement | **Refused**: `send_insert does not support INSERT ... SELECT (use a normal query)` |

So `Input` frames feed `chdb_stream_insert` directly and need no temporary file. The transform of §6.1 step 2 (casts, `DEFAULT`, sort, `FORMAT Parquet`) runs as a second statement over the staged temporary table, because the stream cannot carry `INSERT … SELECT`. Parquet bodies are held in RAM until `chdb_stream_done`, so the body size counts against the worker's memory limit (R1.7).

## 5. Access control inside chDB (L2)

### 5.1 The default user cannot be restricted by SQL

| Probe | Result |
|---|---|
| `SHOW GRANTS` | `default` holds every privilege, including `FILE, URL, REMOTE, S3, …, SOURCES`, `SYSTEM` and `INTROSPECTION` |
| `system.user_directories` | `users_xml` only, which is read-only |
| `REVOKE READ ON URL FROM default`, `REVOKE READ, WRITE ON S3 FROM default` | `497`: "granted, but without grant option" |
| `CREATE USER`, `CREATE ROLE` | `497` |
| `--user=q` at `chdb_connect` | Ignored: `currentUser()` is still `default` |
| `EXECUTE AS q …` | `344`: "IMPERSONATE feature is disabled, set access_control_improvements.allow_impersonate_user". Setting that key in the `--config-file` has no effect in chDB |

### 5.2 A users file at connect does work

```xml
<!-- --config-file=<cfg>.xml -->
<clickhouse>
  <users_config>/…/users-worker.xml</users_config>
  <user_files_path>/…/private-tmp/</user_files_path>
</clickhouse>
<!-- users-worker.xml: the `default` user redefined -->
<users><default> … <profile>worker</profile>
  <grants>
    <query>GRANT SELECT, SHOW, CREATE TEMPORARY TABLE, CREATE VIEW, DROP VIEW, CREATE DATABASE, DROP DATABASE, INSERT ON *.*</query>
    <query>GRANT READ ON S3('http://127\.0\.0\.1:41887/bkt/wh/db/.*')</query>
  </grants></default></users>
<profiles><default/><worker>
  <allow_introspection_functions>0</allow_introspection_functions>
  <constraints><allow_introspection_functions><readonly/></allow_introspection_functions>
               <allow_insert_into_iceberg><readonly/></allow_insert_into_iceberg></constraints>
</worker></profiles>
```

The users file must be a separate file named by `<users_config>`. A `<users>` section inline in the `--config-file` is ignored, and so is `--users_config=` as an argument. The profile named `default` must exist, or the connect fails with `180 THERE_IS_NO_PROFILE`.

| Statement (worker user) | Answer |
|---|---|
| `file('/etc/hostname')`, `file('x.tsv')` | `497`, READ ON FILE |
| `INSERT INTO FUNCTION file(…)` | `497`, WRITE ON FILE |
| `url('http://127.0.0.1:<p>/…')` | `497`, READ ON URL |
| `icebergS3('http://127.0.0.1:<p>/bkt/wh/db/basic/', …)` | **allowed** (matches the grant's URL regex) |
| `s3('http://127.0.0.1:<p>/bkt/other/x.parquet', …)` | `497`, READ ON S3: **the URL regex scopes the grant** |
| `INSERT INTO FUNCTION s3(…)` | `497`, WRITE ON S3 |
| `remote('127.0.0.1:1', system.one)` | `497`, READ ON REMOTE |
| `CREATE TABLE … ENGINE = MergeTree` | `497`, CREATE TABLE |
| `CREATE FUNCTION`, `CREATE DICTIONARY … SOURCE(HTTP(…))`, `SYSTEM DROP DNS CACHE` | `497` |
| `SET allow_introspection_functions = 1`, `SET allow_insert_into_iceberg = 1` | `452 SETTING_CONSTRAINT_VIOLATION` |
| `executable(…)` | `1`: the file is not in `<path>/user_scripts/` |
| **`SELECT … INTO OUTFILE '<abs>'` and `'rel.tsv'`** | **Succeeds and writes the file on the host** (relative to the process cwd), with no FILE grant |
| `system.stack_trace`, `hostName()`, `system.server_settings` | Readable: L1's job |
| `KILL QUERY WHERE 1` | Runs (it only sees its own process) |
| `CREATE VIEW default.v AS SELECT 1` | `497`, READ ON FILE: chDB's `default` database is `Overlay` over `Filesystem` (§5.3) |

### 5.3 `INTO OUTFILE`, `user_files_path`, read-only sessions and databases

- **`INTO OUTFILE` writes a server-side file in chDB**, with an absolute path or one relative to the process cwd. It does so without a FILE grant, and even with `readonly = 1` or `readonly = 2`. A second run answers `504 FILE_ALREADY_EXISTS`. Only L1 (refusing the clause) and L3 (Landlock) stop it.
- **`user_files_path` does not confine `file()` in chDB.** With `--user_files_path=` (empty), `file('x.txt')` reads `x.txt` in the process cwd, and `file('/etc/hostname')` and `file('../../out/*.txt')` read outside it. With it set to a directory, absolute paths still read. The FILE grant is what stops it.
- **Per-connection query-level settings work.** A second `chdb_connect` on the same `--path` and `--config-file`, with `--readonly=2 --max_threads=2`, returns in about 1 ms and reports `readonly = 2`, `max_threads = 2`, while the first connection keeps `0` and `14`. (FL2 Ruling 12's refusal applies to differing *server-level* arguments, which this spike did not test.)
- **`readonly = 2` on the user connection is sticky.** `SET readonly = 0` or `1` gives `164 READONLY`, and so does `CREATE VIEW`. `SET max_threads = 3`, `SETTINGS max_threads = 3`, `CREATE TEMPORARY TABLE`, `INSERT INTO <temporary>`, `USE d1`, reading views over `icebergS3`, and a user-written `icebergS3` inside the grant regex are all allowed.
- **`readonly = 1` is unusable for the user connection.** It refuses `SELECT` from a view whose body is a table function (`164`), and refuses every `SET`.
- **Views live in `Memory` databases.** `CREATE DATABASE d1 ENGINE = Memory` plus `CREATE VIEW d1.t AS SELECT * FROM icebergS3(…)` on the control connection is visible to the user connection (`USE d1`, or `--database=d1` at connect). **`DROP DATABASE default; CREATE DATABASE default ENGINE = Memory`** at worker start removes the `Filesystem` overlay, so `SELECT * FROM 'x.csv'` becomes `60 UNKNOWN_TABLE`, and lets the House's `default` database hold views. Dropping `default` *without* recreating it makes every later `chdb_connect` fail.

## 6. The OS sandbox (L3)

| Check | Build machine (Linux 7.2.4-3-cachyos) | Rootless podman 6.1.1 container, default seccomp profile, `--network=none` | kind node | Stock GKE node image | Stock EKS node image |
|---|---|---|---|---|---|
| Unprivileged user namespaces | Yes: `unshare -Urn` (uid 0 inside); `user.max_user_namespaces = 61829`; `kernel.unprivileged_userns_clone = 1` | **Yes**: `unshare -Urn` inside the container works (`max_user_namespaces = 2147483647`) | unknown | unknown | unknown |
| New network namespace with only `lo` | Yes (`lo` DOWN until `ip link set lo up`) | Yes | unknown | unknown | unknown |
| user+net+mount+pid namespaces together | Yes (`unshare -Urnmpf --mount-proc`: pid 1, 3 pids visible) | not run | unknown | unknown | unknown |
| Landlock | **ABI 10** (`landlock_create_ruleset(NULL, 0, VERSION)`); LSMs `capability,landlock,lockdown,yama,bpf` | **ABI 10** (the host kernel) | unknown (a kind node shares the host kernel, so expect the host's ABI) | unknown | unknown |
| seccomp | `CONFIG_SECCOMP_FILTER=y`; `SECCOMP_GET_ACTION_AVAIL` for `KILL_PROCESS` and `USER_NOTIF` = 0 (available); `PR_SET_NO_NEW_PRIVS` = 0 | Already filtered (`Seccomp: 2`); the probe's own `NO_NEW_PRIVS` succeeds, so a worker filter stacks on the runtime's | unknown | unknown | unknown |

No kind cluster existed on the machine and none was created, because the machine is shared and the brief allows "unknown". GKE and EKS were not reachable. The podman column is the closest local stand-in for a pod, but it is not a pod. Kubernetes' `RuntimeDefault` seccomp profile (containerd's, derived from Docker's) is commonly understood to refuse `unshare(CLONE_NEWUSER)` without `CAP_SYS_ADMIN`. That was **not measured here**, and it is what Task 6 must measure on kind before choosing between in-pod namespaces and §13.2's worker-pod fallback.

**The netns-plus-forwarder shape of §13.2 works end to end with chDB.** The setup was:

- a host-side forwarder from Unix socket `out/fwd.sock` to the stub at `127.0.0.1:41887`, standing in for `house-cache`'s per-worker socket;
- `unshare -Urn`, then `ip link set lo up` inside;
- an in-namespace forwarder from `127.0.0.1:59077` to the same Unix socket. A pathname Unix socket crosses network namespaces because it lives in the filesystem.

```
lo (interfaces in worker netns)
via forwarder 127.0.0.1:59077: 3000
host stub port directly 127.0.0.1:41887: ERR: Code: 499 … Connection refused …
external 1.1.1.1: ERR: Code: 499 … Network is unreachable: 1.1.1.1:80 …
```

## 7. iceberg 0.10.1 + arrow 58 in the `fabric/` workspace

A temporary binary crate `fabric/crates/hs1-iceberg-probe` was added to the workspace, measured, then removed, with `fabric/Cargo.lock` restored byte for byte. Without its `lake` feature it links only the workspace's arrow 59 (it writes an IPC stream). With `lake` it also drives the front's commit path:

- `TableMetadata` is parsed from a pyiceberg `metadata.json`;
- `iceberg::arrow::schema_to_arrow_schema` produces an arrow 58 schema;
- a `Table` is built over `FileIO::new_with_memory()`;
- `Transaction::fast_append().add_data_files([DataFileBuilder…])` is applied;
- `RestCatalogBuilder::load("rest", {uri})` is called, then `tx.commit(&catalog)`.

The commit reaches the catalog and fails only because no catalog is listening (`commit=Some(Unexpected)`). Both builds used `cargo build --release -p hs1-iceberg-probe` with `jobs = 6` and the shared target directory, under `nice`, on the shared machine. The baseline ran first, so the second build compiled only what `lake` adds.

| | Baseline (arrow 59 only) | With `lake` (iceberg 0.10.1 + iceberg-catalog-rest 0.10.1) | Delta |
|---|---|---|---|
| Crates compiled in the build | 42 | 158 (on top of the baseline's) | +158 |
| Crates in `cargo tree -e normal` | 65 | 260 | +195 |
| Release build time (cargo's `Finished`) | 1 m 16 s | 3 m 19 s (wall 200 s) | +3 m 19 s incremental |
| Binary size | 2 638 408 B (stripped 1 807 440 B) | 13 081 856 B (stripped 9 286 448 B) | **+10.4 MB (stripped +7.5 MB)** |

`cargo tree -d` with `lake` shows the full arrow family twice: `arrow-{arith,array,buffer,cast,data,ipc,ord,schema,select,string}` at **58.4.0 and 59.3.0**, with `parquet` 58.4.0 beside the workspace's 59. The other new families are `apache-avro` 0.21, `reqwest` 0.12.28, and second versions of `base64`, `darling` and `getrandom`. `tokio` stays unified at 1.53.2. `cargo deny check licenses` with the feature on reports `licenses ok`. One API fact for Task 10: `Table::builder()` in 0.10.1 **requires `.runtime(iceberg::Runtime::…)`** and fails with `DataInvalid: Runtime must be provided with TableBuilder.runtime()` otherwise.

Two facts about the crate that matter beyond build cost:

- **iceberg 0.10.1 has no S3 storage in its core crate.** `src/io/storage` holds only `memory` and `local_fs`. S3 lives in the separate `iceberg-storage-opendal` 0.10.1, and storage is pluggable through `iceberg::io::{Storage, StorageFactory}` (`CatalogBuilder::with_storage_factory`). The manifests and manifest lists of a `fast_append` are written through that `FileIO`, so `loams-house-lake` must provide one.
- No iceberg release on arrow 59 exists yet (crates.io, 2026-10-08: 0.10.1 of 2026-08-01 is the newest, on arrow 58 and parquet 58), so the "wait for iceberg-rust on arrow 59" option of Q688 has no date.

## 8. What the spike did not do

- No kind, GKE or EKS measurements (§6).
- No Lakekeeper: the SQLite catalog stood in for it, which is enough for chDB, because chDB never talks to the catalog and reads only the pinned metadata file.
- No large-table, cold-cache or concurrency performance. Those are §16's gates, measured on the reference hardware (Task 34).
- `chdb_stream_insert` memory for bodies near the 256 MiB commit target was not measured, only shown to be buffered (Task 12).
