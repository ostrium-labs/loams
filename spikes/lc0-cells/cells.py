#!/usr/bin/env python3
"""LC0 Task 1: Starter tenant cells (spike, not product code).

Against one free TiDB Cloud Starter instance, this script creates N tenant
databases, each with its own SQL user granted only on its database, runs a
small mixed workload per tenant, and measures isolation, per-tenant storage
visibility, per-tenant activity, RU attribution, connection caps, credential
rotation and DDL cost. See README.md and the LC1 plan, LC0 Task 1.

Secrets: the admin credentials are read from the file `ti db
create-db-sql-users` writes (~/.ti/db_users/<cluster-id>/credentials).
Tenant passwords are generated in memory and never written or printed.
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import json
import os
import secrets
import ssl
import statistics
import string
import sys
import time
import tomllib
from dataclasses import dataclass, field

import pymysql

TENANT_DB = "lc0t_{:04d}"
TENANT_USER = "lc0t{:04d}"
RESULTS: dict = {"phases": [], "checks": {}}


def log(msg: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


def gen_password() -> str:
    # The Starter password policy can't be relaxed: mix every class.
    alphabet = string.ascii_letters + string.digits
    core = "".join(secrets.choice(alphabet) for _ in range(20))
    return "Aa1!" + core


@dataclass
class Cell:
    host: str
    port: int
    prefix: str
    admin_user: str
    admin_password: str
    ctx: ssl.SSLContext = field(default_factory=ssl.create_default_context)

    def connect(self, user: str, password: str, db: str | None = None, **kw):
        return pymysql.connect(
            host=self.host,
            port=self.port,
            user=user,
            password=password,
            database=db,
            ssl=self.ctx,
            connect_timeout=20,
            read_timeout=60,
            write_timeout=60,
            autocommit=True,
            **kw,
        )

    def admin(self):
        return self.connect(self.admin_user, self.admin_password)

    def tenant_user(self, i: int) -> str:
        return f"{self.prefix}.{TENANT_USER.format(i)}"


def load_cell(cluster_id: str, host: str, port: int, prefix: str) -> Cell:
    path = os.path.expanduser(f"~/.ti/db_users/{cluster_id}/credentials")
    with open(path, "rb") as fh:
        creds = tomllib.load(fh)
    adm = creds["admin"]
    return Cell(host, port, prefix, adm["username"], adm["password"])


def q(conn, sql: str, args=None):
    with conn.cursor() as cur:
        cur.execute(sql, args)
        try:
            return cur.fetchall()
        except pymysql.err.Error:
            return ()


def timed(fn, *a, **kw):
    t0 = time.perf_counter()
    out = fn(*a, **kw)
    return out, (time.perf_counter() - t0) * 1000.0


def pct(xs: list[float], p: float) -> float:
    if not xs:
        return float("nan")
    xs = sorted(xs)
    k = max(0, min(len(xs) - 1, round(p / 100.0 * (len(xs) - 1))))
    return round(xs[k], 1)


# ---------------------------------------------------------------- provisioning


def create_tenants(cell: Cell, start: int, end: int, pw: dict[int, str]) -> dict:
    """Create tenants start..end-1 serially on one admin connection, timing DDL."""
    lat = {"create_database": [], "create_user": [], "grant": []}
    conn = cell.admin()
    try:
        for i in range(start, end):
            db, user = TENANT_DB.format(i), cell.tenant_user(i)
            pw[i] = gen_password()
            _, ms = timed(q, conn, f"CREATE DATABASE `{db}`")
            lat["create_database"].append(ms)
            _, ms = timed(
                q,
                conn,
                "CREATE USER %s@'%%' IDENTIFIED BY %s WITH MAX_USER_CONNECTIONS 10",
                (user, pw[i]),
            )
            lat["create_user"].append(ms)
            _, ms = timed(q, conn, f"GRANT ALL PRIVILEGES ON `{db}`.* TO %s@'%%'", (user,))
            lat["grant"].append(ms)
    finally:
        conn.close()
    return {k: {"n": len(v), "p50_ms": pct(v, 50), "p95_ms": pct(v, 95), "max_ms": round(max(v), 1)} for k, v in lat.items()}


def tenant_workload(cell: Cell, i: int, password: str) -> dict:
    """A small mixed workload: DDL, 20 inserts, reads, an update, a delete."""
    db = TENANT_DB.format(i)
    t0 = time.perf_counter()
    conn = cell.connect(cell.tenant_user(i), password, db)
    connect_ms = (time.perf_counter() - t0) * 1000.0
    ru = None
    try:
        q(conn, "CREATE TABLE IF NOT EXISTS notes (id BIGINT AUTO_RANDOM PRIMARY KEY, body VARCHAR(255), n INT, KEY (n))")
        rows = [(f"note {k} " + "x" * 120, k) for k in range(20)]
        with conn.cursor() as cur:
            cur.executemany("INSERT INTO notes (body, n) VALUES (%s, %s)", rows)
        q(conn, "SELECT COUNT(*), SUM(n) FROM notes")
        q(conn, "SELECT * FROM notes WHERE n BETWEEN 3 AND 9 ORDER BY n")
        q(conn, "UPDATE notes SET body = CONCAT(body, '!') WHERE n < 5")
        q(conn, "DELETE FROM notes WHERE n = 19")
        info = q(conn, "SELECT @@tidb_last_query_info")
        if info and info[0][0]:
            ru = json.loads(info[0][0]).get("ru_consumption")
    finally:
        conn.close()
    return {"i": i, "connect_ms": connect_ms, "total_ms": (time.perf_counter() - t0) * 1000.0, "last_stmt_ru": ru}


def run_workloads(cell: Cell, ids: list[int], pw: dict[int, str], workers: int = 16) -> dict:
    out = []
    errors = []
    with cf.ThreadPoolExecutor(workers) as ex:
        futs = {ex.submit(tenant_workload, cell, i, pw[i]): i for i in ids}
        for f in cf.as_completed(futs):
            try:
                out.append(f.result())
            except Exception as e:  # noqa: BLE001 - record, don't stop the spike
                errors.append(f"t{futs[f]}: {type(e).__name__}: {e}")
    conn_ms = [r["connect_ms"] for r in out]
    tot_ms = [r["total_ms"] for r in out]
    rus = [r["last_stmt_ru"] for r in out if r["last_stmt_ru"] is not None]
    return {
        "tenants": len(out),
        "errors": errors[:5],
        "error_count": len(errors),
        "connect_p50_ms": pct(conn_ms, 50),
        "connect_p95_ms": pct(conn_ms, 95),
        "session_p50_ms": pct(tot_ms, 50),
        "session_p95_ms": pct(tot_ms, 95),
        "last_query_info_ru_seen": len(rus),
        "last_stmt_ru_sample": rus[:3],
    }


# ---------------------------------------------------------------- checks


def try_sql(conn, sql: str, args=None):
    try:
        return ("ok", q(conn, sql, args))
    except pymysql.err.MySQLError as e:
        return ("error", f"{e.args[0]}: {str(e.args[1])[:120]}" if len(e.args) > 1 else str(e))


def cell_grants_isolate_tenants(cell: Cell, pairs: list[tuple[int, int]], pw: dict[int, str]) -> dict:
    """Tenant A probes tenant B. Expected: every probe is denied or filtered."""
    leaks: list[str] = []
    observations: dict[str, str] = {}
    for a, b in pairs:
        dba, dbb = TENANT_DB.format(a), TENANT_DB.format(b)
        conn = cell.connect(cell.tenant_user(a), pw[a], dba)
        try:
            probes = {
                "select_other": (f"SELECT * FROM `{dbb}`.notes LIMIT 1", "must_error"),
                "insert_other": (f"INSERT INTO `{dbb}`.notes (body, n) VALUES ('evil', 1)", "must_error"),
                "use_other": (f"USE `{dbb}`", "must_error"),
                "create_database": ("CREATE DATABASE lc0x_evil", "must_error"),
                "mysql_user": ("SELECT user FROM mysql.user LIMIT 1", "must_error"),
                "mysql_stats_meta": ("SELECT * FROM mysql.stats_meta LIMIT 1", "must_error"),
                "show_grants_other": (f"SHOW GRANTS FOR '{cell.tenant_user(b)}'@'%'", "must_error"),
                "show_table_status_other": (f"SHOW TABLE STATUS FROM `{dbb}`", "must_error_or_empty"),
                "grant_self_other": (f"GRANT SELECT ON `{dbb}`.* TO '{cell.tenant_user(a)}'@'%'", "must_error"),
                "show_databases": ("SHOW DATABASES", "only_own"),
                "is_schemata": ("SELECT schema_name FROM information_schema.SCHEMATA", "only_own"),
                "is_tables": ("SELECT DISTINCT table_schema FROM information_schema.TABLES", "only_own"),
                "is_columns_other": (f"SELECT column_name FROM information_schema.COLUMNS WHERE table_schema = '{dbb}'", "must_error_or_empty"),
                "is_statistics_other": (f"SELECT index_name FROM information_schema.STATISTICS WHERE table_schema = '{dbb}'", "must_error_or_empty"),
                "is_table_storage_stats_other": (f"SELECT * FROM information_schema.TABLE_STORAGE_STATS WHERE table_schema = '{dbb}'", "must_error_or_empty"),
                "is_tikv_region_status_other": (f"SELECT * FROM information_schema.TIKV_REGION_STATUS WHERE db_name = '{dbb}' LIMIT 1", "must_error_or_empty"),
                "is_ddl_jobs_other": (f"SELECT * FROM information_schema.DDL_JOBS WHERE db_name = '{dbb}' LIMIT 1", "must_error_or_empty"),
                "admin_show_ddl_jobs": ("ADMIN SHOW DDL JOBS 50", "must_error_or_own"),
                "show_stats_meta": ("SHOW STATS_META", "must_error_or_own"),
                "processlist": ("SELECT user, db FROM information_schema.PROCESSLIST", "must_error_or_own_user"),
                "cluster_processlist": ("SELECT user, db FROM information_schema.CLUSTER_PROCESSLIST", "must_error_or_own_user"),
                "slow_query": ("SELECT user, db FROM information_schema.SLOW_QUERY LIMIT 50", "must_error_or_own_user"),
                "statements_summary": ("SELECT schema_name FROM information_schema.STATEMENTS_SUMMARY LIMIT 50", "must_error_or_own_db"),
                "tidb_trx": ("SELECT user, current_sql_digest_text FROM information_schema.TIDB_TRX", "must_error_or_own_user"),
                "user_privileges": ("SELECT grantee FROM information_schema.USER_PRIVILEGES", "must_error_or_own_user"),
            }
            for name, (sql, rule) in probes.items():
                status, res = try_sql(conn, sql)
                verdict = judge(rule, status, res, dba, dbb, cell.tenant_user(a))
                observations.setdefault(name, f"{status}: {verdict}" if status == "ok" else f"denied ({res})")
                if verdict.startswith("LEAK"):
                    leaks.append(f"{name} {dba}->{dbb}: {verdict}")
        finally:
            conn.close()
    return {"pairs": len(pairs), "passed": not leaks, "leaks": leaks[:20], "observations": observations}


def judge(rule, status, res, dba, dbb, own_user):
    if status == "error":
        return "denied"
    rows = res or ()
    flat = [str(c) for r in rows for c in r if c is not None]
    if rule == "must_error":
        return "LEAK: statement succeeded"
    if rule == "must_error_or_empty":
        return "filtered (no rows)" if not rows else f"LEAK: {len(rows)} rows"
    if rule == "only_own":
        tenants = {x for x in flat if x.startswith("lc0t_")}
        return "filtered (own only)" if tenants <= {dba} else f"LEAK: sees {sorted(tenants - {dba})[:3]}"
    if rule == "must_error_or_own":
        other = [x for x in flat if x.startswith("lc0t_") and x != dba]
        return f"filtered ({len(rows)} rows, none foreign)" if not other else f"LEAK: mentions {sorted(set(other))[:3]}"
    if rule == "must_error_or_own_user":
        users = {str(r[0]) for r in rows if r and r[0] is not None}
        # USER_PRIVILEGES grantee looks like 'user'@'%'
        foreign = {u for u in users if own_user not in u and "lc0t" in u}
        others = {u for u in users if own_user not in u}
        if foreign:
            return f"LEAK: other tenants {sorted(foreign)[:3]}"
        if others:
            return f"partial: sees non-tenant users {sorted(others)[:3]}"
        return f"filtered ({len(rows)} rows, own user only)"
    if rule == "must_error_or_own_db":
        other = [x for x in flat if x.startswith("lc0t_") and x != dba]
        return f"filtered ({len(rows)} rows)" if not other else f"LEAK: {sorted(set(other))[:3]}"
    return "?"


def cell_storage_is_visible_per_tenant(cell: Cell, n: int) -> dict:
    """Per-tenant storage, as the admin user sees it.

    information_schema.TABLES.DATA_LENGTH/INDEX_LENGTH come from statistics,
    and TABLE_STORAGE_STATS (region-level, approximate) needs a WHERE on
    TABLE_SCHEMA, so it is read one schema at a time.
    """
    conn = cell.admin()
    try:
        rows = q(
            conn,
            "SELECT table_schema, SUM(data_length), SUM(index_length), SUM(table_rows) "
            "FROM information_schema.TABLES WHERE table_schema LIKE 'lc0t\\_%%' GROUP BY table_schema",
        )
        tables = {r[0]: (int(r[1] or 0), int(r[2] or 0), int(r[3] or 0)) for r in rows}
        st, res = try_sql(
            conn,
            "SELECT table_schema, SUM(table_size) FROM information_schema.TABLE_STORAGE_STATS "
            "WHERE table_schema LIKE 'lc0t\\_%%' GROUP BY table_schema",
        )
        tss_bulk = res if st == "error" else f"ok, {len(res)} schemas"
    finally:
        conn.close()

    def one(i: int):
        c = cell.admin()
        try:
            db = TENANT_DB.format(i)
            r, ms = timed(
                q, c,
                "SELECT SUM(table_size), SUM(table_keys), SUM(region_count) FROM information_schema.TABLE_STORAGE_STATS WHERE table_schema = %s",
                (db,),
            )
            return db, (int(r[0][0] or 0), int(r[0][1] or 0), int(r[0][2] or 0)), ms
        finally:
            c.close()

    per, lat = {}, []
    with cf.ThreadPoolExecutor(8) as ex:
        for db, v, ms in ex.map(one, range(1, n + 1)):
            per[db] = v
            lat.append(ms)
    visible = sum(1 for v in per.values() if v[1] > 0)
    # TABLE_STORAGE_STATS is region-level: tenants that share a region all
    # report the region's size, so identical rows mean "not per tenant".
    distinct = len(set(per.values()))
    rows_visible = sum(1 for v in tables.values() if v[2] > 0)
    return {
        "tables_schemas_listed": len(tables),
        "tables_nonzero_data_length": sum(1 for v in tables.values() if v[0] + v[1] > 0),
        "tss_bulk_query": tss_bulk,
        "tss_schemas_with_keys": visible,
        "tss_sample_mib_keys_regions": {k: per[k] for k in list(per)[:3]},
        "tss_query_p50_ms": pct(lat, 50),
        "tss_sweep_s": round(sum(lat) / 1000.0 / 8, 1),
        "tss_distinct_values": distinct,
        "tables_rows_visible": rows_visible,
        # Per-tenant bytes must be visible and distinct per tenant. Row counts
        # alone (TABLE_ROWS) don't pass; see storage_probe.py.
        "passed": len(tables) == n and visible == n and distinct > 1,
    }


def activity_and_ru(cell: Cell, pw: dict[int, str]) -> dict:
    """What per-user activity and RU figures Starter exposes to the admin user."""
    probes = {
        "PROCESSLIST": "SELECT user, COUNT(*) FROM information_schema.PROCESSLIST GROUP BY user",
        "CLUSTER_PROCESSLIST": "SELECT user, COUNT(*) FROM information_schema.CLUSTER_PROCESSLIST GROUP BY user",
        "SLOW_QUERY": "SELECT user, COUNT(*) FROM information_schema.SLOW_QUERY GROUP BY user LIMIT 5",
        "CLUSTER_SLOW_QUERY": "SELECT user, COUNT(*) FROM information_schema.CLUSTER_SLOW_QUERY GROUP BY user LIMIT 5",
        "SLOW_QUERY_RU": "SELECT user, SUM(request_unit_read), SUM(request_unit_write) FROM information_schema.SLOW_QUERY GROUP BY user LIMIT 5",
        "STATEMENTS_SUMMARY": "SELECT schema_name, SUM(exec_count) FROM information_schema.STATEMENTS_SUMMARY GROUP BY schema_name LIMIT 5",
        "CLUSTER_STATEMENTS_SUMMARY": "SELECT schema_name, SUM(exec_count) FROM information_schema.CLUSTER_STATEMENTS_SUMMARY GROUP BY schema_name LIMIT 5",
        "STATEMENTS_SUMMARY_HISTORY": "SELECT COUNT(*) FROM information_schema.STATEMENTS_SUMMARY_HISTORY",
        "STATEMENTS_SUMMARY_RU": "SELECT schema_name, SUM(avg_request_unit_read*exec_count) FROM information_schema.STATEMENTS_SUMMARY GROUP BY schema_name LIMIT 5",
        "CLIENT_ERRORS_SUMMARY_BY_USER": "SELECT user, SUM(error_count) FROM information_schema.CLIENT_ERRORS_SUMMARY_BY_USER GROUP BY user LIMIT 5",
        "RESOURCE_GROUPS": "SELECT * FROM information_schema.RESOURCE_GROUPS",
        "CREATE_RESOURCE_GROUP": "CREATE RESOURCE GROUP IF NOT EXISTS lc0_rg RU_PER_SEC = 100",
        "tidb_enable_resource_control": "SELECT @@global.tidb_enable_resource_control",
        "USER_ATTRIBUTES": "SELECT user, attribute FROM information_schema.USER_ATTRIBUTES LIMIT 3",
        "TIDB_TRX": "SELECT COUNT(*) FROM information_schema.TIDB_TRX",
        "performance_schema_events": "SELECT COUNT(*) FROM performance_schema.events_statements_summary_by_digest",
    }
    out = {}
    conn = cell.admin()
    # Hold a few tenant sessions open so PROCESSLIST has something to show.
    held = [cell.connect(cell.tenant_user(i), pw[i], TENANT_DB.format(i)) for i in (1, 2, 3)]
    try:
        for k, sql in probes.items():
            st, res = try_sql(conn, sql)
            out[k] = res if st == "error" else {"rows": len(res), "sample": [tuple(str(c) for c in r) for r in res[:3]]}
        # Per-statement RU from the session itself (what a proxy or the
        # control plane could sum per tenant).
        t = held[0]
        q(t, "SELECT COUNT(*) FROM notes")
        info = q(t, "SELECT @@tidb_last_query_info")
        out["tidb_last_query_info"] = json.loads(info[0][0]) if info and info[0][0] else None
        st, res = try_sql(t, "EXPLAIN ANALYZE SELECT * FROM notes WHERE n > 2")
        out["explain_analyze_has_ru"] = (
            res if st == "error" else any("RU" in str(c) for r in res for c in r)
        )
        if st == "ok":
            out["explain_analyze_sample"] = [str(c)[:200] for r in res[:1] for c in r]
    finally:
        for h in held:
            h.close()
        conn.close()
    return out


def connection_cap(cell: Cell, pw: dict[int, str], i: int) -> dict:
    """MAX_USER_CONNECTIONS: set 2, open 3."""
    user = cell.tenant_user(i)
    adm = cell.admin()
    try:
        q(adm, "ALTER USER %s@'%%' WITH MAX_USER_CONNECTIONS 2", (user,))
        conns, third = [], None
        for _ in range(2):
            conns.append(cell.connect(user, pw[i], TENANT_DB.format(i)))
        try:
            c3 = cell.connect(user, pw[i], TENANT_DB.format(i))
            # Some gateways accept the TCP/TLS session and fail on first use.
            st, res = try_sql(c3, "SELECT 1")
            third = f"connected; first query {st} {res if st == 'error' else ''}".strip()
            c3.close()
        except pymysql.err.MySQLError as e:
            third = f"refused: {e.args[0]} {str(e.args[1])[:120] if len(e.args) > 1 else ''}"
        for c in conns:
            c.close()
        q(adm, "ALTER USER %s@'%%' WITH MAX_USER_CONNECTIONS 10", (user,))
    finally:
        adm.close()
    return {"limit": 2, "third_connection": third, "enforced": third.startswith("refused") or "error" in third}


def credential_rotation(cell: Cell, pw: dict[int, str], i: int) -> dict:
    user, db = cell.tenant_user(i), TENANT_DB.format(i)
    out = {}
    adm = cell.admin()
    try:
        old, new = pw[i], gen_password()
        q(adm, "ALTER USER %s@'%%' IDENTIFIED BY %s", (user, new))
        t0 = time.perf_counter()
        # Old password must fail, new must work. Poll for propagation.
        deadline, old_fails_after = t0 + 30, None
        while time.perf_counter() < deadline:
            try:
                cell.connect(user, old, db).close()
                time.sleep(0.5)
            except pymysql.err.MySQLError:
                old_fails_after = round((time.perf_counter() - t0) * 1000.0)
                break
        out["rotate_old_password_denied_after_ms"] = old_fails_after
        try:
            cell.connect(user, new, db).close()
            out["rotate_new_password_works"] = True
        except pymysql.err.MySQLError as e:
            out["rotate_new_password_works"] = f"no: {e}"
        pw[i] = new

        st, res = try_sql(adm, "ALTER USER %s@'%%' PASSWORD EXPIRE", (user,))
        out["password_expire_stmt"] = st if st == "ok" else res
        try:
            c = cell.connect(user, pw[i], db)
            st2, res2 = try_sql(c, "SELECT 1")
            out["after_expire_login"] = f"connected; query {st2} {res2 if st2 == 'error' else ''}".strip()
            st3, res3 = try_sql(c, "ALTER USER USER() IDENTIFIED BY %s", (gen_password(),))
            out["after_expire_self_reset"] = st3 if st3 == "ok" else res3
            c.close()
        except pymysql.err.MySQLError as e:
            out["after_expire_login"] = f"refused: {e.args[0]} {str(e.args[1])[:120] if len(e.args) > 1 else ''}"
        # Restore a known password for later steps.
        q(adm, "ALTER USER %s@'%%' IDENTIFIED BY %s PASSWORD EXPIRE NEVER", (user, pw[i]))

        st, res = try_sql(adm, "ALTER USER %s@'%%' PASSWORD EXPIRE INTERVAL 30 DAY", (user,))
        out["password_expire_interval_stmt"] = st if st == "ok" else res

        # Account lock: the kill switch in design 78 §15.4 locks SQL users.
        held = cell.connect(user, pw[i], db)
        t0 = time.perf_counter()
        q(adm, "ALTER USER %s@'%%' ACCOUNT LOCK", (user,))
        lock_ms = (time.perf_counter() - t0) * 1000.0
        try:
            cell.connect(user, pw[i], db).close()
            out["account_lock_new_connection"] = "STILL CONNECTS"
        except pymysql.err.MySQLError as e:
            out["account_lock_new_connection"] = f"refused: {e.args[0]}"
        st, res = try_sql(held, "SELECT COUNT(*) FROM notes")
        out["account_lock_existing_session"] = "still works" if st == "ok" else f"broken: {res}"
        out["account_lock_stmt_ms"] = round(lock_ms)
        # KILL the existing sessions, as the kill switch would.
        ids = q(adm, "SELECT id FROM information_schema.PROCESSLIST WHERE user = %s", (user,))
        killed = 0
        for (pid,) in ids:
            st, _ = try_sql(adm, f"KILL {int(pid)}")
            killed += st == "ok"
        out["kill_existing_sessions"] = f"{killed}/{len(ids)} killed via PROCESSLIST ids"
        st, res = try_sql(held, "SELECT 1")
        out["held_session_after_kill"] = "still works" if st == "ok" else "closed"
        try:
            held.close()
        except Exception:  # noqa: BLE001
            pass
        q(adm, "ALTER USER %s@'%%' ACCOUNT UNLOCK", (user,))
    finally:
        adm.close()
    return out


def rtt(cell: Cell) -> dict:
    conn = cell.admin()
    try:
        xs = [timed(q, conn, "SELECT 1")[1] for _ in range(10)]
        ver = q(conn, "SELECT VERSION()")[0][0]
        conns = q(conn, "SHOW VARIABLES LIKE 'max_connections'")
    finally:
        conn.close()
    return {"select1_p50_ms": pct(xs, 50), "version": ver, "max_connections": conns}


def teardown(cell: Cell, n: int) -> dict:
    conn = cell.admin()
    try:
        for i in range(1, n + 1):
            try_sql(conn, f"DROP DATABASE IF EXISTS `{TENANT_DB.format(i)}`")
            try_sql(conn, "DROP USER IF EXISTS %s@'%%'", (cell.tenant_user(i),))
        try_sql(conn, "DROP DATABASE IF EXISTS lc0x_evil")
        left_db = q(conn, "SELECT COUNT(*) FROM information_schema.SCHEMATA WHERE schema_name LIKE 'lc0t\\_%%' OR schema_name = 'lc0x_evil'")[0][0]
        st, res = try_sql(conn, "SELECT COUNT(*) FROM mysql.user WHERE user LIKE %s", (f"{cell.prefix}.lc0t%",))
    finally:
        conn.close()
    return {"databases_left": int(left_db), "users_left": int(res[0][0]) if st == "ok" else f"unreadable: {res}"}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cluster-id", required=True)
    ap.add_argument("--host", default="gateway01.us-east-1.prod.aws.tidbcloud.com")
    ap.add_argument("--port", type=int, default=4000)
    ap.add_argument("--prefix", required=True, help="the cluster's SQL user prefix (not secret)")
    ap.add_argument("--steps", default="50,200,500")
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--keep", action="store_true", help="skip SQL teardown")
    args = ap.parse_args()

    cell = load_cell(args.cluster_id, args.host, args.port, args.prefix)
    pw: dict[int, str] = {}
    RESULTS["rtt"] = rtt(cell)
    log(f"rtt {RESULTS['rtt']}")
    created = 0
    steps = [int(s) for s in args.steps.split(",")]
    try:
        for n in steps:
            log(f"phase N={n}: creating tenants {created + 1}..{n}")
            t0 = time.perf_counter()
            ddl = create_tenants(cell, created + 1, n + 1, pw)
            create_s = time.perf_counter() - t0
            created = n
            log(f"  ddl {ddl}")
            wl = run_workloads(cell, list(range(1, n + 1)), pw)
            log(f"  workload {wl}")
            pairs = [(1, n), (n, 1)] + [
                (secrets.randbelow(n) + 1, secrets.randbelow(n) + 1) for _ in range(8)
            ]
            pairs = [(a, b) for a, b in pairs if a != b]
            iso = cell_grants_isolate_tenants(cell, pairs, pw)
            log(f"  isolation passed={iso['passed']} leaks={iso['leaks'][:3]}")
            store = cell_storage_is_visible_per_tenant(cell, n)
            log(f"  storage {store['tables_schemas_listed']} schemas, passed={store['passed']}")
            RESULTS["phases"].append(
                {"n": n, "provision_s": round(create_s, 1), "ddl": ddl, "workload": wl, "isolation": iso, "storage": store}
            )
        RESULTS["checks"]["activity_and_ru"] = activity_and_ru(cell, pw)
        log("  activity/ru done")
        RESULTS["checks"]["connection_cap"] = connection_cap(cell, pw, 2)
        log(f"  conn cap {RESULTS['checks']['connection_cap']}")
        RESULTS["checks"]["credential_rotation"] = credential_rotation(cell, pw, 3)
        log(f"  rotation {RESULTS['checks']['credential_rotation']}")
    finally:
        if not args.keep:
            RESULTS["teardown_sql"] = teardown(cell, created)
            log(f"teardown {RESULTS['teardown_sql']}")
        with open(args.out, "w") as fh:
            json.dump(RESULTS, fh, indent=1, default=str)
    tests = {
        "cell_grants_isolate_tenants": all(p["isolation"]["passed"] for p in RESULTS["phases"]),
        "cell_storage_is_visible_per_tenant": all(p["storage"]["passed"] for p in RESULTS["phases"]),
    }
    if not args.keep:
        tests["teardown_leaves_nothing (SQL)"] = RESULTS["teardown_sql"]["databases_left"] == 0 and RESULTS["teardown_sql"]["users_left"] == 0
    print(json.dumps(tests))
    return 0 if all(tests.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
