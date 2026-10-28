#!/usr/bin/env python3
"""Captures the aggregate fixtures of loams-house-exec (plan HE1 Task 28) from
the pinned oracle, clickhouse-server 26.9.2.8 (R0.3):

  agg_types.tsv  <planned name> TAB <argument types, `|`-separated> TAB
                 <result type on one line> | ERROR <code>
  funnel.tsv     <window> TAB <modes, `,`-separated> TAB <events as JSON:
                 [[ts, c1, c2, c3], ...]> TAB <windowFunnel's answer>

  python3 -I scripts/house/capture-agg-fixtures.py

The server is the oracle compose's (`scripts/house/oracle.sh up`); set
LOAMS_ORACLE_URL and LOAMS_ORACLE_PASSWORD to use another. The event streams
are generated from a fixed seed, so a re-run writes the same files.
"""

import base64
import json
import os
import random
import sys
import urllib.error
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
FIXTURES = os.path.join(ROOT, "fabric/crates/loams-house-exec/tests/fixtures")
URL = os.environ.get("LOAMS_ORACLE_URL", "http://127.0.0.1:28123/")
AUTH = base64.b64encode(
    ("oracle:" + os.environ.get("LOAMS_ORACLE_PASSWORD", "oracle")).encode()
).decode()


def q(sql):
    req = urllib.request.Request(URL, data=sql.encode(), headers={"Authorization": "Basic " + AUTH})
    try:
        with urllib.request.urlopen(req) as r:
            return True, r.read().decode()
    except urllib.error.HTTPError as e:
        return False, e.read().decode()


def quote(s):
    return "'" + s.replace("\\", "\\\\").replace("'", "\\'") + "'"


TYPES = [
    "UInt8", "UInt16", "UInt32", "UInt64", "Int8", "Int16", "Int32", "Int64",
    "Float32", "Float64", "Decimal(9, 2)", "Decimal(18, 4)", "Decimal(38, 3)",
    "Decimal(76, 5)", "Bool", "String", "Date", "Date32", "DateTime",
    "DateTime64(3)", "Enum8('a' = 1, 'b' = 2)", "LowCardinality(String)",
    "Nullable(UInt8)", "Nullable(Int64)", "Nullable(Float64)", "Nullable(String)",
    "LowCardinality(Nullable(String))", "Array(UInt8)", "Array(Nullable(Int32))",
    "Array(String)", "Map(String, UInt64)", "Map(LowCardinality(String), Int32)",
    "Tuple(UInt8, String)",
]

UNARY = [
    "count", "sum", "avg", "min", "max", "any", "anyLast", "groupArray",
    "groupUniqArray", "topK", "approx_top_k", "sumOrNull", "sumOrDefault",
    "avgOrNull", "avgOrDefault", "maxOrNull", "countOrNull", "groupArrayOrNull",
    "sumDistinct", "countDistinct", "sumSimpleState", "maxSimpleState",
    "anySimpleState", "sumArray", "maxArray", "countArray", "groupArrayArray",
    "sumForEach", "countForEach", "groupArrayForEach", "sumMap", "maxMap",
    "minMap", "countMap", "avgMap", "groupArray(2)", "topK(3)",
    "approx_top_k(2)", "topK(3, 3, 'counts')", "groupUniqArray(1)",
    "array_agg", "last_value", "first_value", "any_value",
]

BINARY = [
    ("argMax", ["{t}", "UInt8"]), ("argMin", ["UInt8", "{t}"]),
    ("argMax", ["{t}", "Nullable(UInt8)"]), ("sumIf", ["{t}", "UInt8"]),
    ("sumIf", ["{t}", "Nullable(UInt8)"]), ("groupArrayIf", ["{t}", "Bool"]),
    ("avgIfOrNull", ["{t}", "UInt8"]), ("maxOrNullIf", ["{t}", "UInt8"]),
    ("countIf", ["UInt8"]), ("countIf", ["Nullable(UInt8)"]),
    ("windowFunnel(10)", ["{t}", "UInt8", "UInt8"]),
    ("windowFunnel(10)", ["UInt32", "{t}"]),
    ("sumMap", ["Array(String)", "Array({t})"]),
    ("maxMap", ["Array(UInt8)", "Array({t})"]),
    ("sumMap", ["Array({t})", "Array(UInt8)", "Array(Float32)"]),
    ("sumMap", ["Tuple(Array(String), Array({t}))"]),
    ("sumArrayIf", ["Array({t})", "UInt8"]),
]


