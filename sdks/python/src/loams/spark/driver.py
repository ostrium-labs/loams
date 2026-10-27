"""The Spark Connect job driver (design 81, SP-b D2001 path (a)). **Preview.**

::

    python -m loams.spark.driver --app s3://bucket/jobs/etl.py -- --day 2026-10-27
    python -m loams.spark.driver --app jobs/report.sql --sql

This is the entrypoint the Loams driver container runs for a `connect`-backend
job, and what the local runner starts for one (D2002). It opens a session with
`loams.spark.session()` (so the endpoint, token and catalog come from the
environment the runner sets), then either:

- runs a SQL file, statement by statement, printing one summary line each; or
- runs a PySpark script as `__main__`, with `sys.argv` set to the app and its
  arguments, exactly as `python script.py args...` would.

A local path is read from disk; any other URI (`s3://...`) is read through
Spark (`spark.read.text(uri, wholetext=True)`), so the driver needs no object
store credentials of its own: the session's vended credentials cover it.

Exit code 0 on success, 1 on an exception (the traceback goes to stderr), or
the code a script passes to `sys.exit`.
"""

from __future__ import annotations

import argparse
import builtins
import json
import os
import sys
import tempfile
import traceback
import urllib.parse
from collections.abc import Callable, Mapping, Sequence
from typing import Any, Final

__all__ = ["add_py_files", "env_conf", "load_source", "main", "run_script", "run_sql", "split_sql"]

#: Characters of a statement shown in its summary line.
_SUMMARY_CHARS: Final[int] = 80
#: Rows fetched from a query to report its size without collecting all of it.
_PEEK_ROWS: Final[int] = 20


def split_sql(text: str) -> list[str]:
    """Splits a SQL script into statements on `;`.

    A `;` inside a single- or double-quoted string, a backtick-quoted
    identifier, a `--` line comment or a `/* */` block comment does not split.
    Inside quoted strings a backslash escapes the next character (Spark SQL's
    default) and a doubled quote stays in the string. Line comments are
    dropped; block comments are kept (they may be hints). Statements are
    stripped and empty ones dropped.
    """
    statements: list[str] = []
    current: list[str] = []
    i = 0
    n = len(text)
    quote: str | None = None
    while i < n:
        ch = text[i]
        if quote is not None:
            current.append(ch)
            if ch == "\\" and quote != "`" and i + 1 < n:
                current.append(text[i + 1])
                i += 2
                continue
            if ch == quote:
                quote = None
            i += 1
            continue
        if ch in ("'", '"', "`"):
            quote = ch
            current.append(ch)
            i += 1
            continue
        if ch == "-" and text.startswith("--", i):
            end = text.find("\n", i)
            if end == -1:
                break
            # Keep the newline so the comment still separates tokens.
            current.append("\n")
            i = end + 1
            continue
        if ch == "/" and text.startswith("/*", i):
            end = text.find("*/", i + 2)
            stop = n if end == -1 else end + 2
            current.append(text[i:stop])
            i = stop
            continue
        if ch == ";":
            statements.append("".join(current))
            current = []
            i += 1
            continue
        current.append(ch)
        i += 1
    statements.append("".join(current))
    return [s.strip() for s in statements if s.strip()]


def _is_local(app: str) -> bool:
    scheme = urllib.parse.urlsplit(app).scheme
    # A one-letter scheme is a Windows drive letter, not a URI.
    return scheme in ("", "file") or len(scheme) == 1


def _local_path(app: str) -> str:
    parts = urllib.parse.urlsplit(app)
    if parts.scheme == "file":
        return urllib.parse.unquote(parts.path)
    return app


def load_source(app: str, spark: Any) -> str:
    """The text of `app`: from disk when local, else through Spark."""
    if _is_local(app):
        with open(_local_path(app), encoding="utf-8") as handle:
            return handle.read()
    rows = spark.read.text(app, wholetext=True).collect()
    return "".join(str(row[0]) for row in rows)


