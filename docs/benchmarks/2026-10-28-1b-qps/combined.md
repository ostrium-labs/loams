# BQ1 combined run: every engine at once, native APIs only

Lane BQ1-ALL, 2026-10-29. Harness: [`bench/all`](../../../bench/all/README.md).
Raw results: [`bench/results/bq1/all/`](../../../bench/results/bq1/all/).

**Status: measured on one 14-core laptop. The 1B-QPS figures in this report
are projections, not measurements.**

## What was run

Eleven load slots ran at the same time on one machine. They covered all ten
engines, each through its own native API:

| Slot | Native API | Ops |
|---|---|---|
| kv | loams-kv `Store`, in-process, in-memory redb | `get` and `put` (depth N: a transaction of N keys, `batch_get`) |
| kv-tikv | loams-kv `Store` on TiKV (one store, API v2 keyspace) | `tikv.get` and `tikv.put` |
| sql | MySQL wire to keyspace-mode TiDB (on the same TiKV) | `point-select` (depth N: `IN (…)` of N ids) |
| pg | PG wire to Postgres 17.10 (stock compute) | `point-select` (depth N: N queries in flight per connection) |
| house | ClickHouse native TCP to the House (chDB worker) | `point` (depth N: N connections) |
| iggy | Iggy native TCP | `produce` (depth N: N messages per batch) |
| durable | Resonate HTTP on SQLite | `promise.create` (depth N: N HTTP/2 streams per connection) |
| gateway | loams-gateway's HTTP stack, HTTP/2 (h2c) | `http2.get` of `/healthz` (N streams per connection) |
| graph | Grafeo session API, in-process | `point` (depth N: N ids per statement) |
| meter | `Gate::check_many` and `MeterSink::emit_batch`, in-process | `gate.check`, `sink.emit` (N per call) |
| git | git-on-S3 bucket WAL on a file store, in-process | `push` (depth N: N refs per push) |

