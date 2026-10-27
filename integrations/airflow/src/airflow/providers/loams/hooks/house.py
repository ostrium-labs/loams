# SPDX-License-Identifier: Apache-2.0
"""`LoamsHouseHook`: ClickHouse SQL on Loams House.

The hook speaks `loams.house.v1.HouseService` (design 49 §18), which the
engine's main port proxies to the House front. It is the same surface the
console's House page uses, it runs every statement through the same checks as
the ClickHouse HTTP interface, and it authenticates with the Loams API key, so
one `loams` connection is enough. The SDK has no `house` module yet, so the
hook calls the RPCs over Connect's JSON encoding (`_connect.py`).

- `run()` streams `ExecuteQuery` with `RESULT_FORMAT_JSON_ROWS` and collects
  the header, the rows and the summary;
- `get_query()` reads one query's record (`GetQuery`), which is how a
  deferred task follows a statement it started;
- `cancel()` stops a running query (`CancelQuery`).

Every call has an async twin for triggers.
"""

from __future__ import annotations

import base64
import json
from collections.abc import Iterable, Mapping
from dataclasses import asdict, dataclass
from dataclasses import field as dc_field
from typing import Any

from airflow.providers.loams._connect import AsyncConnectClient, ConnectClient, LoamsApiError, field
from airflow.providers.loams.hooks.base import LoamsHook

__all__ = ["HOUSE_SERVICE", "HouseQueryStatus", "HouseResult", "LoamsHouseHook"]

HOUSE_SERVICE = "loams.house.v1.HouseService"


class HouseQueryStatus:
    """`loams.house.v1.QueryStatus`, as proto3 JSON spells it."""

    RUNNING = "QUERY_STATUS_RUNNING"
    FINISHED = "QUERY_STATUS_FINISHED"
    FAILED = "QUERY_STATUS_FAILED"
    CANCELLED = "QUERY_STATUS_CANCELLED"
    TERMINAL = frozenset({FINISHED, FAILED, CANCELLED})

    _BY_NUMBER = {1: RUNNING, 2: FINISHED, 3: FAILED, 4: CANCELLED}

    @classmethod
    def normalize(cls, value: Any) -> str:
        """Accepts the enum's name or number."""
        if isinstance(value, int):
            return cls._BY_NUMBER.get(value, "QUERY_STATUS_UNSPECIFIED")
        text = str(value or "QUERY_STATUS_UNSPECIFIED")
        if text.isdigit():
            return cls._BY_NUMBER.get(int(text), "QUERY_STATUS_UNSPECIFIED")
        return text


@dataclass
class HouseResult:
    """One statement's result."""

    query_id: str = ""
    columns: list[dict[str, str]] = dc_field(default_factory=list)
    rows: list[list[Any]] = dc_field(default_factory=list)
    summary: dict[str, Any] = dc_field(default_factory=dict)

    @property
    def column_names(self) -> list[str]:
        return [c["name"] for c in self.columns]

    def records(self) -> list[dict[str, Any]]:
        """One dict per row, keyed by column name."""
        names = self.column_names
        return [dict(zip(names, row, strict=False)) for row in self.rows]

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


_INT_SUMMARY_FIELDS = (
    "result_rows",
    "result_bytes",
    "read_rows",
    "read_bytes",
    "written_rows",
    "elapsed_ms",
)


def _summary(frame: Mapping[str, Any]) -> dict[str, Any]:
    out: dict[str, Any] = {name: int(field(frame, name, 0) or 0) for name in _INT_SUMMARY_FIELDS}
    out["truncated"] = bool(field(frame, "truncated", False))
    out["query_id"] = field(frame, "query_id", "") or ""
    snapshots = field(frame, "snapshot_ids") or {}
    out["snapshot_ids"] = {str(k): int(v) for k, v in snapshots.items()}
    return out


