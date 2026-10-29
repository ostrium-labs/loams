# BQ1-sql: Loams SQL benchmark harness

Loams SQL is TiDB with a keyspace on TiKV. This harness benchmarks it toward the 1B QPS goal. The report is in [`docs/benchmarks/2026-10-28-1b-qps/sql.md`](../../docs/benchmarks/2026-10-28-1b-qps/sql.md), and results are in `bench/results/bq1/sql/`.

| File | Purpose |
|---|---|
| `cluster.sh up\|down` | Brings up one PD, N TiKV stores (API v2, one replica per region) and M keyspace-mode TiDBs under Podman, each pinned to its own cpuset. Images come from `release/sqldb-images.toml`. |
| `run.sh <scenario>` | Runs one scenario (`single`, `single-tikv`, `units-1`, `units-2`, `units-4`) inside one bench-lock hold, with the quiet window. |
| `native-load.sh --duration --cores --depth [--ops]` | BQ1-ALL's combined-run entry point. Prints JSON lines on stdout. |
| `loadgen/` | Go MySQL-wire load generator: sysbench-style `sbtest` schema, point selects (depth 1, or a batched `IN` point get at depth N) and the `oltp_read_write` mix. It also reports PD/TiDB counter rates. |
| `tikvload/` | Go TiKV native-API load generator: raw Get/BatchGet and transactional snapshot Get/BatchGet. It uses client-go at the revision TiDB v8.5.8 pins. |

Build the two load generators (about 50 MB; sysbench is not installed on the bench host):

```sh
(cd bench/sql/loadgen && go build -o /mnt/Projects/bench-data/bq1-sql/loadgen .)
(cd bench/sql/tikvload && GOTOOLCHAIN=go1.25.12 go build -o /mnt/Projects/bench-data/bq1-sql/tikvload .)
```

Run a scenario under the lock:

```sh
.he0-shared/benchslot.sh bq1-sql bench/sql/run.sh single
```

The cluster's data lives under `/mnt/Projects/bench-data/bq1-sql/cluster`, and `cluster.sh down` removes it.
