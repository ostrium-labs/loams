#!/usr/bin/env python3
"""Merge the settings an application sent over HTTP into its query_log dump.

HE0 (plan HE1, ruling R1.1 in he1-rulings/t1.md): ClickHouse's
`system.query_log.Settings` holds only the settings whose value differs from
the server's default, and asynchronous inserts are not initial `query_log`
rows at all (HE1 R4.5). So a client that sends `max_http_get_redirects=0` (the
default) on every insert leaves no trace, and the House refuses the setting
the mining never saw. The harness puts a logging reverse proxy (Caddy, JSON
access log) in front of the application's clickhouse-server; this script reads
that log and, by `query_id`:

* adds to each `query_log.tsv` row the settings its request carried in the URL
  that `Settings` lacks (their values as sent);
* appends a row for each asynchronous insert in `async_insert_log.tsv` that has
  no `query_log` row (type `QueryFinish`, initial), with its request's settings.

The R2.5 dump contract is unchanged: the result is still `TSVWithNames` with
`query_id`, `query`, `type`, `is_initial_query` and `Settings`. URL parameters
that are not settings (the query, the database, credentials, query
parameters, the HTTP interface's own options) are left out. Stdlib only.

    http_settings.py <query_log.tsv> <async_insert_log.tsv> <access.log>
"""

from __future__ import annotations

import json
import sys
import urllib.parse

# URL parameters ClickHouse's HTTP interface takes that are not settings.
NOT_SETTINGS = {
    "query", "database", "default_format", "user", "password", "quota_key",
    "query_id", "stacktrace", "role", "buffer_size", "wait_end_of_query",
    "session_id", "session_timeout", "session_check", "close_session",
    "compress", "decompress", "client_protocol_version", "http_response_headers",
}


def unescape(field: str) -> str:
    out, it = [], iter(field)
    for c in it:
        if c != "\\":
            out.append(c)
            continue
        e = next(it, "\\")
        out.append({"t": "\t", "n": "\n", "r": "\r", "b": "\b", "f": "\f", "0": "\0"}.get(e, e))
    return "".join(out)


def escape(value: str) -> str:
    return (value.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n")
            .replace("\r", "\\r"))


def map_items(text: str) -> list[tuple[str, str]]:
    """`{'k':'v',...}` as pairs."""
    items, cur, strings, inside, esc = [], [], [], False, False
    for c in text.strip()[1:-1] if text.strip().startswith("{") else "":
        if inside:
            if esc:
                cur.append(c)
                esc = False
            elif c == "\\":
                esc = True
            elif c == "'":
                strings.append("".join(cur))
                cur, inside = [], False
            else:
                cur.append(c)
        elif c == "'":
            inside = True
    for i in range(0, len(strings) - 1, 2):
        items.append((strings[i], strings[i + 1]))
    return items


def map_text(items: list[tuple[str, str]]) -> str:
    q = lambda s: s.replace("\\", "\\\\").replace("'", "\\'")
    return "{" + ",".join(f"'{q(k)}':'{q(v)}'" for k, v in items) + "}"


def requests(log_path: str) -> dict[str, dict[str, str]]:
    """query_id -> the settings its request's URL carried."""
    out: dict[str, dict[str, str]] = {}
    with open(log_path, encoding="utf-8", errors="replace") as f:
        for line in f:
            try:
                entry = json.loads(line)
            except ValueError:
                continue
            uri = (entry.get("request") or {}).get("uri", "")
            params = urllib.parse.parse_qsl(urllib.parse.urlsplit(uri).query, keep_blank_values=True)
            query_id = next((v for k, v in params if k == "query_id"), None)
            if not query_id:
                continue
            settings = {k: v for k, v in params
                        if k not in NOT_SETTINGS and not k.startswith("param_")}
            out.setdefault(query_id, {}).update(settings)
    return out


def read_tsv(path: str) -> tuple[list[str], list[list[str]]]:
    with open(path, encoding="utf-8") as f:
        lines = [line.rstrip("\n") for line in f if line.strip()]
    if not lines:
        return [], []
    return lines[0].split("\t"), [line.split("\t") for line in lines[1:]]


def main() -> int:
    query_log, async_log, access_log = sys.argv[1:4]
    sent = requests(access_log)
    header, rows = read_tsv(query_log)
    col = {name: i for i, name in enumerate(header)}
    for need in ("query_id", "query", "Settings"):
        if need not in col:
            raise SystemExit(f"http_settings: {query_log} has no `{need}` column")
    added, seen = 0, set()
    for row in rows:
        query_id = unescape(row[col["query_id"]])
        seen.add(query_id)
        extra = sent.get(query_id)
        if not extra:
            continue
        items = map_items(unescape(row[col["Settings"]]))
        known = {k for k, _ in items}
        more = [(k, v) for k, v in sorted(extra.items()) if k not in known]
        if more:
            row[col["Settings"]] = escape(map_text(items + more))
            added += len(more)
    appended = 0
    a_header, a_rows = read_tsv(async_log) if async_log != "-" else ([], [])
    if a_rows:
        a_col = {name: i for i, name in enumerate(a_header)}
        for a in a_rows:
            query_id = unescape(a[a_col["query_id"]])
            if query_id in seen or a[a_col.get("status", 0)] not in ("Ok", "ok"):
                continue
            seen.add(query_id)
            row = [""] * len(header)
            row[col["query_id"]] = a[a_col["query_id"]]
            row[col["query"]] = a[a_col["query"]]
            if "type" in col:
                row[col["type"]] = "QueryFinish"
            if "is_initial_query" in col:
                row[col["is_initial_query"]] = "1"
            if "query_kind" in col:
                row[col["query_kind"]] = "Insert"
            row[col["Settings"]] = escape(map_text(sorted(sent.get(query_id, {}).items())))
            rows.append(row)
            appended += 1
    with open(query_log, "w", encoding="utf-8") as f:
        f.write("\t".join(header) + "\n")
        for row in rows:
            f.write("\t".join(row) + "\n")
    print(f"http_settings: {len(sent)} requests; {added} settings added to query_log rows; "
          f"{appended} asynchronous inserts appended")
    return 0


if __name__ == "__main__":
    sys.exit(main())
