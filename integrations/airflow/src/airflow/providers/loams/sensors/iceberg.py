# SPDX-License-Identifier: Apache-2.0
"""`LoamsIcebergSnapshotSensor`: waits for a table to exist or get a new snapshot."""

from __future__ import annotations

from collections.abc import Sequence
from typing import TYPE_CHECKING, Any

from airflow.sdk import BaseSensorOperator, PokeReturnValue

from airflow.providers.loams.hooks.iceberg import LoamsIcebergHook

if TYPE_CHECKING:
    from airflow.sdk import Context

__all__ = ["LoamsIcebergSnapshotSensor"]


class LoamsIcebergSnapshotSensor(BaseSensorOperator):
    """Succeeds when the table has a current snapshot other than `after_snapshot_id`.

    Without `after_snapshot_id` it succeeds as soon as the table exists and
    has any snapshot (data was committed). Pass the id an earlier run saw
    (templated, from XCom) to wait for the next commit.

    :return: (XCom) the current snapshot's id, time, operation and summary.
    """

    template_fields: Sequence[str] = ("table", "after_snapshot_id")

    def __init__(
        self,
        *,
        table: str,
        after_snapshot_id: int | str | None = None,
        loams_conn_id: str = LoamsIcebergHook.default_conn_name,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self.table = table
        self.after_snapshot_id = after_snapshot_id
        self.loams_conn_id = loams_conn_id

    def poke(self, context: Context) -> PokeReturnValue:
        hook = LoamsIcebergHook(loams_conn_id=self.loams_conn_id)
        if not hook.table_exists(self.table):
            self.log.info("Table %s does not exist yet", self.table)
            return PokeReturnValue(is_done=False)
        snapshot = hook.current_snapshot(self.table)
        if snapshot is None:
            self.log.info("Table %s has no snapshot yet", self.table)
            return PokeReturnValue(is_done=False)
        after = self.after_snapshot_id
        if after not in (None, "") and int(after) == snapshot["snapshot_id"]:
            self.log.info("Table %s is still at snapshot %s", self.table, after)
            return PokeReturnValue(is_done=False)
        return PokeReturnValue(is_done=True, xcom_value=snapshot)
