# BQ1-kv: the Redis gateway toward 1B QPS

Harness for `docs/benchmarks/2026-10-28-1b-qps/kv.md` (lane BQ1-kv of the
coordinator's BQ1 brief).

- `loadgen/`: `kv-loadgen`, a closed-loop RESP GET/SET load generator. The host
  has neither `valkey-benchmark` nor `memtier_benchmark`, so this measures the
  same things: connections × pipeline depth in flight, uniform keys, latency
  per request as its batch's round trip, p50/p99/p99.9 from a log-linear
  histogram (about 3% precision). Output is one JSON object.
- `run.sh`: starts 1..N `loams-gateway --module resp` processes pinned with
  `taskset`, runs the load generator on disjoint cores, records the CPU
  seconds of each server process (and of TiKV/PD via `EXTRA_PIDS`), deletes
  the data, and writes `bench/results/bq1/kv/<name>.json`.
- `suite.sh`: the runs in the report, in the order they were taken.

Build (release, shared target):

```
.he0-shared/buildslot.sh cargo build --release -j 4 -p kv-loadgen
.he0-shared/buildslot.sh cargo build --release -j 4 -p loams-gateway --features resp-tikv
```

Every measurement runs under the exclusive bench lock:

```
.he0-shared/benchslot.sh bq1-kv bench/kv/suite.sh <phase>
```

TiKV runs use `scripts/tikv/playground.sh start --tag loams-bq1kv --force`
(PD and TiKV v8.5.8, API v2) and stop it afterwards.
