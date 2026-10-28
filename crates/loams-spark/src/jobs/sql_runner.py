# The Loams Spark SQL runner (design 81 SP-b, D2001): runs each statement of a
# SQL file with spark.sql, in order, and stops at the first failure.
#
# Used by JVM Spark (`spark-submit runner.py <file>`): the Kubernetes backend
# mounts it from a ConfigMap and the local runner writes it to its data
# directory. Connect-mode jobs use `python -m loams.spark.driver --sql`, which
# splits statements the same way.
#
# The file is read through Spark (`spark.read.text(..., wholetext=True)`), so an
# s3a:// file needs no extra client in the image.
import sys

from pyspark.sql import SparkSession


def split_sql(text):
    """Splits on `;` outside quotes, backticks and `--` comments."""
    out, buf = [], []
    quote = None
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if quote:
            buf.append(c)
            if c == quote:
                quote = None
        elif c in ("'", '"', "`"):
            quote = c
            buf.append(c)
        elif c == "-" and text.startswith("--", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
            continue
        elif c == ";":
            stmt = "".join(buf).strip()
            if stmt:
                out.append(stmt)
            buf = []
        else:
            buf.append(c)
        i += 1
    stmt = "".join(buf).strip()
    if stmt:
        out.append(stmt)
    return out


def main(argv):
    if len(argv) < 2:
        print("usage: runner.py <sql file>", file=sys.stderr)
        return 2
    path = argv[1]
    spark = SparkSession.builder.getOrCreate()
    text = "\n".join(r.value for r in spark.read.text(path, wholetext=True).collect())
    statements = split_sql(text)
    for n, stmt in enumerate(statements, 1):
        head = " ".join(stmt.split())[:80]
        print(f"[loams-spark] statement {n}/{len(statements)}: {head}", flush=True)
        df = spark.sql(stmt)
        # Show the result of a query; DDL and DML return an empty frame.
        if df.columns:
            df.show(20, truncate=False)
    print(f"[loams-spark] {len(statements)} statement(s) done", flush=True)
    spark.stop()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