def name_of(call):
    """`topK(3)` -> (`topK`, `(3)`)."""
    if "(" in call:
        i = call.index("(")
        return call[:i], call[i:]
    return call, ""


def type_case(call, args):
    fn, params = name_of(call)
    cols = ", ".join(
        "defaultValueOfTypeName(%s) AS a%d" % (quote(t), i) for i, t in enumerate(args)
    )
    argv = ", ".join("a%d" % i for i in range(len(args)))
    sql = "SELECT toTypeName(%s%s(%s)) FROM (SELECT %s) FORMAT TSVRaw" % (fn, params, argv, cols)
    ok, out = q(sql)
    if not ok:
        code = out.split("Code: ", 1)[1].split(".", 1)[0] if "Code: " in out else "?"
        return "ERROR " + code
    # One line: named tuples print over several lines in toTypeName.
    return " ".join(part.strip() for part in out.strip().splitlines()).replace("( ", "(")


def planned(call):
    """The engine's planned name: parameters as `name(p, …)`."""
    return call


def capture_types():
    rows = []
    for call in UNARY:
        for t in TYPES:
            rows.append((planned(call), [t]))
    for call, args in BINARY:
        for t in TYPES:
            rows.append((planned(call), [a.replace("{t}", t) for a in args]))
    seen = set()
    with open(os.path.join(FIXTURES, "agg_types.tsv"), "w") as f:
        for call, args in rows:
            key = (call, tuple(args))
            if key in seen:
                continue
            seen.add(key)
            f.write("%s\t%s\t%s\n" % (call, "|".join(args), type_case(call, args)))


MODES = [
    [], ["strict_order"], ["strict_deduplication"], ["strict_increase"],
    ["strict_once"], ["strict_order", "allow_reentry"],
    ["strict_deduplication", "strict_increase"], ["strict_once", "strict_order"],
    ["strict_once", "strict_deduplication"], ["strict_once", "strict_increase"],
]


def capture_funnel():
    rnd = random.Random(28)
    with open(os.path.join(FIXTURES, "funnel.tsv"), "w") as f:
        for stream in range(60):
            n = rnd.randint(0, 14)
            events = []
            for _ in range(n):
                ts = rnd.randint(0, 40)
                conds = [1 if rnd.random() < 0.35 else 0 for _ in range(3)]
                events.append([ts] + conds)
            window = rnd.choice([0, 3, 8, 20, 100])
            for modes in MODES:
                params = ", ".join([str(window)] + [quote(m) for m in modes])
                if events:
                    values = ", ".join("(%d, %d, %d, %d)" % tuple(e) for e in events)
                    src = "values('ts UInt32, c1 UInt8, c2 UInt8, c3 UInt8', %s)" % values
                else:
                    src = "(SELECT toUInt32(0) AS ts, toUInt8(0) AS c1, toUInt8(0) AS c2, toUInt8(0) AS c3 WHERE 0)"
                ok, out = q(
                    "SELECT windowFunnel(%s)(ts, c1, c2, c3) FROM %s "
                    "SETTINGS max_threads = 1 FORMAT TSVRaw" % (params, src)
                )
                if not ok:
                    sys.exit("windowFunnel failed: " + out)
                f.write("%d\t%s\t%s\t%s\n" % (window, ",".join(modes), json.dumps(events), out.strip()))


def main():
    ok, version = q("SELECT version() FORMAT TSVRaw")
    if not ok or version.strip() != "26.9.2.8":
        sys.exit("the oracle is %r, not 26.9.2.8" % version.strip())
    os.makedirs(FIXTURES, exist_ok=True)
    capture_types()
    capture_funnel()


if __name__ == "__main__":
    main()