class HouseResultBuilder:
    """Folds `ExecuteQueryResponse` frames into a `HouseResult`."""

    def __init__(self) -> None:
        self.result = HouseResult()
        self.done = False

    def feed(self, frame: Mapping[str, Any]) -> None:
        header = field(frame, "header")
        if header is not None:
            self.result.query_id = field(header, "query_id", "") or self.result.query_id
            self.result.columns = [
                {"name": field(c, "name", ""), "type": field(c, "clickhouse_type", "")}
                for c in field(header, "columns", []) or []
            ]
            fmt = field(header, "format")
            if fmt not in (None, "", "RESULT_FORMAT_JSON_ROWS", 2):
                raise LoamsApiError(
                    f"the House answered format {fmt}, not the JSON rows asked for", code="internal"
                )
            return
        batch = field(frame, "batch")
        if batch is not None:
            rows_json = field(batch, "rows_json")
            if rows_json:
                self.result.rows.extend(parse_json_rows(base64.b64decode(rows_json)))
            return
        summary = field(frame, "summary")
        if summary is not None:
            self.result.summary = _summary(summary)
            self.result.query_id = self.result.summary["query_id"] or self.result.query_id
            self.done = True
        # `progress` frames carry nothing the result keeps.


def parse_json_rows(data: bytes) -> list[list[Any]]:
    """ClickHouse `JSONCompactEachRow`: one JSON array per line."""
    rows: list[list[Any]] = []
    for line in data.splitlines():
        line = line.strip()
        if line:
            rows.append(json.loads(line))
    return rows


def query_record(response: Mapping[str, Any]) -> dict[str, Any]:
    """`GetQueryResponse.query` as a plain dict with normalised types."""
    record = field(response, "query") or {}
    return {
        "query_id": field(record, "query_id", ""),
        "principal": field(record, "principal", ""),
        "started_at": field(record, "started_at"),
        "duration_ms": int(field(record, "duration_ms", 0) or 0),
        "status": HouseQueryStatus.normalize(field(record, "status")),
        "error_code": int(field(record, "error_code", 0) or 0),
        "error_message": field(record, "error_message", "") or "",
        "read_rows": int(field(record, "read_rows", 0) or 0),
        "read_bytes": int(field(record, "read_bytes", 0) or 0),
        "result_rows": int(field(record, "result_rows", 0) or 0),
    }


