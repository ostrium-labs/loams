# SPDX-License-Identifier: Apache-2.0
"""`LoamsHouseQueryOperator`: ClickHouse SQL on Loams House."""

from __future__ import annotations

import re
from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Any

from airflow.sdk import BaseOperator

from airflow.providers.loams.hooks.house import LoamsHouseHook, split_statements
from airflow.providers.loams.triggers.house import LoamsHouseQueryTrigger

if TYPE_CHECKING:
    from airflow.sdk import Context

__all__ = ["LoamsHouseQueryOperator", "task_query_id"]

_UNSAFE = re.compile(r"[^A-Za-z0-9_.:\-]")


def task_query_id(context: Context, index: int = 0) -> str:
    """A query id fixed by the task try: `airflow:<dag>:<task>:<run>:<map>:<try>[:<n>]`.

    The same try always names the same query, so a deferred task that comes
    back after a triggerer restart finds the query it started.
    """
    ti = context["ti"]
    parts = [
        "airflow",
        ti.dag_id,
        ti.task_id,
        ti.run_id,
        str(getattr(ti, "map_index", -1)),
        str(getattr(ti, "try_number", 1)),
    ]
    if index:
        parts.append(str(index))
    return _UNSAFE.sub("_", ":".join(parts))


class LoamsHouseQueryOperator(BaseOperator):
    """Runs ClickHouse SQL on Loams House and returns the result.

    :param sql: one statement, or a list run in order (the House takes one
        statement per call). Templated; `.sql` files work.
    :param database: the current database (`default` when empty).
    :param params_: query parameters for `{name:Type}` placeholders.
    :param settings: ClickHouse settings for the statement(s).
    :param read_only: refuse anything but reads (the default). Set False for
        `INSERT ... SELECT`, `CREATE TABLE`, etc.
    :param max_rows: rows to return (0 is the House's 1 000; at most 100 000).
    :param timeout_ms: the statement's limit (0 is the House's 300 s).
    :param query_id: the query id; by default one fixed by the task try.
    :param return_rows: return the rows (else only the summary). The return
        value goes to XCom, so keep results small.
    :param deferrable: run the (last) statement from the triggerer, freeing
        the worker slot while it runs.
    :param poll_interval: seconds between status checks when deferred.
    """

    template_fields: Sequence[str] = ("sql", "database", "params_", "settings", "query_id")
    template_ext: Sequence[str] = (".sql",)
    template_fields_renderers = {"sql": "sql", "params_": "json", "settings": "json"}
    ui_color = "#f6d365"

    def __init__(
        self,
        *,
        sql: str | Sequence[str],
        loams_conn_id: str = LoamsHouseHook.default_conn_name,
        database: str | None = None,
        params_: Mapping[str, Any] | None = None,
        settings: Mapping[str, Any] | None = None,
        read_only: bool = True,
        max_rows: int = 0,
        timeout_ms: int = 0,
        query_id: str | None = None,
        return_rows: bool = True,
        deferrable: bool = False,
        poll_interval: float = 5.0,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self.sql = sql
        self.loams_conn_id = loams_conn_id
        self.database = database
        self.params_ = dict(params_ or {})
        self.settings = dict(settings or {})
        self.read_only = read_only
        self.max_rows = max_rows
        self.timeout_ms = timeout_ms
        self.query_id = query_id
        self.return_rows = return_rows
        self.deferrable = deferrable
        self.poll_interval = poll_interval
        self._running_query_id: str | None = None

    @property
    def hook(self) -> LoamsHouseHook:
        return LoamsHouseHook(loams_conn_id=self.loams_conn_id, database=self.database)

    def _query_id(self, context: Context, index: int, last: bool) -> str:
        if self.query_id:
            return self.query_id if last else f"{self.query_id}:{index}"
        return task_query_id(context, index)

    def _output(self, result: dict[str, Any]) -> dict[str, Any]:
        if not self.return_rows:
            result = {k: v for k, v in result.items() if k != "rows"}
        return result

    def execute(self, context: Context) -> Any:
        statements = split_statements(self.sql)
        if not statements:
            raise ValueError("no SQL to run")
        hook = self.hook
        last_index = len(statements) - 1
        # All but the last statement run here, in order; with deferrable the
        # last one runs from the triggerer.
        upto = last_index if self.deferrable else last_index + 1
        result = None
        for index in range(upto):
            self._running_query_id = self._query_id(context, index, index == last_index)
            result = hook.run(
                statements[index],
                params=self.params_,
                settings=self.settings,
                read_only=self.read_only,
                max_rows=self.max_rows,
                timeout_ms=self.timeout_ms,
                query_id=self._running_query_id,
            )
            self._running_query_id = None
        if self.deferrable:
            query_id = self._query_id(context, last_index, True)
            self.defer(
                trigger=LoamsHouseQueryTrigger(
                    query_id=query_id,
                    loams_conn_id=self.loams_conn_id,
                    sql=statements[last_index],
                    database=self.database,
                    params=self.params_,
                    settings=self.settings,
                    read_only=self.read_only,
                    max_rows=self.max_rows,
                    timeout_ms=self.timeout_ms,
                    poll_interval=self.poll_interval,
                    return_rows=self.return_rows,
                ),
                method_name="execute_complete",
            )
        if result is None:  # unreachable: at least one statement ran
            raise RuntimeError("no statement ran")
        return self._output(result.to_dict())

    def execute_complete(self, context: Context, event: dict[str, Any]) -> Any:
        if event.get("status") != "success":
            raise RuntimeError(f"House query {event.get('query_id')} failed: {event.get('message')}")
        self.log.info("House query %s finished", event.get("query_id"))
        if "record" in event:
            # The query had already been started (the triggerer restarted):
            # its record is what is known, not its rows.
            return {"query_id": event["query_id"], "record": event["record"]}
        return self._output(
            {
                "query_id": event.get("query_id"),
                "columns": event.get("columns", []),
                "rows": event.get("rows", []),
                "summary": event.get("summary", {}),
            }
        )

    def on_kill(self) -> None:
        if self._running_query_id:
            self.log.info("Cancelling House query %s", self._running_query_id)
            self.hook.cancel(self._running_query_id)
