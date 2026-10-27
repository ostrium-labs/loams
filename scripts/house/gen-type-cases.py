#!/usr/bin/env python3
"""Generate the HE1 Task 24 arithmetic/type matrix; no external dependencies."""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[2]
TYPES = ["UInt8", "UInt16", "UInt32", "UInt64", "Int8", "Int16", "Int32", "Int64", "Float32", "Float64"]
OPS = {"plus": "+", "minus": "-", "multiply": "*", "divide": "/", "modulo": "%", "intDiv": None}
def generate():
    folder = ROOT / "conformance/clickhouse/oracle/corpus/he1/24"
    folder.mkdir(parents=True, exist_ok=True)
    for left in TYPES:
        for right in TYPES:
            a, b = left.lower(), right.lower()
            for name, op in OPS.items():
                expression = f"{a} {op} {b}" if op else f"intDiv({a}, {b})"
                sql = f"SELECT {expression} AS value, toTypeName({expression}) AS type FROM he24.numeric ORDER BY row_id\n"
                (folder / f"{name}-{a}-{b}.sql").write_text(sql)
            expressions = [f"{a} {op} {b}" for op in ("=", "!=", "<", "<=", ">", ">=")]
            sql = "SELECT " + ", ".join(f"{e} AS c{i}, toTypeName({e}) AS t{i}" for i,e in enumerate(expressions)) + " FROM he24.numeric ORDER BY row_id\n"
            (folder / f"compare-{a}-{b}.sql").write_text(sql)
    print("Generated 600 arithmetic and 100 comparison cases")
if __name__ == "__main__":
    generate()