def add_py_files(spark: Any, files: Sequence[str]) -> list[str]:
    """Ships `--py-file`s to the Connect server (`spark.addArtifacts(..., pyfile=True)`).

    A local file is added as is. A remote one (`s3://...`, a `.py`, `.zip` or `.egg`) is
    read through Spark's `binaryFile` source into a temporary directory first, keeping its
    base name, because `addArtifacts` uploads local files only. Returns the paths added.
    """
    added: list[str] = []
    tmp: str | None = None
    for item in files:
        if _is_local(item):
            path = _local_path(item)
        else:
            if tmp is None:
                tmp = tempfile.mkdtemp(prefix="loams-spark-pyfiles-")
            name = os.path.basename(urllib.parse.urlparse(item).path) or "pyfile.zip"
            row = spark.read.format("binaryFile").load(item).select("content").first()
            if row is None:
                raise FileNotFoundError(item)
            path = os.path.join(tmp, name)
            with open(path, "wb") as handle:
                handle.write(bytes(row[0]))
        spark.addArtifacts(path, pyfile=True)
        added.append(path)
    return added


def env_conf(environ: Mapping[str, str] | None = None) -> dict[str, str]:
    """The job's Spark conf the runner passes in `LOAMS_SPARK_CONF` (a JSON object).

    The Kubernetes renderer and the local runner set it from the job spec's `conf`
    (crates/loams-spark `render::connect_driver_env`); absent means none.
    """
    raw = (os.environ if environ is None else environ).get("LOAMS_SPARK_CONF")
    if not raw:
        return {}
    parsed = json.loads(raw)
    if not isinstance(parsed, dict):
        raise ValueError("LOAMS_SPARK_CONF must be a JSON object")
    return {str(k): str(v) for k, v in parsed.items()}


def _summary(index: int, total: int, statement: str, outcome: str) -> str:
    line = " ".join(statement.split())
    if len(line) > _SUMMARY_CHARS:
        line = line[: _SUMMARY_CHARS - 3] + "..."
    return f"[{index}/{total}] {outcome}: {line}"


def run_sql(spark: Any, text: str, out: Callable[[str], None] = print) -> int:
    """Runs each statement of a SQL script; returns how many ran.

    A query's rows are fetched up to a small cap, so a `SELECT` in a batch file
    runs rather than being planned and dropped; DDL and DML run on `spark.sql`.
    """
    statements = split_sql(text)
    for index, statement in enumerate(statements, start=1):
        rows = spark.sql(statement).take(_PEEK_ROWS + 1)
        count = len(rows)
        shown = f"{_PEEK_ROWS}+" if count > _PEEK_ROWS else str(count)
        out(_summary(index, len(statements), statement, f"ok, {shown} row(s)"))
    return len(statements)


def run_script(app: str, source: str, args: Sequence[str]) -> None:
    """Executes a PySpark script as `__main__` with `sys.argv = [app, *args]`."""
    code = compile(source, app, "exec")
    saved = sys.argv
    sys.argv = [app, *args]
    try:
        exec(code, {"__name__": "__main__", "__file__": app, "__builtins__": builtins})
    finally:
        sys.argv = saved


def _parse(argv: Sequence[str]) -> tuple[argparse.Namespace, list[str]]:
    # Everything after the first `--` belongs to the app, untouched by argparse.
    items = list(argv)
    rest: list[str] = []
    if "--" in items:
        cut = items.index("--")
        items, rest = items[:cut], items[cut + 1 :]
    parser = argparse.ArgumentParser(
        prog="python -m loams.spark.driver",
        description="Run a PySpark script or a SQL file against Loams Spark Connect.",
    )
    parser.add_argument("--app", required=True, help="the script or SQL file: a path or a URI")
    parser.add_argument("--sql", action="store_true", help="the app is a SQL file")
    parser.add_argument(
        "--py-file",
        action="append",
        default=[],
        dest="py_files",
        help="an extra .py, .zip or .egg for the app (repeatable)",
    )
    parser.add_argument("args", nargs="*", help="arguments for the app (or put them after --)")
    ns = parser.parse_args(items)
    return ns, [*ns.args, *rest]


def _default_session() -> Any:
    from loams.spark import session

    return session(conf=env_conf()).getOrCreate()


def main(
    argv: Sequence[str] | None = None,
    *,
    session_factory: Callable[[], Any] | None = None,
) -> int:
    """Runs the driver; returns the process exit code."""
    ns, args = _parse(sys.argv[1:] if argv is None else argv)
    app: str = ns.app
    try:
        spark = (session_factory or _default_session)()
        source = load_source(app, spark)
        if ns.py_files:
            add_py_files(spark, ns.py_files)
        if ns.sql:
            run_sql(spark, source)
        else:
            run_script(app, source, args)
    except SystemExit as stop:
        if stop.code is None:
            return 0
        if isinstance(stop.code, int):
            return stop.code
        print(stop.code, file=sys.stderr)
        return 1
    except Exception:
        traceback.print_exc(file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
