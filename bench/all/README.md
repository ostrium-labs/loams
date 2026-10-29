# bench/all: the BQ1 combined run

This directory runs every Loams engine through its native API **at the same
time** on one machine, and reports per-engine QPS, the total aggregate QPS
and the efficiency per core from that single run. It is lane BQ1-ALL of the
1B-QPS wave (`docs/benchmarks/2026-10-28-1b-qps/`). The report is
[`combined.md`](../../docs/benchmarks/2026-10-28-1b-qps/combined.md).

| File | What it does |
|---|---|
| `CONTRACT.md` | The `native-load.sh` interface and its JSON line. |
| `budget.sh` | The core and memory budget (below). |
| `engines.sh` | Starts the servers that `bench/all` owns, pinned and memory-capped: PD, TiKV and TiDB; Postgres; Resonate; loams-gateway; and iggy-server. |
| `native/*.sh` | First-version native-load scripts for kv (embedded and TiKV), sql, pg, durable and gateway. They drive `bq1-native` (`loadgen/`) against the servers that `engines.sh` started. `native/iggy.sh` drives `iggy-bench` in chunks that fit the window. |
| `loadgen/` | `bq1-native`, the Rust closed-loop load generator. It covers loams-kv (`Store` on the embedded or TiKV backend), PG wire (pipelined tokio-postgres), MySQL wire, and HTTP/2 (h2c). |
| `run.sh` | Starts the servers. Then, for each depth, it starts every slot's script so that their windows overlap, samples `/proc/stat` once a second, and aggregates the results. |
| `aggregate.py` | Writes `bench/results/bq1/all/<ts>-d<depth>.{json,md}`. |

## Running it

```sh
cargo build --release -j 4 -p bq1-native -p loams-gateway      # via .he0-shared/buildslot.sh
cp target/release/{bq1-native,loams-gateway} /mnt/Projects/bench-data/bq1-all/bin/
# plus resonate, iggy-server and iggy-bench in the same bin directory
NATIVE_HOUSE=…/bench/house/native-load.sh NATIVE_GRAPH=… NATIVE_METER=… NATIVE_GIT=… \
  .he0-shared/benchslot.sh bq1-all bench/all/run.sh --quiet --duration 60 --depths 1,16
```

Before a lane's script is merged, `NATIVE_<SLOT>` points at it in that lane's
worktree. A slot without a script is skipped, and the result lists it.

## Slots

| Slot | Engine | Native API | Script |
|---|---|---|---|
| kv | kv | loams-kv `Store` in-process, on an in-memory redb store | `native/kv.sh` |
| kv-tikv | kv | loams-kv `Store` on TiKV (txn get/put; `batch_get` at depth N) | `native/kv-tikv.sh` |
| sql | sql | MySQL wire to keyspace-mode TiDB (point select; `IN (…)` at depth N) | `native/sql.sh` |
| pg | pg | PG wire to Postgres 17.10 (point select; N in flight per connection) | `native/pg.sh` |
| house | house | ClickHouse native TCP (`clickhouse benchmark`) | `bench/house/native-load.sh` |
| iggy | iggy | Iggy native TCP: `iggy-bench` producers, depth is messages per batch, run in chunks that fit the window | `native/iggy.sh` |
| durable | durable | Resonate HTTP `promise.create` over HTTP/2 (N streams per connection) | `native/durable.sh` |
| gateway | gateway | loams-gateway HTTP stack over HTTP/2 (N streams per connection) | `native/gateway.sh` |
| graph | graph | Grafeo session API, in-process (`graph-qps-bench --path native`) | `bench/graph/native-load.sh` |
| meter | meter | `Gate::check_many` and `MeterSink::emit_batch`, in-process | `bench/meter/native-load.sh` |
| git | git | git-on-S3 bucket WAL, ref updates on a file store, in-process | `bench/git/native-load.sh` |

RisingWave is optional and not in the default run. Its lane's script starts a
four-process cluster with Postgres and RustFS, which does not fit the
15 GB budget next to everything else.

## Core budget (14 cores, no SMT)

The CPU is an Intel Core Ultra 5 225H: cores 0–3 are P-cores (4.9 GHz), 4–11
are E-cores (4.4 GHz), and 12–13 are low-power E-cores (2.5 GHz).

| Core | Server | Load generator |
|---|---|---|
| 0 | TiKV (used by sql and kv-tikv) | |
| 1 | TiDB + PD | |
| 2 | Postgres 17.10 | |
| 3 | House (loams-fabric + workers) | |
| 4 | iggy-server | |
| 5 | Resonate (SQLite) | |
| 6 | loams-gateway | |
| 7 | | sql (MySQL wire) + kv-tikv |
| 8 | | pg |
| 9 | | house (`clickhouse benchmark`) |
| 10 | | iggy (`iggy-bench`) |
| 11 | | durable + gateway |
| 12 | kv (in-process) + graph (in-process) | (same process) |
| 13 | meter (in-process) + git (in-process) | (same process) |

The rules behind it:

- Every server gets a core of its own, and so does every network engine's
  load generator, except the two that share core 7 and the two that share
  core 11.
- The in-process engines have no network hop, so server and load are one
  process. Two of them share each low-power core.
- An engine's CPU is the busy time of the cores it runs on, measured from
  `/proc/stat`. A shared core is split evenly between the slots that use it.
  This works because everything is pinned to disjoint cores: containers with
  `--cpuset-cpus`, and host processes with `taskset`.

## Memory budget (15 GB, about 11 GB free on an idle laptop)

| Engine | Cap | How |
|---|---|---|
| TiKV | 1.4 GB | container `--memory`, `memory-usage-limit` 1.1 GB, block cache 256 MB, one store, `max-replicas = 1` |
| TiDB | 1.2 GB | container `--memory` |
| PD | 512 MB | container `--memory` |
| Postgres | 768 MB | container `--memory`, `shared_buffers` 256 MB, PGDATA on a 1 GB tmpfs |
| House | about 1.5 GB | 1M-row `kv` table, one worker per two cores (one) |
| Iggy | about 0.5 GB | data volume sized to the window by a calibration run |
| graph | about 1 GB | 100,000 persons (`PERSONS`) |
| kv, kv-tikv, sql, pg datasets | small | 100,000 keys or rows each |
| Resonate, gateway, meter, git | under 200 MB each | |

That is about 8 GB in all, which leaves room for the desktop and the page
cache. Data lives under `/mnt/Projects/bench-data/bq1-all` (and each lane's
own directory for lane scripts), and `engines.sh down` removes it.

## Alignment of the windows

Scripts that take `--start-at` (`native/*`) start their measured
window at the same second, T0. The older lane scripts (House, graph, meter,
git) have no `--start-at`, so `run.sh` launches each one early by its setup
time (`*_LEAD`), which lands its window near T0. Every result records each
slot's window and its overlap with [T0, T0 + duration], as intersection over
union, so a window that runs long scores low too. The JSON also keeps
the per-core busy fractions over the window.
