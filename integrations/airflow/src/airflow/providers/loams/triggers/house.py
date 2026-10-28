# SPDX-License-Identifier: Apache-2.0
"""`LoamsHouseQueryTrigger`: waits, in the triggerer, for a House query to finish.

Two uses:

- **Poll only** (`sql` unset): follow a query some other process started, by
  its id, through `GetQuery`, until it is finished, failed or cancelled.
  `LoamsHouseQuerySensor` defers to this.
- **Run and wait** (`sql` set): the deferrable `LoamsHouseQueryOperator`
  hands its statement to the trigger, which runs it from the triggerer
  (`ExecuteQuery` is a stream that lasts as long as the statement) under a
  query id fixed by the task try, so the worker slot is free meanwhile.
  When the trigger starts it first asks `GetQuery` for that id: a query
  already running (the triggerer restarted) is followed, not started again;
  one already finished answers at once. Only an id the House has never seen
  is started.

The event is `{"status": "success" | "error", "query_id", ...}`: a success
carries the summary (and rows, for run-and-wait, up to `max_rows`), an error
its message.
"""

from __future__ import annotations

import asyncio
import time
from collections.abc import AsyncIterator, Mapping
from typing import Any

from airflow.triggers.base import BaseTrigger, TriggerEvent

from airflow.providers.loams._connect import LoamsApiError
from airflow.providers.loams.hooks.house import HouseQueryStatus, LoamsHouseHook

__all__ = ["LoamsHouseQueryTrigger"]


class LoamsHouseQueryTrigger(BaseTrigger):
    """Waits for a House query to end, optionally running it first.

    :param query_id: the query's id.
    :param loams_conn_id: the `loams` connection.
    :param sql: the statement to run when the House has not seen `query_id`.
    :param database, params, settings, read_only, max_rows, timeout_ms: as
        `LoamsHouseHook.run` takes them.
    :param poll_interval: seconds between `GetQuery` calls.
    :param not_found_timeout: in poll-only mode, seconds to wait for an id
        the House does not know before failing (None waits for ever).
    :param return_rows: put the rows in the event (run-and-wait only).
    """

    def __init__(
        self,
        *,
        query_id: str,
        loams_conn_id: str = LoamsHouseHook.default_conn_name,
        sql: str | None = None,
        database: str | None = None,
        params: Mapping[str, Any] | None = None,
        settings: Mapping[str, Any] | None = None,
        read_only: bool = True,
        max_rows: int = 0,
        timeout_ms: int = 0,
        poll_interval: float = 5.0,
        not_found_timeout: float | None = 300.0,
        return_rows: bool = True,
    ) -> None:
        super().__init__()
        self.query_id = query_id
        self.loams_conn_id = loams_conn_id
        self.sql = sql
        self.database = database
        self.params = dict(params or {})
        self.settings = dict(settings or {})
        self.read_only = read_only
        self.max_rows = max_rows
        self.timeout_ms = timeout_ms
        self.poll_interval = poll_interval
        self.not_found_timeout = not_found_timeout
        self.return_rows = return_rows

    def serialize(self) -> tuple[str, dict[str, Any]]:
        return (
            f"{type(self).__module__}.{type(self).__qualname__}",
            {
                "query_id": self.query_id,
                "loams_conn_id": self.loams_conn_id,
                "sql": self.sql,
                "database": self.database,
                "params": self.params,
                "settings": self.settings,
                "read_only": self.read_only,
                "max_rows": self.max_rows,
                "timeout_ms": self.timeout_ms,
                "poll_interval": self.poll_interval,
                "not_found_timeout": self.not_found_timeout,
                "return_rows": self.return_rows,
            },
        )

    def _hook(self) -> LoamsHouseHook:
        return LoamsHouseHook(loams_conn_id=self.loams_conn_id, database=self.database)

    async def run(self) -> AsyncIterator[TriggerEvent]:
        hook = self._hook()
        try:
            record = await hook.aget_query(self.query_id)
            if record is None and self.sql:
                yield TriggerEvent(await self._run_statement(hook))
                return
            yield TriggerEvent(await self._follow(hook, record))
        except LoamsApiError as exc:
            yield TriggerEvent(
                {
                    "status": "error",
                    "query_id": self.query_id,
                    "message": str(exc),
                    "code": exc.code,
                    "reason": exc.reason,
                }
            )

    async def _run_statement(self, hook: LoamsHouseHook) -> dict[str, Any]:
        self.log.info("Running House query %s from the triggerer", self.query_id)
        result = await hook.arun(
            str(self.sql),
            database=self.database,
            params=self.params,
            settings=self.settings,
            read_only=self.read_only,
            max_rows=self.max_rows,
            timeout_ms=self.timeout_ms,
            query_id=self.query_id,
        )
        event: dict[str, Any] = {
            "status": "success",
            "query_id": result.query_id or self.query_id,
            "summary": result.summary,
            "columns": result.columns,
        }
        if self.return_rows:
            event["rows"] = result.rows
        return event

    async def _follow(self, hook: LoamsHouseHook, record: dict[str, Any] | None) -> dict[str, Any]:
        started = time.monotonic()
        while True:
            if record is not None:
                status = record["status"]
                if status == HouseQueryStatus.FINISHED:
                    return {"status": "success", "query_id": self.query_id, "record": record}
                if status in (HouseQueryStatus.FAILED, HouseQueryStatus.CANCELLED):
                    return {
                        "status": "error",
                        "query_id": self.query_id,
                        "record": record,
                        "message": record.get("error_message")
                        or f"query {self.query_id} ended {status.removeprefix('QUERY_STATUS_').lower()}",
                    }
                self.log.debug("House query %s is %s", self.query_id, status)
            elif self.not_found_timeout is not None and time.monotonic() - started > self.not_found_timeout:
                return {
                    "status": "error",
                    "query_id": self.query_id,
                    "message": f"the House has no query {self.query_id} after {self.not_found_timeout}s",
                }
            await asyncio.sleep(self.poll_interval)
            record = await hook.aget_query(self.query_id)

    async def on_kill(self) -> None:
        """The task was killed by a user: cancel the query in the House."""
        try:
            await self._hook().acancel(self.query_id)
        except Exception:  # best effort; the task is already ending
            self.log.exception("Cancelling House query %s failed", self.query_id)
