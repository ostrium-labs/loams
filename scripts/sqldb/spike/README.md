# Loams SQL spike stack and baselines (SQ1 Task 1)

A PD plus one TiKV on their own ports, with keyspace-mode TiDB containers started per measurement. It runs beside the desktop's `deploy/tikv` stack (19379/20160) and never uses ports 5180 or 8090. The `LOAMS_IT_SQLDB=1` tests of later tasks reuse it.

| What | Where |
|---|---|
| PD client / peer | `127.0.0.1:29379` / `29380` |
| TiKV / status | `127.0.0.1:30160` / `30180` |
| TiDB N (MySQL / status) | `127.0.0.1:24000+N` / `25000+N` |
| Compose project | `loams-sqldb-spike` (`deploy/sqldb/spike/compose.yaml`) |
| Images | by digest from `release/sqldb-images.toml` |
| Output | `target-spike/*.jsonl` (git-ignored) |

Needs Podman with `podman compose` (or Docker Compose), the `mysql`/`mariadb` client, `curl`, `jq` and `envsubst`; `sysbench` is used if installed. About 6 GB of free RAM: PD is capped at 1 GiB, TiKV at 3 GiB and each TiDB at `TIDB_MEM` (1g).

```sh
scripts/sqldb/spike/up.sh          # PD + TiKV, waits until the store is Up
scripts/sqldb/spike/keyspace.sh    # PD keyspace API latency (20 creates, DISABLED, ARCHIVED)
scripts/sqldb/spike/coldstart.sh   # bootstrap and warm-restart cold starts, TiDB RSS (~30 min)
scripts/sqldb/spike/regions.sh     # regions per database, 20/50/100 keyspaces (~25 min)
scripts/sqldb/spike/report.sh      # renders docs/sqldb/performance.md
scripts/sqldb/spike/down.sh -v     # removes TiDB containers, PD, TiKV and the volumes
```

Smaller runs for a smoke test: `BOOT_RUNS=1 WARM_RUNS=1 TABLE_SETS=0 LOAD_SECS=5 coldstart.sh`, `CHECKPOINTS=2 HB_WINDOW=5 regions.sh`; set `SPIKE_OUT` to keep their output away from `target-spike/`.

Every TiDB the scripts start has `keyspace-name` set (`lib.sh` refuses an empty one): a TiDB without it would be a second cluster GC worker (D260). Each keyspace is created through `POST /pd/api/v2/keyspaces` first; `pd.toml` pre-allocates none.
