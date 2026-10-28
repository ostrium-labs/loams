"""Loams House through `loams.house.v1`: what the previous release accepts. A
release whose House takes `INSERT` announces `house.insert` in
`GetInstance.features`; until then the fixture is DDL only (a lake table), and
the check reads its schema (§54 §14 step 2)."""

from lib import Skip

NAME = "house"
TABLE = "up1fx_t"
COLUMNS = [["id", "UInt64"], ["v", "String"]]


def statements(insert: bool) -> list[str]:
    """What the fixture runs: DDL, and rows only when the House takes `INSERT`."""
    ddl = [
        "CREATE DATABASE IF NOT EXISTS up1fx",
        f"CREATE TABLE IF NOT EXISTS up1fx.{TABLE} (id UInt64, v String) ENGINE = MergeTree ORDER BY id",
    ]
    return ddl + ([f"INSERT INTO up1fx.{TABLE} VALUES (1, 'ada'), (2, 'bob')"] if insert else [])


def run(stack, sql):
    # Every fixture statement writes (DDL, INSERT): `readOnly` absent means
    # true, which refuses them with `house_read_only`. A release from before
    # the field ignores it.
    _, end = stack.http().connect_stream(
        "loams.house.v1.HouseService/ExecuteQuery", {"sql": sql, "readOnly": False}
    )
    if end:
        raise AssertionError(f"House {sql!r}: {end}")


def describe(stack):
    status, reply = stack.http().connect(
        "loams.house.v1.HouseService/DescribeTable", {"database": "up1fx", "table": TABLE}
    )
    if status != 200:
        return None
    return [[c.get("name"), c.get("clickhouseType")] for c in reply.get("columns", [])]


def write(stack):
    if not stack.serves("loams.house.v1"):
        raise Skip("the engine does not serve loams.house.v1 (no House in this stack)")
    insert = stack.feature("house.insert")
    for sql in statements(insert):
        run(stack, sql)
    return {"columns": COLUMNS, "insert": insert}


def check(stack, rec):
    got = describe(stack)
    return [] if got == rec["columns"] else [f"House table up1fx.{TABLE}: {got}, created {rec['columns']}"]


def write_new(stack):
    run(stack, f"CREATE TABLE IF NOT EXISTS up1fx.{TABLE}_new (id UInt64) ENGINE = MergeTree ORDER BY id")
    return {"table": f"{TABLE}_new"}


def check_new(stack, rec, present):
    status, _ = stack.http().connect(
        "loams.house.v1.HouseService/DescribeTable", {"database": "up1fx", "table": rec["table"]}
    )
    there = status == 200
    return [] if there == present else [f"House table {rec['table']} is {'missing' if present else 'still there'}"]
