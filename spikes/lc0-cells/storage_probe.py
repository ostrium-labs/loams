#!/usr/bin/env python3
"""LC0 Task 1 follow-up: are per-tenant storage figures per tenant?

Two schemas of very different size (20 rows vs 20,000 rows); read
information_schema.TABLES (stats-derived) and TABLE_STORAGE_STATS
(region-derived) for each, now and as statistics catch up.
Usage: storage_probe.py <cluster-id> <user-prefix>
"""
import sys, time, json
import cells

c = cells.load_cell(sys.argv[1], "gateway01.us-east-1.prod.aws.tidbcloud.com", 4000, sys.argv[2])
a = c.admin(); q = cells.q
sizes = {"lc0p_small": 20, "lc0p_big": 20000}
for db, n in sizes.items():
    q(a, f"DROP DATABASE IF EXISTS {db}")
    q(a, f"CREATE DATABASE {db}")
    q(a, f"CREATE TABLE {db}.t (id BIGINT AUTO_RANDOM PRIMARY KEY, body VARCHAR(255), n INT, KEY(n))")
    with a.cursor() as cur:
        for off in range(0, n, 1000):
            cur.executemany(f"INSERT INTO {db}.t (body,n) VALUES (%s,%s)", [("z" * 200, k) for k in range(off, min(n, off + 1000))])
expected = {db: n * 200 for db, n in sizes.items()}
t0 = time.time()
def read():
    out = {}
    for db in sizes:
        r = q(a, "SELECT data_length, index_length, table_rows FROM information_schema.TABLES WHERE table_schema=%s", (db,))[0]
        s = q(a, "SELECT table_size, table_keys, region_count FROM information_schema.TABLE_STORAGE_STATS WHERE table_schema=%s", (db,))[0]
        out[db] = {"tables(data,index,rows)": [int(x or 0) for x in r], "tss(MiB,keys,regions)": [int(x or 0) for x in s], "approx_payload_bytes": expected[db]}
    return out
res = []
for wait in (0, 30, 60, 120, 240):
    time.sleep(wait)
    res.append({"t_s": round(time.time() - t0), **read()})
    print(json.dumps(res[-1]), flush=True)
cells.try_sql(a, "ANALYZE TABLE lc0p_big.t"); time.sleep(20)
print(json.dumps({"after_analyze": read()}), flush=True)
for db in sizes:
    q(a, f"DROP DATABASE {db}")
q(a, "DROP DATABASE IF EXISTS lc0p_storage")