class LoamsHouseHook(LoamsHook):
    """ClickHouse SQL on Loams House.

    :param loams_conn_id: the `loams` connection.
    :param database: the default database for statements (`default` when
        empty); the connection's extra `house_database` by default.
    """

    hook_name = "Loams House"

    def __init__(
        self,
        loams_conn_id: str = LoamsHook.default_conn_name,
        *,
        database: str | None = None,
        **kwargs: Any,
    ) -> None:
        super().__init__(loams_conn_id=loams_conn_id, **kwargs)
        self.database = database

    # -- requests ------------------------------------------------------------

    def _client(self) -> ConnectClient:
        return self.connect_client(self.settings.house_endpoint)

    def _aclient(self) -> AsyncConnectClient:
        return self.async_connect_client(self.settings.house_endpoint)

    def build_request(
        self,
        sql: str,
        *,
        database: str | None = None,
        params: Mapping[str, Any] | None = None,
        settings: Mapping[str, Any] | None = None,
        read_only: bool = True,
        max_rows: int = 0,
        max_bytes: int = 0,
        timeout_ms: int = 0,
        query_id: str | None = None,
    ) -> dict[str, Any]:
        """The `ExecuteQueryRequest`, as proto3 JSON."""
        sql = sql.strip().rstrip(";").strip()
        if not sql:
            raise ValueError("an empty statement")
        request: dict[str, Any] = {
            "sql": sql,
            "database": database or self.database or self.settings.house_database or "",
            "format": "RESULT_FORMAT_JSON_ROWS",
            "readOnly": bool(read_only),
        }
        if params:
            request["params"] = {str(k): _param(v) for k, v in params.items()}
        if settings:
            request["settings"] = {str(k): _param(v) for k, v in settings.items()}
        if max_rows:
            request["maxRows"] = int(max_rows)
        if max_bytes:
            request["maxBytes"] = str(int(max_bytes))
        if timeout_ms:
            request["timeoutMs"] = int(timeout_ms)
        if query_id:
            request["queryId"] = query_id
        return request

    # -- blocking ------------------------------------------------------------

    def run(self, sql: str, **kwargs: Any) -> HouseResult:
        """Runs one statement and answers its result (see `build_request`)."""
        request = self.build_request(sql, **kwargs)
        builder = HouseResultBuilder()
        for frame in self._client().stream(HOUSE_SERVICE, "ExecuteQuery", request):
            builder.feed(frame)
        if not builder.done:
            raise LoamsApiError("the House stream ended without a summary", code="unavailable")
        self.log.info(
            "House query %s: %s rows read, %s rows returned%s",
            builder.result.query_id,
            builder.result.summary.get("read_rows"),
            builder.result.summary.get("result_rows"),
            " (truncated)" if builder.result.summary.get("truncated") else "",
        )
        return builder.result

    def get_records(self, sql: str, **kwargs: Any) -> list[list[Any]]:
        return self.run(sql, **kwargs).rows

    def get_query(self, query_id: str) -> dict[str, Any] | None:
        """The query's record, or None when the House has no such query."""
        try:
            response = self._client().call(HOUSE_SERVICE, "GetQuery", {"queryId": query_id})
        except LoamsApiError as exc:
            if exc.not_found:
                return None
            raise
        return query_record(response)

    def cancel(self, query_id: str) -> None:
        try:
            self._client().call(HOUSE_SERVICE, "CancelQuery", {"queryId": query_id})
        except LoamsApiError as exc:
            if not exc.not_found:
                raise

    def list_tables(self, database: str | None = None, *, filter: str = "") -> list[dict[str, Any]]:
        """The database's tables, from catalog metadata, every page."""
        tables: list[dict[str, Any]] = []
        token = ""
        while True:
            response = self._client().call(
                HOUSE_SERVICE,
                "ListTables",
                {
                    "database": database or self.database or "",
                    "filter": filter,
                    "pageSize": 1000,
                    "pageToken": token,
                },
            )
            tables.extend(field(response, "tables", []) or [])
            token = field(response, "next_page_token", "") or ""
            if not token:
                return tables

    def test_connection(self) -> tuple[bool, str]:
        try:
            result = self.run("SELECT 1")
            return True, f"House answered {result.rows}"
        except Exception as exc:
            return False, str(exc)

    # -- async (triggers) ----------------------------------------------------

    async def arun(self, sql: str, **kwargs: Any) -> HouseResult:
        request = self.build_request(sql, **kwargs)
        builder = HouseResultBuilder()
        async for frame in self._aclient().stream(HOUSE_SERVICE, "ExecuteQuery", request):
            builder.feed(frame)
        if not builder.done:
            raise LoamsApiError("the House stream ended without a summary", code="unavailable")
        return builder.result

    async def aget_query(self, query_id: str) -> dict[str, Any] | None:
        try:
            response = await self._aclient().call(HOUSE_SERVICE, "GetQuery", {"queryId": query_id})
        except LoamsApiError as exc:
            if exc.not_found:
                return None
            raise
        return query_record(response)

    async def acancel(self, query_id: str) -> None:
        try:
            await self._aclient().call(HOUSE_SERVICE, "CancelQuery", {"queryId": query_id})
        except LoamsApiError as exc:
            if not exc.not_found:
                raise


def _param(value: Any) -> str:
    """A query parameter or setting as the House takes it: text."""
    if isinstance(value, bool):
        return "1" if value else "0"
    if isinstance(value, (list, tuple, dict)):
        return json.dumps(value)
    return str(value)


def split_statements(sql: str | Iterable[str]) -> list[str]:
    """A list of statements stays a list; one string is one statement.

    The House takes one statement per call, and splitting SQL text on `;`
    is wrong inside string literals, so the operator asks for a list.
    """
    if isinstance(sql, str):
        return [sql]
    return [s for s in sql if s and s.strip()]
