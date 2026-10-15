"""Loams Postgres: a table with two rows on a compute's timeline. The gate
passes the compute's DSN (`--pg`); without one the stack runs no compute."""

from lib import Skip

NAME = "postgres"
ROWS = [(1, "ada"), (2, "bob")]


def connect(stack):
    if not stack.pg:
        raise Skip("no compute DSN (--pg): the stack runs no Postgres compute")
    try:
        import psycopg
    except ImportError as e:
        raise Skip(f"psycopg is not installed: {e}")
    try:
        return psycopg.connect(stack.pg, autocommit=True, connect_timeout=10)
    except psycopg.OperationalError as e:
        raise Skip(f"no compute at the DSN: {e}")


def rows(stack):
    with connect(stack) as conn:
        return [list(r) for r in conn.execute("SELECT id, v FROM up1fx ORDER BY id").fetchall()]


def write(stack):
    with connect(stack) as conn:
        conn.execute("CREATE TABLE IF NOT EXISTS up1fx (id int PRIMARY KEY, v text)")
        for row in ROWS:
            conn.execute("INSERT INTO up1fx VALUES (%s, %s) ON CONFLICT (id) DO UPDATE SET v = EXCLUDED.v", row)
    return {"rows": [list(r) for r in ROWS]}


def check(stack, rec):
    got = rows(stack)
    return [] if got[: len(rec["rows"])] == rec["rows"] else [f"up1fx: {got}, wrote {rec['rows']}"]


def write_new(stack):
    with connect(stack) as conn:
        conn.execute("INSERT INTO up1fx VALUES (100, 'new') ON CONFLICT (id) DO NOTHING")
    return {"row": [100, "new"]}


def check_new(stack, rec, present):
    there = rec["row"] in rows(stack)
    return [] if there == present else [f"row {rec['row']} is {'missing' if present else 'still there'}"]
