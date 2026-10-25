# LC0 Task 1: Starter tenant cells (spike)

Spike code for [LC1](../../docs/plans/2026-10-23-lc1-loams-cloud.md) LC0 Task 1 (design 78 §15.1, D1717). It is not product code: not wired into a crate, the CLI or the release, and deleted or kept as a fixture by LC1's first task.

Run on 2026-10-23 against one free TiDB Cloud Starter instance, `lc0-cells` (`aws-us-east-1`, TiDB `v8.5.3-serverless`, monthly spending limit 0, so free quota only), in the owner's no-card LC0 organization. The client ran on a Linux workstation about **206 ms** from the gateway (`SELECT 1` p50). Every latency below includes that round trip.

## How to run

```bash
set -a; . ~/tidbcloud-lc0.env; set +a           # TiDB Cloud API keys, never printed
export PATH="$HOME/.ti/bin:$PATH"               # ti 0.2.10
ti db create-db-cluster --db-cluster-type starter --db-cluster-name lc0-cells --wait
ti db create-db-sql-users --db-cluster-id <id>  # writes ~/.ti/db_users/<id>/credentials (0600)
uv run --no-project --with pymysql==1.1.2 python cells.py \
  --cluster-id <id> --prefix <user_prefix> --steps 50,200,500 --out out/run1.json
uv run --no-project --with pymysql==1.1.2 python storage_probe.py <id> <user_prefix>
./teardown.sh <id>                              # teardown_leaves_nothing
```

- `cells.py` reads the `ti`-managed admin user (`role_admin`) from `~/.ti/db_users/<id>/credentials`.
- Tenant passwords are generated in memory and never written or printed.
- Tenant *i* gets database `lc0t_<i>` and user `<prefix>.lc0t<i>`, with `GRANT ALL ON lc0t_<i>.*` and nothing else.
- `out/` holds the raw results. They contain no secrets: the user prefix is not one.

## Results

**Tests:**

| Test | Result |
|---|---|
| `cell_grants_isolate_tenants` | **Pass** at N = 50, 200 and 500: 10 probe pairs per phase, 25 probes each, no leak |
| `cell_storage_is_visible_per_tenant` | **Fail, for bytes.** Row counts are visible per tenant; bytes are not (below) |
| `teardown_leaves_nothing` | **Pass**: SQL objects (0 databases, 0 users left), then the instance and the local credentials (`teardown.sh`) |

The first full run (`out/run1.json`) used a storage criterion that later turned out to be wrong: any `TABLE_STORAGE_STATS` row. `storage_probe.py` showed that figure is per region, not per tenant, so the criterion now also requires distinct values per tenant. `out/run2-n50.json` is the corrected rerun at N = 50.

**DDL cost** (serial, on one admin connection, ms):

| N | `CREATE DATABASE` p50 / p95 | `CREATE USER` p50 / p95 | `GRANT` p50 / p95 | Provisioning time for the phase |
|---|---|---|---|---|
| 1–50 | 305 / 381 | 252 / 317 | 281 / 331 | 44 s for 50 tenants |
| 51–200 | 307 / 354 | 302 / 321 | 288 / 325 | 133 s for 150 |
| 201–500 | 306 / 335 | 303 / 313 | 301 / 312 | 264 s for 300 |

- DDL cost does not grow with N up to 500. Each statement costs about one round trip plus roughly 100 ms.
- A tenant costs about **0.9 s** of serial provisioning from this client.
- `DROP DATABASE` plus `DROP USER` for 500 tenants took about 6 minutes.

**Workload:** each tenant connected, created a table, inserted 20 rows, ran two reads, an update and a delete. The runs used 16 client threads, and every tenant ran its workload again in each phase.

| N | Errors | Connect p50 / p95 | Session p50 / p95 |
|---|---|---|---|
| 50 | 0 | 1.43 s / 8.4 s | 3.2 s / 10.2 s |
| 200 | 0 | 1.35 s / 3.1 s | 3.1 s / 5.1 s |
| 500 | 0 | 1.33 s / 2.3 s | 3.0 s / 3.9 s |

- A new TLS connection costs about **1.3 s** at this distance, about 6 round trips.
- The p95 tail at the start of a burst (8 s at N = 50, in both full runs) looks like the gateway pacing a burst of new connections. Device clients should keep connections pooled, and the control plane's HTTP driver avoids it.

**Isolation:** tenant A probing tenant B. Every probe was denied or filtered.

| Probe | Observed as tenant A |
|---|---|
| `SELECT` / `INSERT` on B's table | 1142, denied |
| `USE` B; `CREATE DATABASE` | 1044, denied |
| `SHOW DATABASES`, `information_schema.SCHEMATA`, `.TABLES` | Own database only |
| `information_schema.COLUMNS` / `STATISTICS` / `TABLE_STORAGE_STATS` / `DDL_JOBS` for B | No rows |
| `TIKV_REGION_STATUS`, `SLOW_QUERY`, `STATEMENTS_SUMMARY` | 1142, denied |
| `ADMIN SHOW DDL JOBS` | 8121, needs `SUPER` |
| `SHOW STATS_META`, `mysql.user`, `mysql.stats_meta` | 1142, denied |
| `SHOW GRANTS FOR` B; `GRANT … ON B` to self | Denied |
| `SHOW TABLE STATUS FROM` B | 1044, denied |
| `PROCESSLIST`, `CLUSTER_PROCESSLIST`, `TIDB_TRX`, `USER_PRIVILEGES` | Own user only |

**Per-tenant storage visibility.** The probe used two schemas: 20 rows (about 4 KB of payload) and 20,000 rows (about 4 MB).