Every server and every load generator was pinned to its own core or cores
(the [core budget](../../../bench/all/README.md#core-budget-14-cores-no-smt)).
The four in-process engines shared the two low-power cores in pairs. Memory
was capped at about 8 GB in all.

Each depth ran for 60 s, with the windows aligned on one start second. The
whole run was repeated (runs A and B), so every number below is the mean of
two runs. Both runs were taken inside the bench lock and the quiet window.
Before the start, the load average was under 2 and no `rustc` was running.

## Host conditions in the window

| | Depth 1 (A / B) | Depth 16 (A / B) |
|---|---|---|
| Load average, 1 min mean | 29.2 / 30.7 | 50.1 / 48.8 |
| iowait | 13.5 % / 14.2 % | 20.4 % / 18.5 % |
| All 14 cores busy | 76.5 % / 75.5 % | 71.8 % / 71.9 % |
| Busy cores (sum over the engines) | 10.71 / 10.58 | 10.05 / 10.08 |

The quiet window kept other work off the machine. The high load average comes
from the run itself: about 11 engines, each with its own thread pools, on 14
cores. The iowait comes from the engines that fsync on every commit (Resonate
on SQLite, git pushes, TiKV's Raft log), and those engines leave about
3.5 cores idle.

## Results (mean of runs A and B)

QPS counts operations. A batch of N counts as N.

"Busy cores" is the measured busy time of an engine's cores over the window:
its server cores plus its share of its load-generator cores. A core shared by
two slots is split evenly. "CPU-s" is that busy time in core-seconds over the
60 s window.

| Engine (slot) | QPS, depth 1 | QPS, depth 16 | Busy cores (d16) | CPU-s (d1 / d16) | QPS per busy core (d16) |
|---|---:|---:|---:|---:|---:|
| meter | 398,994 | 392,602 | 0.50 | 29.7 / 29.6 | 785,203 |
| iggy | 15,784 | 216,138 | 1.02 | 72.3 / 60.5 | 211,900 |
| gateway | 61,350 | 100,410 | 1.48 | 87.1 / 88.1 | 67,616 |
| pg | 55,940 | 93,451 | 1.98 | 109.7 / 117.5 | 47,197 |
| kv (in-process) | 27,109 | 52,984 | 0.50 | 29.8 / 29.7 | 105,969 |
| sql | 7,630 | 33,920 | 1.06 | 78.2 / 62.8 | 32,000 |
| kv-tikv | 8,436 | 6,282 | 1.08 | 68.5 / 64.3 | 5,816 |
| git | 186 | 2,228 | 0.50 | 29.7 / 29.6 | 4,455 |
| graph | 32 | 864 | 0.50 | 29.8 / 29.7 | 1,729 |
| durable | 506 | 534 | 0.78 | 47.2 / 46.2 | 684 |
| house | 20 | 0 (failed) | 0.66 | 51.2 / 39.0 | – |
| **Total** | **575,986** | **899,412** | **10.07** | | **89,360** |

Runs A and B agree to within 2 % on the totals:

- depth 1: 569,660 and 582,312;
- depth 16: 903,401 and 895,423.

The per-run tables, with p50, p99 and p99.9 per operation, are in the
`*.md` files next to the JSON. Latencies at depth 1, p50 / p99:

| Op | p50 / p99 (ms) |
|---|---|
| kv get | 0.013 / 0.35 |
| pg point-select | 0.27 / 1.1 |
| gateway | 0.26 / 0.46 |
| sql | 3.9 / 11 |
| tikv.get | 1.8 / 5.3 |
| iggy produce | 0.21 / 1.3 |
| Resonate create | 0.76 / 131 |

### What the numbers say

- **The aggregate is lopsided.** Of the pipelined total, 44 % is the meter
  gate (in-process, 0.5 core), 24 % Iggy, 11 % the gateway and 10 %
  Postgres. The network engines alone (everything except kv, graph, meter
  and git) add up to 149,665 QPS at depth 1 and 450,734 at depth 16.
- **Pipelining pays where the protocol carries it.** The gains from depth 1
  to depth 16:
  - Iggy: 13.7× (it batches messages).
  - sql: 4.4× (one `IN` list replaces 16 round trips).
  - git: 12× (16 refs per push).
  - graph: 27× (16 ids per statement).
  - pg (1.7×), the gateway (1.6×) and kv (2×) gain less, because their
    servers' single core was already busy at depth 1.
  - The meter does not gain. The gate's call costs about 1 µs, so batching
    has little to remove.
- **Three engines are bound by I/O or by their design, not by CPU.**
  - Resonate on SQLite stays near 500 promise creations/s at any depth, and
    its p99 grows to seconds at depth 16. Every create is its own SQLite
    commit.
  - The House answers one query at a time per worker (20 QPS on one server
    core). At depth 16 under full-machine contention, one query failed and
    `clickhouse benchmark` stopped, which recorded 0 QPS with 1 error. Run
    alone at depth 16, the same script gives 33 QPS.
  - Graph point lookups take about 30 ms each at depth 1, which looks like a
    scan rather than an index lookup.
- **TiKV is the shared bottleneck of sql and kv-tikv.** One TiKV on one core,
  with PD on TiDB's core, serves both. kv-tikv *drops* from depth 1 to depth
  16 (8.4k to 6.3k), and its p99 reaches 1.5 s. That happens because sql's
  batched reads take the store's single core.

## Projection to 1B QPS (projected, not measured)

These figures use the **pipelined (depth 16)** numbers. They assume linear
scale-out with no cross-node cost, and the assumptions are listed below.

| Basis | Measured | Needed for 1B QPS |
|---|---|---|
| This mix on one node (14 cores; the laptop is the node) | 899,412 QPS per node | **about 1,112 nodes** |
| This mix per busy core | 89,360 QPS per busy core | about 11,200 busy cores |
| The network engines only, same node | 450,734 QPS per node | about 2,220 nodes |

The same projection per engine, as if that engine alone carried 1B QPS at its
measured efficiency:

| Engine | QPS per busy core (d16) | Busy cores for 1B | 14-core nodes for 1B | What breaks linear scaling first |
|---|---:|---:|---:|---|
| meter | 785,203 | 1,274 | 91 | Nothing on the gate path (lease hits are local). The sink's flush to the stream is the limit (`sink.emit`, about 33k/s on a quarter core here). |
| iggy | 211,900 | 4,719 | 337 | Partition count per stream, and a server per shard. |
| kv (in-process) | 105,969 | 9,437 | 674 | The single embedded committer: puts serialise (p50 21 ms at depth 16). |
| gateway | 67,616 | 14,789 | 1,056 | Nothing shared. It scales by adding gateways behind L4. |
| pg | 47,197 | 21,188 | 1,513 | One writer per database. Reads scale by replica or shard. |
| sql | 32,000 | 31,250 | 2,232 | PD's TSO (one timestamp per transaction) and TiKV store cores. |
| kv-tikv | 5,816 | 171,933 | 12,281 | Here, one TiKV core shared with sql. Then PD's TSO, one per transaction. |
| git | 4,455 | 224,467 | 16,033 | One fsync per WAL append on a file store. On S3, the PUT latency. |
| graph | 1,729 | 578,369 | 41,312 | Point lookups without an index (about 30 ms each). |
| durable (Resonate) | 684 | 1,462,043 | 104,432 | One SQLite commit per promise, with no group commit. |
| house | – | – | – | One query at a time per chDB worker. No number is meaningful. |

Assumptions:

- Throughput scales linearly in nodes. The per-engine lane reports give the
  measured scaling efficiency; this run does not measure scaling.
- A node is this laptop: 14 cores, 15 GB, local NVMe and loopback networking.
  Server-class cores are faster. Real networking adds cost that loopback
  hides.
- The load generators ran on the same machine, and their cores count in "busy
  cores". For the network engines, a fleet does not carry its clients' cores,
  so the per-core figures for those engines are conservative by up to about
  2×.
- The mix is whatever each engine reached here. It is not a workload model of
  1B real requests.

## What would raise the per-core numbers most

1. **Resonate: group commit** (batch SQLite or Postgres commits across
   in-flight promises). Today it is about 600 creates per core.
2. **The House: more than one query in flight per worker**, or the in-process
   House engine (design 75) instead of a chDB worker per query. It should
   also survive one failed query: `clickhouse benchmark` needs
   `--continue_on_errors`.
3. **Graph: an index on the point-lookup key.** About 30 ms per lookup points
   to a scan.
4. **TiKV paths: give TiKV more than one core, and batch TSO requests.** At
   depth 16, kv-tikv and sql compete for one store core.
5. **The embedded kv committer:** put latency at depth 16 (p50 21 ms) holds
   back the gets that share its core.

## Caveats

- **Postgres:** this is the stock Postgres 17.10 image. The Loams Postgres
  compute stack (pageserver and safekeeper) was not on the machine and does
  not fit the memory budget next to everything else. The PG lane measures the
  compute.
- **Gateway:** loams-gateway does not front the Loams Connect API yet, so its
  slot measures the gateway's HTTP stack (`/healthz` over HTTP/2). That is
  the same surface `bench/gateway/native-load.sh` measures. This run includes
  the HTTP/2 and pipelining fix (#1521).
- **Iggy:** Iggy is produce only. The lane script's data-volume sizing ran
  166 s for a 60 s window in a first attempt, so `bench/all/native/iggy.sh`
  runs `iggy-bench` in roughly 10 s chunks until the window ends. A consumer
  group in later chunks reported nothing, so consume is left to the Iggy
  lane. Iggy's p50 is the message-weighted mean of the chunks' p50s, and its
  p99 and p99.9 are the worst chunk's.
- **Window alignment:** the House, graph, meter and git scripts predate
  `--start-at`. They were launched early by their measured setup time.
  - graph, meter and git overlap the common window at 0.96–0.99
    (intersection over union).
  - Iggy overlaps at 0.90–0.95, because its last chunk runs a few seconds
    past the window.
  - The House overlaps at 0.72–0.82 at depth 1 and 0.33 at depth 16, where
    its run stopped early.
- **RisingWave** is not in the run (it is optional, and its cluster does not
  fit the memory budget alongside everything else).
- **Earlier attempt:** a first full run, before the Iggy driver and the
  overlap metric were fixed, totalled 627,513 QPS at depth 1 and 988,722 at
  depth 16. It is superseded and not in the results directory.
- **Lint changes:** after the measured runs, `run.sh` had shellcheck fixes
  (how the argument list is split). Behaviour is unchanged.

## Reproducing

Build `bq1-native` and `loams-gateway` (release). Put them in
`/mnt/Projects/bench-data/bq1-all/bin` together with `resonate`,
`iggy-server` and `iggy-bench`. Point `NATIVE_HOUSE`, `NATIVE_GRAPH`,
`NATIVE_METER` and `NATIVE_GIT` at the lane scripts until those are merged.
Then run:

```sh
SETUP_S=60 .he0-shared/benchslot.sh bq1-all bench/all/run.sh --quiet --duration 60 --depths 1,16
```

Script versions are recorded per slot in each result JSON (`script`,
`script_sha256_12`, `args`). The runs used these lane commits:

- House `9d6e69078`
- graph and git `b6292490f`
- meter (bq1-meter worktree, uncommitted `native-load.sh`, with
  `meter-loadgen` rebuilt from it on 2026-10-29)
