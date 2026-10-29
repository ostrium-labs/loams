# BQ1-pg: the Postgres front door and the Loams Postgres compute

The harness behind [docs/benchmarks/2026-10-28-1b-qps/pg.md](../../docs/benchmarks/2026-10-28-1b-qps/pg.md)
(lane BQ1-pg of the 1B QPS benchmarks). pgbench 17.10 from the pinned image
(`PSQL_IMAGE` in `conformance/compute/pins.env`) measures these:

| Script | What it measures |
|---|---|
| `stock.sh` | Paths. One stock Postgres 17.10 server with TLS, measured plain, over TLS, through `loams-gateway` in splice mode (SSLRequest: SNI routing, then a byte splice) and through it in terminate mode (direct TLS: terminate and re-encrypt). Select-only with 64 and 1 clients, and TPC-B with 32 clients. |
| `scaling.sh pg-cores` | One server on 1, 2 and 4 cores. |
| `scaling.sh shards` | 1, 2 and 4 independent one-core servers, run together. |
| `scaling.sh gw-cores` | The gateway on 1, 2 and 4 cores, splice and terminate. |
| `compute.sh` | The Loams Postgres compute (`deploy/loams-postgres-dev`, compute-v17 with a pageserver and a safekeeper) at scale 10, where the data fits the compute's caches, and scale 50, where it does not. |
| `project.py` | The report's tables and the projection to 1B QPS, from `bench/results/bq1/pg/*.jsonl`. |

```sh
cargo build --release -p loams-gateway --bin loams-gateway   # through buildslot.sh -j 4
cp …/target/release/loams-gateway /mnt/Projects/bench-data/bq1-pg/
export GW_BIN=/mnt/Projects/bench-data/bq1-pg/loams-gateway
bench/pg/hold.sh stock.sh                  # waits for a quiet laptop, then the bench slot
bench/pg/hold.sh scaling.sh pg-cores shards gw-cores
bench/pg/hold.sh compute.sh
bench/pg/project.py
```

Core sets are fixed and disjoint: Postgres 0-5, gateway 6-7, pgbench 8-11 and storage 12-13
(`lib.sh`). Every run records the following:

- pgbench's TPS;
- p50, p99 and p99.9 from a 1% sampled per-transaction log;
- busy CPU per core set over the measured interval, from `/proc/stat`;
- the gateway process's own CPU per query;
- two seconds of background load on the bench's cores before the run, because the bench slot
  does not stop other agents' builds.

Data stays in RAM (a tmpfs inside the server container) or under `/mnt/Projects/bench-data/bq1-pg`,
and it is deleted after each run.