| Source | What it gives | Per tenant? |
|---|---|---|
| `information_schema.TABLES.TABLE_ROWS` | Correct row counts (20 and 20,000), about **60–90 s** after the writes | **Yes** |
| `information_schema.TABLES.DATA_LENGTH`, `INDEX_LENGTH` | `TABLE_ROWS × 16` and `× 8` bytes (320 and 320,000 for 200-byte rows). This is a row-count estimate, not bytes, and `ANALYZE TABLE` doesn't change it (`tidb_enable_auto_analyze` is read-only on Starter) | Counts only; **bytes are wrong** |
| `information_schema.TABLE_STORAGE_STATS` | Needs `WHERE table_schema = …`, one query per schema (about 213 ms each; a 500-tenant sweep with 8 connections took 14 s). It reports **the region's** size and keys, and every tenant in a region shows the same figure (both probe schemas showed `4 MiB, 110,925 keys, 1 region`) | **No** |
| The TiDB Cloud API (`v1beta1/clusters/{id}?view=FULL`) | `usage` is `null` | No |
| The billing API (`billing.tidbapi.com/v1beta1/bills/{month}`) | Organization totals by project and service. It worked with this key and showed $0 | No: per cluster at best |
| The console | Per-instance storage and RUs | No: per instance |

So §14.6's "per-database storage from `information_schema.TABLES`" holds only for row counts. The control plane has to estimate bytes itself, for example `TABLE_ROWS` times a sampled average row size, and lean on the cell's hard limits.

**Per-tenant activity and RU attribution:**

| Source | As the `ti` admin user (`role_admin`) |
|---|---|
| `PROCESSLIST`, `CLUSTER_PROCESSLIST` | **Works:** every tenant's sessions, with user and database |
| `CLIENT_ERRORS_SUMMARY_BY_USER` | **Works:** error counts per user |
| `SLOW_QUERY`, `CLUSTER_SLOW_QUERY`, `STATEMENTS_SUMMARY`, `CLUSTER_STATEMENTS_SUMMARY`, `RESOURCE_GROUPS` | Denied (1142). The docs say they're unsupported on Starter and point to the console |
| `STATEMENTS_SUMMARY_HISTORY`, `performance_schema` digests | Readable, but with no per-user RU |
| `CREATE RESOURCE GROUP` | 20001, not supported |
| `@@tidb_last_query_info` | **Works in the tenant's own session:** `ru_consumption` for the last statement (0.5–5.8 RU per statement here) |
| `EXPLAIN ANALYZE` | Shows `RU:` per operator |

- **No per-user or per-database RU figure exists server-side on Starter.**
- The only RU attribution is client-side: `tidb_last_query_info` read after each statement. That works when the control plane runs the SQL itself, through its HTTP driver or a proxy. It doesn't work for a device's own MySQL connection, which is untrusted (§14.6), since the device would be reporting its own usage.
- Per-tenant fairness is therefore **not observable** from the server, except as connection counts.

**Connection caps:**
- `CREATE USER … WITH MAX_USER_CONNECTIONS n` and `ALTER USER … WITH MAX_USER_CONNECTIONS n` are **accepted and ignored**.
  - `SHOW CREATE USER` doesn't show the limit, and `mysql.user` has no such column.
  - A user limited to 1 opened 4 sessions; a user limited to 2 opened 3.
  - `@@max_user_connections` and `@@max_connections` read 0, and the variables are read-only.
- The watcher alternative works: as admin, count sessions per user in `CLUSTER_PROCESSLIST` and `KILL <id>` the extras. The kill closed the tenant's session.

**Credential rotation:**

| Operation | Result |
|---|---|
| `ALTER USER … IDENTIFIED BY` (new password) | Works. The old password was refused **1.1 s** later, and the new one works at once |
| `ALTER USER … PASSWORD EXPIRE` | Works. The next login is refused with **1862** ("Your password has expired"). The client must use the expired-password sandbox to reset it |
| `PASSWORD EXPIRE INTERVAL 30 DAY` | Accepted |
| `ALTER USER … ACCOUNT LOCK` | New connections are refused at once (**3118**). **Existing sessions keep working** until killed |
| `KILL <id>` from `CLUSTER_PROCESSLIST` | Ends the locked user's session. So the kill switch (§15.4) is `ACCOUNT LOCK` plus a kill sweep |

**The spending limit:** not driven to exhaustion.
- The instance's free share is 50M RUs a month. This spike's whole workload, about 1,500 tenant sessions of about 8 statements each, used on the order of 60,000 RUs (estimate, from the per-statement figures). Burning 50M RUs would mean hours of synthetic load on the shared free quota, so it wasn't done.
- What was confirmed: a free instance reports `spending_limit.monthly = 0`, and the limitations page states the behaviour at quota (new connections denied, existing ones throttled).
- To confirm it live: run the LC1 conformance nightly against a dedicated throwaway instance near the end of a month, or have the owner watch the console during LC2a Task 11's budget week.

## Recommendation for the report

- **Cell size:** keep **200** (D1717).
  - Isolation and DDL cost hold flat to 500.
  - The limits that bite are the instance's free share, not the tenant count: 5 GiB of row storage, 50M RUs a month, and 400 concurrent connections (5,000 with a spending limit). With 200 tenants, each gets about 25 MiB and 250k RUs a month, and 2 connections each at the 400 cap. That is below §15.3's defaults of 100 MiB, 1M RUs and 10 connections. So either 200 tenants with smaller quotas, or about **50** per cell at §15.3's quotas.
  - At $0 (D1756: 1 control DB and at most 4 free cells), that is **200 to 800 try-out tenants in total**.
- **Fairness:** not observable server-side. Use connection counting with kills, row-count growth from `TABLE_ROWS`, and the cell's hard cap. Per-tenant RU needs the SQL to go through the control plane.
- **Go** for cells, with the corrections above to §14.6, §15.1 and §15.3.
