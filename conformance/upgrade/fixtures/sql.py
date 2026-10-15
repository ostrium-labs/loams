"""A TiDB database with a table and two rows (Loams SQL, over MySQL)."""

from lib import Skip

NAME = "sql"
ROWS = [(1, "ada"), (2, "bob")]


def connect(stack):
    try:
        import pymysql
    except ImportError as e:
        raise Skip(f"pymysql is not installed: {e}")
    host, _, port = stack.tidb.partition(":")
    try:
        return pymysql.connect(host=host, port=int(port or 4000), user="root", password="", autocommit=True,
                               connect_timeout=10)
    except pymysql.err.OperationalError as e:
        raise Skip(f"no TiDB at {stack.tidb}: {e}")


def rows(stack):
    with connect(stack) as conn, conn.cursor() as cur:
        cur.execute("SELECT id, v FROM up1fx.t ORDER BY id")
        return [list(r) for r in cur.fetchall()]


def write(stack):
    with connect(stack) as conn, conn.cursor() as cur:
        cur.execute("CREATE DATABASE IF NOT EXISTS up1fx")
        cur.execute("CREATE TABLE IF NOT EXISTS up1fx.t (id INT PRIMARY KEY, v VARCHAR(64))")
        cur.executemany("REPLACE INTO up1fx.t VALUES (%s, %s)", ROWS)
    return {"rows": [list(r) for r in ROWS]}


def check(stack, rec):
    got = rows(stack)
    return [] if got[: len(rec["rows"])] == rec["rows"] else [f"up1fx.t: {got}, wrote {rec['rows']}"]


def write_new(stack):
    with connect(stack) as conn, conn.cursor() as cur:
        cur.execute("REPLACE INTO up1fx.t VALUES (100, 'new')")
    return {"row": [100, "new"]}


def check_new(stack, rec, present):
    there = rec["row"] in rows(stack)
    return [] if there == present else [f"row {rec['row']} is {'missing' if present else 'still there'}"]
