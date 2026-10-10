# Loams House: measured performance

This page holds measured numbers only, each with the date, the build and where it was measured. The commands and the raw outputs behind each one are in [`docs/plans/hs1-spike.md`](../plans/hs1-spike.md). The targets are §49 §16's gates. The reference-hardware runs that decide them are added by HS1 Task 34.

## Worker boot and idle memory

Measured 2026-10-08 (HS1 Task 1) with chdb-core v26.9.0 (ClickHouse 26.9.2.1) on the build machine: Core Ultra 5 225H, 14 cores, 15 GiB, Linux 7.2.4, shared with other builds, load average 4–6. The page cache was warm after the first boot. Each boot is `exec`, then `dlopen(libchdb.so)`, then `chdb_connect(--path=<fresh>)`, then `SELECT 1`, over 50 runs.

| Step | p50 | p95 |
|---|---|---|
| Spawn to `main` | 1.4 ms | 1.7 ms |
| `dlopen` libchdb (554 MB) | 40.5 ms | 46.1 ms |
| `chdb_connect` | 89.7 ms | 103.1 ms |
| First `SELECT 1` | 4.2 ms | 5.2 ms |
| **Total, spawn to first answer** | **136.5 ms** | **154.2 ms** (gate: p95 ≤ 1 s) |

| Case | Value |
|---|---|
| First boot with the library cold in the page cache | 1 068 ms (of which `dlopen` 884 ms) |
| A second connection in a booted process (`--readonly=2 --max_threads=2`) | 1.1–1.4 ms |

Sandbox overhead (30 runs each, load average 7–9, same build):

| Boot under | p50 | p95 |
|---|---|---|
| No sandbox | 146.3 ms | 155.2 ms |
| Worker `--config-file` (users file with grants) | 148.0 ms | 163.3 ms |
| That + `unshare -Urn` (user and network namespaces) | 147.2 ms | 166.1 ms |
| That + mount and pid namespaces (`unshare -Urnmpf`) | 147.8 ms | 163.4 ms |

Idle memory after `SELECT 1`, from `/proc/<pid>/smaps_rollup`:

| Workers alive | Private dirty per worker | Shared per worker | PSS per worker | Total PSS |
|---|---|---|---|---|
| 1 | 104 MiB | 3 MiB (262 MiB private including clean) | 262 MiB | 262 MiB |
| 4 | 97 MiB | 161 MiB | 137 MiB | 546 MiB |
| 16 | 66 MiB | 161 MiB | 76 MiB | 1 212 MiB |

## Iceberg reads (functional, not timed)

Measured 2026-10-08 through a loopback stub S3, with the request counts taken from its log. The table was 3 000 rows in 6 data files, under 2 identity partitions and 3 snapshots.

| Query | Data files read |
|---|---|
| Pinned full scan | 6 of 6 |
| Partition predicate (`part = 'a'`) | 3 of 6 |
| Min/max predicate on `long` or `timestamptz` | 2 of 6 |
| `count()` with no filter | 0 (answered from manifests) |
| ListObjectsV2 calls per pinned query | 0 (unpinned: 3) |

## Build cost of `iceberg` 0.10.1 (arrow 58) in `fabric/`

Measured 2026-10-08 (HS1 Task 1) with a throwaway probe crate in `fabric/`, `cargo build --release`, `jobs = 6`, on the shared build machine. The baseline links arrow 59 only. The `lake` build adds iceberg 0.10.1 and iceberg-catalog-rest 0.10.1, and exercises `fast_append` plus a REST commit.

| | Arrow 59 only | + iceberg 0.10.1 (arrow 58, parquet 58) | Delta |
|---|---|---|---|
| Crates compiled | 42 | +158 | +158 |
| Crates in the normal dependency tree | 65 | 260 | +195 |
| Release build time | 1 m 16 s | +3 m 19 s (incremental over the baseline) | +3 m 19 s |
| Binary size, unstripped | 2 638 408 B | 13 081 856 B | +10.4 MB |
| Binary size, stripped | 1 807 440 B | 9 286 448 B | +7.5 MB |
