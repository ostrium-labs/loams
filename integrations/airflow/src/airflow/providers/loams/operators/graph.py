# SPDX-License-Identifier: Apache-2.0
"""`LoamsGraphQueryOperator`: a GQL or openCypher statement on Loams Graph."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Any

from airflow.sdk import BaseOperator

from airflow.providers.loams.hooks.graph import LoamsGraphHook

if TYPE_CHECKING:
    from airflow.sdk import Context

__all__ = ["LoamsGraphQueryOperator"]


class LoamsGraphQueryOperator(BaseOperator):
    """Runs one statement on a graph and returns its rows.

    :param statement: the statement (templated; `.gql` and `.cypher` files work).
    :param graph: the graph; the connection's extra `graph` by default.
    :param parameters: bound as `$name` values, never interpolated.
    :param language: `gql` (default) or `cypher`.
    :param read_only: refuse a statement that writes.
    :param max_rows: 0 takes the graph's limit.
    :param idempotency_key: answer an earlier run of this key instead of
        running again; by default one fixed by the task try, so a retry
        after a lost answer does not write twice.
    :return: `{"columns", "column_types", "rows", "truncated", ...}`, rows
        as dicts keyed by column.
    """

    template_fields: Sequence[str] = ("statement", "graph", "parameters", "idempotency_key")
    template_ext: Sequence[str] = (".gql", ".cypher")
    template_fields_renderers = {"parameters": "json"}
    ui_color = "#d4c5f9"

    def __init__(
        self,
        *,
        statement: str,
        graph: str | None = None,
        parameters: Mapping[str, Any] | None = None,
        language: str = "gql",
        read_only: bool = False,
        max_rows: int = 0,
        timeout_ms: int = 0,
        idempotency_key: str | None = None,
        loams_conn_id: str = LoamsGraphHook.default_conn_name,
        **kwargs: Any,
    ) -> None:
        super().__init__(**kwargs)
        self.statement = statement
        self.graph = graph
        self.parameters = dict(parameters or {})
        self.language = language
        self.read_only = read_only
        self.max_rows = max_rows
        self.timeout_ms = timeout_ms
        self.idempotency_key = idempotency_key
        self.loams_conn_id = loams_conn_id

    def execute(self, context: Context) -> dict[str, Any]:
        key = self.idempotency_key
        if key is None and not self.read_only:
            ti = context["ti"]
            key = f"airflow:{ti.dag_id}:{ti.task_id}:{ti.run_id}:{getattr(ti, 'map_index', -1)}"
        result = LoamsGraphHook(loams_conn_id=self.loams_conn_id, graph=self.graph).query(
            self.statement,
            parameters=self.parameters,
            language=self.language,
            read_only=self.read_only,
            max_rows=self.max_rows,
            timeout_ms=self.timeout_ms,
            idempotency_key=key,
        )
        self.log.info(
            "Graph statement answered %d rows%s", len(result.rows), " (truncated)" if result.truncated else ""
        )
        return result.to_dict()
