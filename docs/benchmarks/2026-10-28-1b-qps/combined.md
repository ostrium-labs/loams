# BQ1 combined run: every engine at once, native APIs only

Lane BQ1-ALL, 2026-10-29. Harness: [`bench/all`](../../../bench/all/README.md).
Raw results: [`bench/results/bq1/all/`](../../../bench/results/bq1/all/).

**Status: measured on one 14-core laptop. The 1B-QPS figures in this report
are projections, not measurements.** The 2026-10-30 re-run, with every BQ1 fix
in, is in [Follow-up](#follow-up-fixes-and-re-runs-2026-10-29-and-30):
1,231,462 QPS at depth 16 (two gated runs). That projects to about 812 nodes
for 1B, down from 1,112.

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
| sql | 7,630 | 33,920 | 1.50 ¹ | 78.2 / 62.8 | 22,621 ¹ |
| kv-tikv | 8,436 | 6,282 | 0.64 ¹ | 68.5 / 64.3 | 9,784 ¹ |
| git | 186 | 2,228 | 0.50 | 29.7 / 29.6 | 4,455 |
| graph | 32 | 864 | 0.50 | 29.8 / 29.7 | 1,729 |
| durable | 506 | 534 | 0.78 | 47.2 / 46.2 | 684 |
| house | 20 | 0 (failed) | 0.66 | 51.2 / 39.0 | – |
| **Total** | **575,986** | **899,412** | **10.07** | | **89,360** |

¹ Corrected on 2026-10-30. The first version charged the one TiKV core (core 0) entirely to kv-tikv, although sql's reads ran on it too. It is now split evenly between the two, like every other shared core. The totals do not change. The CPU-s column is as first published.

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
| sql | 22,621 ¹ | 44,207 | 3,158 | PD's TSO (one timestamp per transaction) and TiKV store cores. |
| kv-tikv | 9,784 ¹ | 102,205 | 7,300 | Here, one TiKV core shared with sql. Then PD's TSO, one per transaction. |
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

Status on 2026-10-30: every item below has a fix or a pull request. See
[Follow-up](#follow-up-fixes-and-re-runs-2026-10-29-and-30).

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

## Follow-up: fixes and re-runs (2026-10-29 and 30)

Every finding above got a fix, or a pull request with before/after numbers
from this harness:

- The isolated before/after runs ran the slots in question alone, back to
  back on the same machine, with 30 s windows. They are relative comparisons.
- The combined re-run below ran with all of the fixes in.

### The fixes

| Finding | Fix | Before → after (isolated) |
|---|---|---|
| Resonate: one SQLite commit per promise, and hundreds of request threads parked on the connection lock (424 workers in `futex_wait`) | resonate#29 (merged): one writer thread owns the connection; group commit of up to 256 jobs per transaction, one savepoint each. Durability is unchanged. | depth 1: 826 → **3,598**/s, p99 45 → 21 ms; depth 16: 3,648 → 4,197/s |
| Graph: an indexed point lookup built a set of every node with the label on every query | ostrium-labs/grafeo#2 (LG-0007) and loams#1578: the label is checked per candidate, on both the equality and `IN` paths | point lookup: 96 → **24,268**/s (p50 10.5 → 0.04 ms); 16 ids per statement: 2,802 → **57,731** ids/s |
| House: `202` and 0 QPS at depth 16 | loams#1597: the worker pool and the memory admission queue now serve waiters in order. A statement that just finished took the worker, or the memory, back ahead of a queued one, which then waited 30 s. | depth 16 in the combined run: 0 QPS with 1 error → **14 QPS, no error**. Both 202s are reproduced by new tests that fail before the fix. |
| sql and kv-tikv shared one TiKV core | loams#1577: the kv slot gets its own PD and TiKV on core 9, and the House load moves to core 11 | depth 1: sql 5,136 → 6,433, tikv.get 3,667 → 5,889; depth 16: about even (42.8k → 42.4k together) |

### Combined re-run with every fix (mean of two runs)

**Status: measured.** The two runs were taken on 2026-10-30 inside the bench
lock and the quiet window, and both passed the thermal gate described below.

| | Depth 1 (two runs) | Depth 16 (two runs) |
|---|---|---|
| Total QPS | 747,498 / 716,755 | 1,219,000 / 1,243,923 |
| Busy cores (of 14) | 10.32 / 10.78 | 9.44 / 9.41 |
| Load average, 1 min mean | 29.2 / 32.1 | 39.1 / 37.4 |
| iowait | 15.9 % / 13.6 % | 21.6 % / 18.7 % |
| CPU at the start, and its max | 60 / 88 °C, 62 / 86 °C | 65 / 90 °C, 61 / 89 °C |

| Engine (slot) | QPS, depth 1 | QPS, depth 16 | Busy cores (d16) | QPS per busy core (d16) |
|---|---:|---:|---:|---:|
| meter | 458,357 | 450,295 | 0.50 | 900,590 |
| iggy | 22,173 | 299,828 | 0.61 | 487,524 |
| gateway | 91,432 | 158,704 | 1.31 | 121,148 |
| pg | 89,008 | 149,408 | 1.97 | 75,841 |
| kv (in-process) | 32,962 | 62,036 | 0.50 | 124,072 |
| sql | 14,572 | 56,539 | 1.81 | 31,324 |
| graph | 9,579 | 48,907 | 0.50 | 97,814 |
| kv-tikv | 13,255 | 3,986 | 0.41 | 9,842 |
| git | 131 | 1,160 | 0.50 | 2,321 |
| durable (Resonate) | 642 | 586 | 0.47 | 1,246 |
| house | 16 | 14 | 0.85 | 16 |
| **Total** | **732,127** | **1,231,462** | **9.43** | **130,659** |

**Projected, not measured.** The assumptions above still apply. At this mix,
1B QPS takes **about 812 nodes** of this laptop class, down from 1,112, or
about 7,650 busy cores. The network engines alone add up to 669,063 QPS at
depth 16, which is about 1,495 nodes.

What the re-run says:

- **Graph and the House are now part of the total.** Graph reaches
  48,907 lookups/s on half a low-power core (864 before). The House completes
  at depth 16 instead of failing. It is still about 15 QPS on one core,
  because one chDB worker runs one query at a time. More queries per worker,
  or the in-process House engine (design 75), is the remaining fix.
- **Resonate's group commit hardly shows in the full mix.** It is 586/s at
  depth 16, against 4,197/s alone. Its server core is only about half busy
  while the machine's iowait is about 20 %: in the full mix it waits on the
  disk (WAL checkpoints, alongside git's and TiKV's fsyncs), not on the lock.
- **kv-tikv gets slower at depth 16 on its own cluster:** 3,986/s with a 2 s
  p99, on a TiKV core that is only 0.41 busy. Its PD and TiKV share one core,
  and a transaction waits on PD's timestamps, so the wait is latency rather
  than CPU. At depth 1 it improved (8,436 → 13,255). This one is open:
  - The next step is to try PD on its own core.
  - Batched TSO requests would help on any core layout.
- **Not attributable to the fixes:**
  - pg, the gateway and Iggy got faster although they were not changed (pg
    93k → 149k, gateway 100k → 159k).
  - Some of that is the new budget (the House load left core 9).
  - Some is the machine: run A and B's temperatures were not recorded, and
    the hot runs below show swings of more than 40 % with identical binaries.

### The thermal gate, and the runs that do not count

The laptop's cooling is failing. The package sits at 86 °C at a load average
of 1, with the fan near its maximum (coordinator, 2026-10-30). Between the
fixes and this re-run, repeat combined runs with identical binaries fell
from 1.27M to 921k and then to 548k QPS at depth 16. The meter, alone on its
core, fell from 460k to 343k over the same runs.

So `run.sh` now:
- records the CPU package temperature with every sample;
- starts a round only below 70 °C (`MAX_START_TEMP`). It waits at most
  `COOL_WAIT`, then skips the round;
- labels a result `throttled` when it started at 70 °C or more, or peaked at
  95 °C or more.

The combined runs of 2026-10-29 afternoon recorded no temperature. They stay
in `bench/results/bq1/all/ungated-2026-10-29/` and are not used above.

Run A and B, the first headline at the top of this report, recorded no
temperature either. Their numbers stand as measured, with that caveat.

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
