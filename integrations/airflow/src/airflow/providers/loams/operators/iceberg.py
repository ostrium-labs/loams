# SPDX-License-Identifier: Apache-2.0
"""Operators on Iceberg tables in the Loams catalog (Lakekeeper)."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Any

from airflow.sdk import BaseOperator

from airflow.providers.loams.hooks.iceberg import LoamsIcebergHook

if TYPE_CHECKING:
    from airflow.sdk import Context

__all__ = [
    "LoamsIcebergAppendOperator",
    "LoamsIcebergCreateNamespaceOperator",
    "LoamsIcebergScanOperator",
]


class LoamsIcebergCreateNamespaceOperator(BaseOperator):
    """Creates a catalog namespace (by default, does nothing if it exists)."""

    template_fields: Sequence[str] = ("namespace", "properties")

    def __init__(
        self,
        *,
        namespace: str,
        properties: Mapping[str, str] | None = None,
        exist_ok: bool = True,
        loams_conn_id: str = LoamsIcebergHook.default_conn_name,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self.namespace = namespace
        self.properties = dict(properties or {})
        self.exist_ok = exist_ok
        self.loams_conn_id = loams_conn_id

    def execute(self, context: Context) -> str:
        LoamsIcebergHook(loams_conn_id=self.loams_conn_id).create_namespace(
            self.namespace, self.properties, exist_ok=self.exist_ok
        )
        return self.namespace


class LoamsIcebergAppendOperator(BaseOperator):
    """Appends rows to a table as one commit, or overwrites with them.

    :param table: `<namespace>.<table>` in the catalog.
    :param rows: dicts keyed by column (templated, so an upstream task's
        XCom works: `rows="{{ ti.xcom_pull('extract') }}"` with
        `render_template_as_native_obj=True` on the DAG).
    :param overwrite: replace rows instead of appending.
    :param overwrite_filter: with `overwrite`, replace only rows matching this
        filter (for example `"day = '2026-10-27'"`); all rows when empty.
    :return: the new snapshot id.
    """

    template_fields: Sequence[str] = ("table", "rows", "overwrite_filter")
    template_fields_renderers = {"rows": "json"}

    def __init__(
        self,
        *,
        table: str,
        rows: Sequence[Mapping[str, Any]] | str,
        overwrite: bool = False,
        overwrite_filter: str = "",
        loams_conn_id: str = LoamsIcebergHook.default_conn_name,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self.table = table
        self.rows = rows
        self.overwrite = overwrite
        self.overwrite_filter = overwrite_filter
        self.loams_conn_id = loams_conn_id

    def execute(self, context: Context) -> int | None:
        if isinstance(self.rows, str):
            raise TypeError(
                "rows rendered to text; set render_template_as_native_obj=True on the DAG or pass a list"
            )
        hook = LoamsIcebergHook(loams_conn_id=self.loams_conn_id)
        rows = list(self.rows)
        if self.overwrite:
            snapshot = hook.overwrite_rows(self.table, rows, overwrite_filter=self.overwrite_filter)
        else:
            snapshot = hook.append_rows(self.table, rows)
        self.log.info(
            "%s %d rows in %s: snapshot %s",
            "Wrote" if self.overwrite else "Appended",
            len(rows),
            self.table,
            snapshot,
        )
        return snapshot


class LoamsIcebergScanOperator(BaseOperator):
    """Reads rows from a table (returns them, so keep `limit` small)."""

    template_fields: Sequence[str] = ("table", "row_filter", "snapshot_id")

    def __init__(
        self,
        *,
        table: str,
        row_filter: str = "",
        selected_fields: Sequence[str] = ("*",),
        limit: int | None = 1000,
        snapshot_id: int | None = None,
        loams_conn_id: str = LoamsIcebergHook.default_conn_name,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self.table = table
        self.row_filter = row_filter
        self.selected_fields = tuple(selected_fields)
        self.limit = limit
        self.snapshot_id = snapshot_id
        self.loams_conn_id = loams_conn_id

    def execute(self, context: Context) -> list[dict[str, Any]]:
        return LoamsIcebergHook(loams_conn_id=self.loams_conn_id).scan_rows(
            self.table,
            row_filter=self.row_filter,
            selected_fields=self.selected_fields,
            limit=self.limit,
            snapshot_id=int(self.snapshot_id) if self.snapshot_id not in (None, "") else None,
        )
