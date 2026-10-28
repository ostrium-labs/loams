# SPDX-License-Identifier: Apache-2.0
"""`LoamsHouseQuerySensor`: waits for a House query, by id, to finish."""

from __future__ import annotations

from collections.abc import Sequence
from datetime import timedelta
from typing import TYPE_CHECKING, Any

from airflow.sdk import BaseSensorOperator

from airflow.providers.loams.hooks.house import HouseQueryStatus, LoamsHouseHook
from airflow.providers.loams.triggers.house import LoamsHouseQueryTrigger

if TYPE_CHECKING:
    from airflow.sdk import Context

__all__ = ["LoamsHouseQuerySensor"]


class LoamsHouseQuerySensor(BaseSensorOperator):
    """Succeeds when the query is finished; fails when it failed or was cancelled.

    :param query_id: the query's id (templated).
    :param deferrable: wait in the triggerer instead of a worker slot.
    """

    template_fields: Sequence[str] = ("query_id",)

    def __init__(
        self,
        *,
        query_id: str,
        loams_conn_id: str = LoamsHouseHook.default_conn_name,
        deferrable: bool = False,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self.query_id = query_id
        self.loams_conn_id = loams_conn_id
        self.deferrable = deferrable

    def poke(self, context: Context) -> bool:
        record = LoamsHouseHook(loams_conn_id=self.loams_conn_id).get_query(self.query_id)
        if record is None:
            self.log.info("The House has no query %s yet", self.query_id)
            return False
        status = record["status"]
        if status == HouseQueryStatus.FINISHED:
            return True
        if status in (HouseQueryStatus.FAILED, HouseQueryStatus.CANCELLED):
            raise RuntimeError(
                f"House query {self.query_id} ended {status}: {record.get('error_message') or 'no message'}"
            )
        return False

    def execute(self, context: Context) -> Any:
        if not self.deferrable:
            return super().execute(context)
        if self.poke(context):
            return None
        self.defer(
            trigger=LoamsHouseQueryTrigger(
                query_id=self.query_id,
                loams_conn_id=self.loams_conn_id,
                poll_interval=float(self.poke_interval),
                not_found_timeout=None,
            ),
            method_name="execute_complete",
            timeout=timedelta(seconds=float(self.timeout)) if self.timeout else None,
        )
        return None

    def execute_complete(self, context: Context, event: dict[str, Any]) -> dict[str, Any] | None:
        if event.get("status") != "success":
            raise RuntimeError(f"House query {self.query_id}: {event.get('message')}")
        return event.get("record")
